//! Feature flag definitions in `control.db`.
//!
//! One table, `flag_definitions`, project-scoped, soft-deleted, with a
//! version that bumps on every change (`metadata.version` on the wire). The
//! `/flags` hot path reads compiled definitions from a bounded per-project
//! cache; every write invalidates its project's entry under a generation
//! counter so a racing reader can never re-insert stale definitions.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;

use super::eval::CompiledFlag;
use super::validate::{self, Invalid, MAX_FLAGS_PER_PROJECT};
use crate::contract::flags::{FeatureFlag, FeatureFlagInput, FlagFilters};
use crate::storage_bootstrap::CONTROL_APPLICATION_ID;

/// Bound on cached projects. A full cache is cleared wholesale: the next
/// request per project reloads from SQLite.
const MAX_CACHED_PROJECTS: usize = 1_024;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS flag_definitions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    filters TEXT NOT NULL CHECK(json_valid(filters)),
    ensure_experience_continuity INTEGER NOT NULL DEFAULT 0
        CHECK(ensure_experience_continuity IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS flag_definitions_live_key
    ON flag_definitions(project_id, key) WHERE deleted = 0;
CREATE INDEX IF NOT EXISTS flag_definitions_project
    ON flag_definitions(project_id, deleted, id);
"#;

const COLUMNS: &str =
    "id,key,name,active,filters,ensure_experience_continuity,created_at,updated_at,version";

#[derive(Debug)]
pub enum FlagStoreError {
    Database(rusqlite::Error),
    InvalidStorage,
    Unavailable,
    NotFound,
    /// A live flag with this key already exists in the project.
    Conflict,
    Invalid(Invalid),
    LimitExceeded(String),
    Corrupt(String),
}

impl std::fmt::Display for FlagStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "flag store database error: {error}"),
            Self::InvalidStorage => formatter.write_str("not a validated Hoglet control database"),
            Self::Unavailable => formatter.write_str("flag store unavailable"),
            Self::NotFound => formatter.write_str("flag not found"),
            Self::Conflict => formatter.write_str("a flag with this key already exists"),
            Self::Invalid(invalid) => write!(formatter, "invalid flag: {invalid}"),
            Self::LimitExceeded(message) => formatter.write_str(message),
            Self::Corrupt(message) => write!(formatter, "corrupt flag definition: {message}"),
        }
    }
}

impl std::error::Error for FlagStoreError {}

impl From<rusqlite::Error> for FlagStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<Invalid> for FlagStoreError {
    fn from(invalid: Invalid) -> Self {
        Self::Invalid(invalid)
    }
}

/// `PATCH` body: every field optional; absent fields keep their value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FeatureFlagPatch {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub filters: Option<FlagFilters>,
    #[serde(default)]
    pub ensure_experience_continuity: Option<bool>,
}

/// A project's live definitions, compiled for evaluation.
#[derive(Debug, Default)]
pub struct ProjectFlags {
    pub flags: Vec<CompiledFlag>,
}

#[derive(Default)]
struct Cache {
    generation: u64,
    projects: HashMap<String, Arc<ProjectFlags>>,
}

pub struct FlagStore {
    connection: Mutex<Connection>,
    cache: Mutex<Cache>,
}

