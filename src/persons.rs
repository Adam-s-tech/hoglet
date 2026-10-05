//! Read-only access to the identity projection in `projections.db`.
//!
//! The publication thread is the only writer of persons and distinct ids
//! (`projections.rs`); everything else — flag evaluation, queries, the persons
//! API — reads through this store on its own SQLite connections, so a slow
//! reader can never block publication (SQLite WAL mode).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value};

/// Explicit bound on pooled read connections.
const MAX_POOLED_CONNECTIONS: usize = 8;
/// Explicit bound on one override sync batch.
pub const MAX_OVERRIDE_BATCH: usize = 100_000;
/// Largest page of the persons list.
pub const MAX_PERSON_PAGE: usize = 100;
/// Longest persons search string, in characters.
pub const MAX_PERSON_SEARCH: usize = 200;
/// Most distinct ids resolved, or returned for one person, per call.
pub const MAX_DISTINCT_IDS_PER_CALL: usize = 1_000;

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
                .map(
                    |(id, properties, is_identified, created_at, first_seen_key)| {
                        Ok(PersonRecord {
                            project_id: project_id.to_owned(),
                            id,
                            properties: decode_object(&properties)?,
                            is_identified,
                            created_at,
                            first_seen_key,
                        })
                    },
                )
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

/// Keyset position in the persons list (newest first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonCursor {
    pub created_at: String,
    pub id: String,
}

/// One row of the persons list.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonListEntry {
    pub person: PersonRecord,
    /// First-seen order, at most the requested number.
    pub distinct_ids: Vec<String>,
    /// Newest event time, RFC 3339 UTC.
    pub last_seen: Option<String>,
}

type RawPerson = (PersonRecord, String);

fn read_person(project_id: &str, row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPerson> {
    Ok((
        PersonRecord {
            project_id: project_id.to_owned(),
            id: row.get(0)?,
            properties: Map::new(),
            is_identified: row.get(2)?,
            created_at: row.get(3)?,
            first_seen_key: row.get(4)?,
        },
        row.get(1)?,
    ))
}

fn finish_person((mut person, encoded): RawPerson) -> Result<PersonRecord, PersonStoreError> {
    person.properties = decode_object(&encoded)?;
    Ok(person)
}

fn distinct_ids_on(
    connection: &Connection,
    project_id: &str,
    person_id: &str,
    limit: usize,
) -> Result<Vec<String>, PersonStoreError> {
    let limit = limit.min(MAX_DISTINCT_IDS_PER_CALL) as i64;
    let mut statement = connection.prepare_cached(
        "SELECT distinct_id FROM distinct_ids
         WHERE project_id = ?1 AND person_id = ?2
         ORDER BY rowid
         LIMIT ?3",
    )?;
    let ids = statement
        .query_map(params![project_id, person_id, limit], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}

impl PersonStore {
    /// Persons of a project, newest first, keyset-paged.
    ///
    /// `search` matches a distinct id by exact value or prefix, or the
    /// `email`/`name` person properties case-insensitively by substring.
    /// Returns at most `MAX_PERSON_PAGE + 1` rows (one look-ahead row).
    pub fn list_persons(
        &self,
        project_id: &str,
        search: Option<&str>,
        after: Option<&PersonCursor>,
        limit: usize,
        distinct_ids_per_person: usize,
    ) -> Result<Vec<PersonListEntry>, PersonStoreError> {
        let limit = limit.clamp(1, MAX_PERSON_PAGE + 1) as i64;
        let search: Option<String> = search
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(MAX_PERSON_SEARCH).collect());
        self.with_connection(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT p.id, p.properties, p.is_identified, p.created_at, p.first_seen_key,
                        p.last_seen
                 FROM persons p
                 WHERE p.project_id = ?1
                   AND (?2 IS NULL OR p.created_at < ?2 OR (p.created_at = ?2 AND p.id < ?3))
                   AND (?4 IS NULL
                        OR p.id IN (SELECT d.person_id FROM distinct_ids d
                                    WHERE d.project_id = ?1
                                      AND d.distinct_id >= ?4
                                      AND d.distinct_id <= ?4 || char(1114111))
                        OR instr(lower(coalesce(json_extract(p.properties, '$.email'), '')),
                                 lower(?4)) > 0
                        OR instr(lower(coalesce(json_extract(p.properties, '$.name'), '')),
                                 lower(?4)) > 0)
                 ORDER BY p.created_at DESC, p.id DESC
                 LIMIT ?5",
            )?;
            let rows = statement
                .query_map(
                    params![
                        project_id,
                        after.map(|cursor| cursor.created_at.as_str()),
                        after.map(|cursor| cursor.id.as_str()),
                        search.as_deref(),
                        limit
                    ],
                    |row| Ok((read_person(project_id, row)?, row.get::<_, Option<String>>(5)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut entries = Vec::with_capacity(rows.len());
            for (row, last_seen) in rows {
                let person = finish_person(row)?;
                let distinct_ids =
                    distinct_ids_on(connection, project_id, &person.id, distinct_ids_per_person)?;
                entries.push(PersonListEntry {
                    person,
                    distinct_ids,
                    last_seen,
                });
            }
            Ok(entries)
        })
    }

    /// One person by id.
    pub fn person(
        &self,
        project_id: &str,
        person_id: &str,
    ) -> Result<Option<PersonRecord>, PersonStoreError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, properties, is_identified, created_at, first_seen_key
                     FROM persons WHERE project_id = ?1 AND id = ?2",
                    params![project_id, person_id],
                    |row| read_person(project_id, row),
                )
                .optional()?
                .map(finish_person)
                .transpose()
        })
    }

    /// A person's distinct ids in first-seen order, at most `limit` (itself
    /// capped at [`MAX_DISTINCT_IDS_PER_CALL`]).
    pub fn distinct_ids_of(
        &self,
        project_id: &str,
        person_id: &str,
        limit: usize,
    ) -> Result<Vec<String>, PersonStoreError> {
        self.with_connection(|connection| distinct_ids_on(connection, project_id, person_id, limit))
    }

    /// `distinct_id -> person_id` for the given ids. Ids the projection has
    /// not seen yet are absent. At most [`MAX_DISTINCT_IDS_PER_CALL`] ids are
    /// resolved per call.
    pub fn resolve_distinct_ids(
        &self,
        project_id: &str,
        distinct_ids: &[String],
    ) -> Result<HashMap<String, String>, PersonStoreError> {
        let ids = &distinct_ids[..distinct_ids.len().min(MAX_DISTINCT_IDS_PER_CALL)];
        self.with_connection(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT person_id FROM distinct_ids WHERE project_id = ?1 AND distinct_id = ?2",
            )?;
            let mut resolved = HashMap::with_capacity(ids.len());
            for id in ids {
                if resolved.contains_key(id) {
                    continue;
                }
                if let Some(person_id) = statement
                    .query_row(params![project_id, id], |row| row.get::<_, String>(0))
                    .optional()?
                {
                    resolved.insert(id.clone(), person_id);
                }
            }
            Ok(resolved)
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
