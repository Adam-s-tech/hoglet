//! Read-only access to the identity projection in `projections.db`.
//!
//! The publication thread is the only writer of persons and distinct ids
//! (`projections.rs`); everything else — flag evaluation, queries, the persons
//! API — reads through this store on its own SQLite connections, so a slow
//! reader can never block publication (SQLite WAL mode).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value};

/// Explicit bound on pooled read connections.
const MAX_POOLED_CONNECTIONS: usize = 8;
/// Explicit bound on one override sync batch.
pub const MAX_OVERRIDE_BATCH: usize = 100_000;

#[derive(Debug)]
pub enum PersonStoreError {
    Database(rusqlite::Error),
    Corrupt(String),
}

impl std::fmt::Display for PersonStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "person store error: {error}"),
            Self::Corrupt(message) => write!(formatter, "corrupt person data: {message}"),
        }
    }
}

impl std::error::Error for PersonStoreError {}

impl From<rusqlite::Error> for PersonStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// One person, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonRecord {
    pub project_id: String,
    pub id: String,
    pub properties: Map<String, Value>,
    pub is_identified: bool,
    /// RFC 3339.
    pub created_at: String,
    /// Stable flag bucketing key: survives merges (experience continuity).
    pub first_seen_key: String,
}

/// One identity override change. `person_id == distinct_id` means the
/// override no longer exists and must be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideChange {
    pub project_id: String,
    pub distinct_id: String,
    pub person_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideBatch {
    /// If this differs from the reader's last seen epoch, the reader must
    /// discard everything and resync from seq 0.
    pub epoch: u64,
    /// Highest seq included. Pass it back as `after_seq`.
    pub max_seq: u64,
    pub changes: Vec<OverrideChange>,
    /// More changes are pending beyond this bounded batch.
    pub truncated: bool,
}

pub struct PersonStore {
    path: PathBuf,
    pool: Mutex<Vec<Connection>>,
}

impl PersonStore {
    pub fn open(projections_db: &Path) -> Result<Self, PersonStoreError> {
        let store = Self {
            path: projections_db.to_path_buf(),
            pool: Mutex::new(Vec::new()),
        };
        // Fail at startup, not on the first request.
        store.with_connection(|_| Ok(()))?;
        Ok(store)
    }

    /// Run `f` on a pooled read-only connection.
    pub fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, PersonStoreError>,
    ) -> Result<T, PersonStoreError> {
        let pooled = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
        let connection = match pooled {
            Some(connection) => connection,
            None => {
                let connection = Connection::open_with_flags(
                    &self.path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                connection.busy_timeout(Duration::from_secs(5))?;
                connection
            }
        };
        let result = f(&connection);
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pool.len() < MAX_POOLED_CONNECTIONS {
            pool.push(connection);
        }
        result
    }

    /// The person a distinct id currently resolves to.
    pub fn person_for_distinct_id(
        &self,
        project_id: &str,
        distinct_id: &str,
    ) -> Result<Option<PersonRecord>, PersonStoreError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT p.id, p.properties, p.is_identified, p.created_at, p.first_seen_key
                     FROM distinct_ids d
                     JOIN persons p ON p.project_id = d.project_id AND p.id = d.person_id
                     WHERE d.project_id = ?1 AND d.distinct_id = ?2",
                    params![project_id, distinct_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, bool>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?
                .map(|(id, properties, is_identified, created_at, first_seen_key)| {
                    Ok(PersonRecord {
                        project_id: project_id.to_owned(),
                        id,
                        properties: decode_object(&properties)?,
                        is_identified,
                        created_at,
                        first_seen_key,
                    })
                })
                .transpose()
        })
    }

    /// Identity override changes with `seq > after_seq`, across all projects,
    /// oldest first, at most [`MAX_OVERRIDE_BATCH`] rows.
    pub fn overrides_since(&self, after_seq: u64) -> Result<OverrideBatch, PersonStoreError> {
        self.with_connection(|connection| {
            let (seq, epoch): (i64, i64) = connection.query_row(
                "SELECT seq, epoch FROM identity_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let mut statement = connection.prepare(
                "SELECT project_id, distinct_id, person_id, seq FROM distinct_ids
                 WHERE seq > ?1 AND seq <= ?2
                 ORDER BY seq
                 LIMIT ?3",
            )?;
            let mut rows = statement.query(params![
                after_seq as i64,
                seq,
                (MAX_OVERRIDE_BATCH + 1) as i64
            ])?;
            let mut changes: Vec<(u64, OverrideChange)> = Vec::new();
            let mut truncated = false;
            while let Some(row) = rows.next()? {
                if changes.len() == MAX_OVERRIDE_BATCH {
                    truncated = true;
                    break;
                }
                let row_seq: i64 = row.get(3)?;
                changes.push((
                    row_seq as u64,
                    OverrideChange {
                        project_id: row.get(0)?,
                        distinct_id: row.get(1)?,
                        person_id: row.get(2)?,
                    },
                ));
            }
            let max_seq = if truncated {
                // One merge stamps many rows with one seq. Never split a seq
                // across batches, or the remainder would be skipped.
                let last = changes.last().map(|(seq, _)| *seq).unwrap_or(after_seq);
                let complete = changes.iter().any(|(seq, _)| *seq < last);
                if complete {
                    changes.retain(|(seq, _)| *seq < last);
                    last - 1
                } else {
                    let rest: Vec<(u64, OverrideChange)> = connection
                        .prepare(
                            "SELECT project_id, distinct_id, person_id FROM distinct_ids
                             WHERE seq = ?1",
                        )?
                        .query_map([last as i64], |row| {
                            Ok((
                                last,
                                OverrideChange {
                                    project_id: row.get(0)?,
                                    distinct_id: row.get(1)?,
                                    person_id: row.get(2)?,
                                },
                            ))
                        })?
                        .collect::<Result<_, _>>()?;
                    changes = rest;
                    last
                }
            } else {
                (seq as u64).max(after_seq)
            };
            let changes = changes.into_iter().map(|(_, change)| change).collect();
            Ok(OverrideBatch {
                epoch: epoch as u64,
                max_seq,
                changes,
                truncated,
            })
        })
    }
}

pub fn decode_object(encoded: &str) -> Result<Map<String, Value>, PersonStoreError> {
    match serde_json::from_str(encoded) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(PersonStoreError::Corrupt(
            "person properties are not a JSON object".to_owned(),
        )),
    }
}