impl FlagStore {
    /// Open the validated control database (identity is checked before any
    /// pragma or schema write) and ensure the flag table exists.
    pub fn open(control_db: &Path) -> Result<Self, FlagStoreError> {
        if !control_db.is_file() {
            return Err(FlagStoreError::InvalidStorage);
        }
        let mut connection = Connection::open_with_flags(
            control_db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        validate_control_database(&connection)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(SCHEMA)?;
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
            cache: Mutex::new(Cache::default()),
        })
    }

    /// Live flags, newest first.
    pub fn list(&self, project_id: &str) -> Result<Vec<FeatureFlag>, FlagStoreError> {
        Ok(self
            .load_live(project_id, "ORDER BY id DESC")?
            .into_iter()
            .map(|(flag, _)| flag)
            .collect())
    }

    /// Live flags with their versions, oldest first (local evaluation).
    pub fn definitions(&self, project_id: &str) -> Result<Vec<(FeatureFlag, i64)>, FlagStoreError> {
        self.load_live(project_id, "ORDER BY id")
    }

    pub fn get(&self, project_id: &str, id: i64) -> Result<FeatureFlag, FlagStoreError> {
        let connection = self.lock()?;
        load(&connection, project_id, id).map(|(flag, _)| flag)
    }

    pub fn get_with_version(
        &self,
        project_id: &str,
        id: i64,
    ) -> Result<(FeatureFlag, i64), FlagStoreError> {
        let connection = self.lock()?;
        load(&connection, project_id, id)
    }

    pub fn create(
        &self,
        project_id: &str,
        input: &FeatureFlagInput,
    ) -> Result<FeatureFlag, FlagStoreError> {
        validate::validate_input(input)?;
        let filters = encode_filters(&input.filters)?;
        let now = timestamp();
        let id = {
            let mut connection = self.lock()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let live: i64 = transaction.query_row(
                "SELECT count(*) FROM flag_definitions WHERE project_id=?1 AND deleted=0",
                [project_id],
                |row| row.get(0),
            )?;
            if usize::try_from(live).unwrap_or(usize::MAX) >= MAX_FLAGS_PER_PROJECT {
                return Err(FlagStoreError::LimitExceeded(format!(
                    "a project may have at most {MAX_FLAGS_PER_PROJECT} feature flags"
                )));
            }
            let project_exists = transaction
                .query_row("SELECT 1 FROM projects WHERE id=?1", [project_id], |_| {
                    Ok(())
                })
                .optional()?
                .is_some();
            if !project_exists {
                return Err(FlagStoreError::NotFound);
            }
            transaction
                .execute(
                    "INSERT INTO flag_definitions(
                        project_id,key,name,active,filters,ensure_experience_continuity,
                        version,deleted,created_at,updated_at
                     ) VALUES (?1,?2,?3,?4,?5,?6,1,0,?7,?7)",
                    params![
                        project_id,
                        input.key,
                        input.name,
                        input.active,
                        filters,
                        input.ensure_experience_continuity,
                        now
                    ],
                )
                .map_err(map_write_error)?;
            let id = transaction.last_insert_rowid();
            transaction.commit()?;
            id
        };
        self.invalidate(project_id);
        self.get(project_id, id)
    }

    pub fn update(
        &self,
        project_id: &str,
        id: i64,
        patch: &FeatureFlagPatch,
    ) -> Result<FeatureFlag, FlagStoreError> {
        let updated = {
            let mut connection = self.lock()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (current, _) = load(&transaction, project_id, id)?;
            let next = FeatureFlagInput {
                key: patch.key.clone().unwrap_or(current.key),
                name: patch.name.clone().unwrap_or(current.name),
                active: patch.active.unwrap_or(current.active),
                filters: patch.filters.clone().unwrap_or(current.filters),
                ensure_experience_continuity: patch
                    .ensure_experience_continuity
                    .unwrap_or(current.ensure_experience_continuity),
            };
            validate::validate_input(&next)?;
            let filters = encode_filters(&next.filters)?;
            transaction
                .execute(
                    "UPDATE flag_definitions SET
                        key=?3,name=?4,active=?5,filters=?6,ensure_experience_continuity=?7,
                        version=version+1,updated_at=?8
                     WHERE project_id=?1 AND id=?2 AND deleted=0",
                    params![
                        project_id,
                        id,
                        next.key,
                        next.name,
                        next.active,
                        filters,
                        next.ensure_experience_continuity,
                        timestamp()
                    ],
                )
                .map_err(map_write_error)?;
            let (updated, _) = load(&transaction, project_id, id)?;
            transaction.commit()?;
            updated
        };
        self.invalidate(project_id);
        Ok(updated)
    }

    /// Soft delete: the row stays for history, the key becomes reusable.
    pub fn delete(&self, project_id: &str, id: i64) -> Result<(), FlagStoreError> {
        let changed = {
            let connection = self.lock()?;
            connection.execute(
                "UPDATE flag_definitions SET deleted=1,active=0,version=version+1,updated_at=?3
                 WHERE project_id=?1 AND id=?2 AND deleted=0",
                params![project_id, id, timestamp()],
            )?
        };
        if changed == 0 {
            return Err(FlagStoreError::NotFound);
        }
        self.invalidate(project_id);
        Ok(())
    }

    /// Live definitions compiled for evaluation, from the bounded cache.
    pub fn compiled(&self, project_id: &str) -> Result<Arc<ProjectFlags>, FlagStoreError> {
        let generation = {
            let cache = self.lock_cache();
            if let Some(cached) = cache.projects.get(project_id) {
                return Ok(cached.clone());
            }
            cache.generation
        };
        let flags = self
            .definitions(project_id)?
            .into_iter()
            .map(|(flag, version)| CompiledFlag::compile(flag, version))
            .collect();
        let compiled = Arc::new(ProjectFlags { flags });
        let mut cache = self.lock_cache();
        // A write since our read means these definitions may be stale; serve
        // them for this request (they were current when read) but don't cache.
        if cache.generation == generation {
            if cache.projects.len() >= MAX_CACHED_PROJECTS {
                cache.projects.clear();
            }
            cache
                .projects
                .insert(project_id.to_owned(), compiled.clone());
        }
        Ok(compiled)
    }

    fn load_live(
        &self,
        project_id: &str,
        order: &str,
    ) -> Result<Vec<(FeatureFlag, i64)>, FlagStoreError> {
        let connection = self.lock()?;
        let sql = format!(
            "SELECT {COLUMNS} FROM flag_definitions
             WHERE project_id=?1 AND deleted=0 {order} LIMIT ?2"
        );
        let mut statement = connection.prepare_cached(&sql)?;
        let rows =
            statement.query_map(params![project_id, MAX_FLAGS_PER_PROJECT as i64], read_row)?;
        rows.map(|row| decode(row?)).collect()
    }

    fn invalidate(&self, project_id: &str) {
        let mut cache = self.lock_cache();
        cache.generation = cache.generation.wrapping_add(1);
        cache.projects.remove(project_id);
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, FlagStoreError> {
        self.connection
            .lock()
            .map_err(|_| FlagStoreError::Unavailable)
    }

    fn lock_cache(&self) -> MutexGuard<'_, Cache> {
        // The cache holds no invariant a panic could break; recover it.
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

type RawRow = (i64, String, String, bool, String, bool, String, String, i64);

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn decode(row: RawRow) -> Result<(FeatureFlag, i64), FlagStoreError> {
    let (id, key, name, active, filters, continuity, created_at, updated_at, version) = row;
    let filters: FlagFilters = serde_json::from_str(&filters)
        .map_err(|error| FlagStoreError::Corrupt(format!("flag {id}: {error}")))?;
    Ok((
        FeatureFlag {
            id,
            key,
            name,
            active,
            filters,
            ensure_experience_continuity: continuity,
            created_at,
            updated_at,
        },
        version,
    ))
}

fn load(
    connection: &Connection,
    project_id: &str,
    id: i64,
) -> Result<(FeatureFlag, i64), FlagStoreError> {
    let row = connection
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM flag_definitions
                 WHERE project_id=?1 AND id=?2 AND deleted=0"
            ),
            params![project_id, id],
            read_row,
        )
        .optional()?
        .ok_or(FlagStoreError::NotFound)?;
    decode(row)
}

fn encode_filters(filters: &FlagFilters) -> Result<String, FlagStoreError> {
    serde_json::to_string(filters).map_err(|error| {
        FlagStoreError::Invalid(Invalid {
            field: "filters".into(),
            message: error.to_string(),
        })
    })
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn map_write_error(error: rusqlite::Error) -> FlagStoreError {
    match error {
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            FlagStoreError::Conflict
        }
        other => FlagStoreError::Database(other),
    }
}

fn validate_control_database(connection: &Connection) -> Result<(), FlagStoreError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| FlagStoreError::InvalidStorage)?;
    if application_id != CONTROL_APPLICATION_ID {
        return Err(FlagStoreError::InvalidStorage);
    }
    let metadata: Option<(String, String)> = connection
        .query_row(
            "SELECT pair_id,database_role FROM database_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| FlagStoreError::InvalidStorage)?;
    if !matches!(metadata, Some((ref pair_id, ref role)) if !pair_id.is_empty() && role == "control")
    {
        return Err(FlagStoreError::InvalidStorage);
    }
    Ok(())
}
