//! Read-only catalog queries over rebuildable `projections.db` state.
//!
//! Publication owns writes and exactly-once ordering. This module validates the
//! database role before opening a separate read connection and exposes only
//! project-scoped autocomplete operations, shaped as `contract::persons`
//! catalog types.
//!
//! Event names, event property keys and event property values come from the
//! catalog tables maintained at publication. Person property keys and values
//! are derived on read from the most recent [`PERSON_SCAN_LIMIT`] persons of
//! the project via SQLite `json_each` — bounded, and exact for small projects.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::contract::common::PropertySource;
use crate::contract::persons::{CatalogEvent, CatalogProperty, CatalogValue};
use crate::storage_bootstrap::PROJECTIONS_APPLICATION_ID;

/// Most event names returned.
pub const MAX_CATALOG_EVENTS: usize = 200;
/// Most property keys returned.
pub const MAX_CATALOG_PROPERTIES: usize = 500;
/// Most property values returned.
pub const MAX_CATALOG_VALUES: usize = 100;
/// Persons scanned to derive person property keys and values.
pub const PERSON_SCAN_LIMIT: usize = 10_000;
/// Longest search string, in characters.
pub const MAX_CATALOG_SEARCH: usize = 200;
/// Longest property key.
pub const MAX_CATALOG_KEY: usize = 1_000;

#[derive(Debug)]
pub enum ProjectionCatalogError {
    InvalidStorage,
    InvalidRequest(&'static str),
    Database(rusqlite::Error),
    Unavailable,
}

impl std::fmt::Display for ProjectionCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStorage => formatter.write_str("not a validated projections database"),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::Database(error) => {
                write!(formatter, "projection catalog database error: {error}")
            }
            Self::Unavailable => formatter.write_str("projection catalog unavailable"),
        }
    }
}

impl std::error::Error for ProjectionCatalogError {}

impl From<rusqlite::Error> for ProjectionCatalogError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub struct ProjectionCatalog {
    connection: Mutex<Connection>,
}

fn bounded_search(search: &str) -> String {
    search.trim().chars().take(MAX_CATALOG_SEARCH).collect()
}

fn rfc3339_millis(millis: i64) -> Option<String> {
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Contract property type of a stored type guess or SQLite `json_each` type.
fn property_type(raw: &str) -> &'static str {
    match raw {
        "number" | "integer" | "real" => "number",
        "boolean" | "true" | "false" => "boolean",
        "array" => "array",
        "object" => "object",
        _ => "string",
    }
}

/// Bounded subquery: rowids of the project's most recent persons.
const RECENT_PERSONS: &str = "SELECT rowid FROM persons WHERE project_id = ?1
                              ORDER BY rowid DESC LIMIT ?2";

impl ProjectionCatalog {
    pub fn open(path: &Path) -> Result<Self, ProjectionCatalogError> {
        if !path.is_file() {
            return Err(ProjectionCatalogError::InvalidStorage);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        validate_projection_database(&connection)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        // Prove the publication-owned catalog schema exists without mutating it.
        connection
            .prepare("SELECT project_id,name FROM event_names LIMIT 0")
            .map_err(|_| ProjectionCatalogError::InvalidStorage)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Event names containing `search` (case-insensitive), by count.
    pub fn event_names(
        &self,
        project_id: &str,
        search: &str,
        limit: usize,
    ) -> Result<Vec<CatalogEvent>, ProjectionCatalogError> {
        let connection = self.lock()?;
        let limit = limit.clamp(1, MAX_CATALOG_EVENTS) as i64;
        let mut statement = connection.prepare_cached(
            "SELECT name, count, last_seen FROM event_names
             WHERE project_id = ?1 AND instr(lower(name), lower(?2)) > 0
             ORDER BY count DESC, name LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![project_id, bounded_search(search), limit], |row| {
                Ok(CatalogEvent {
                    name: row.get(0)?,
                    count: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    last_seen: rfc3339_millis(row.get(2)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Property keys of `source` containing `search`, by count.
    ///
    /// Event counts are events carrying the key; person counts are persons
    /// (within the scanned window) carrying it.
    pub fn property_keys(
        &self,
        project_id: &str,
        source: PropertySource,
        search: &str,
    ) -> Result<Vec<CatalogProperty>, ProjectionCatalogError> {
        let connection = self.lock()?;
        let search = bounded_search(search);
        let mut keys = match source {
            PropertySource::Event => {
                let mut statement = connection.prepare_cached(
                    "SELECT key, type_guess, count FROM property_keys
                     WHERE project_id = ?1 AND source = 'event'
                       AND instr(lower(key), lower(?2)) > 0
                     ORDER BY count DESC, key LIMIT ?3",
                )?;
                statement
                    .query_map(
                        params![project_id, search, MAX_CATALOG_PROPERTIES as i64],
                        |row| {
                            Ok(CatalogProperty {
                                key: row.get(0)?,
                                source: "event".to_owned(),
                                property_type: property_type(&row.get::<_, String>(1)?).to_owned(),
                                count: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                            })
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()?
            }
            PropertySource::Person => {
                let mut statement = connection.prepare_cached(&format!(
                    "SELECT j.key, j.type, count(*) FROM persons p, json_each(p.properties) j
                     WHERE p.rowid IN ({RECENT_PERSONS})
                       AND j.type <> 'null'
                       AND instr(lower(j.key), lower(?3)) > 0
                     GROUP BY j.key, j.type"
                ))?;
                // key -> (persons with it, (dominant type, its count)).
                let mut folded: HashMap<String, (u64, (&'static str, u64))> = HashMap::new();
                let mut rows =
                    statement.query(params![project_id, PERSON_SCAN_LIMIT as i64, search])?;
                while let Some(row) = rows.next()? {
                    let key: String = row.get(0)?;
                    let kind = property_type(&row.get::<_, String>(1)?);
                    let count = u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0);
                    let entry = folded.entry(key).or_insert((0, (kind, 0)));
                    entry.0 += count;
                    if count > entry.1.1 {
                        entry.1 = (kind, count);
                    }
                }
                folded
                    .into_iter()
                    .map(|(key, (count, (kind, _)))| CatalogProperty {
                        key,
                        source: "person".to_owned(),
                        property_type: kind.to_owned(),
                        count,
                    })
                    .collect()
            }
        };
        keys.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        keys.truncate(MAX_CATALOG_PROPERTIES);
        Ok(keys)
    }

    /// Values of one property containing `search`, by count.
    pub fn property_values(
        &self,
        project_id: &str,
        source: PropertySource,
        key: &str,
        search: &str,
        limit: usize,
    ) -> Result<Vec<CatalogValue>, ProjectionCatalogError> {
        if key.is_empty() || key.len() > MAX_CATALOG_KEY {
            return Err(ProjectionCatalogError::InvalidRequest("key"));
        }
        let connection = self.lock()?;
        let limit = limit.clamp(1, MAX_CATALOG_VALUES) as i64;
        let search = bounded_search(search);
        let read = |row: &rusqlite::Row<'_>| {
            Ok(CatalogValue {
                value: row.get(0)?,
                count: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
            })
        };
        let values = match source {
            PropertySource::Event => connection
                .prepare_cached(
                    "SELECT value, count FROM property_values
                     WHERE project_id = ?1 AND key = ?2 AND instr(lower(value), lower(?3)) > 0
                     ORDER BY count DESC, value LIMIT ?4",
                )?
                .query_map(params![project_id, key, search, limit], read)?
                .collect::<Result<Vec<_>, _>>()?,
            PropertySource::Person => connection
                .prepare_cached(&format!(
                    "SELECT v, count(*) AS n FROM (
                         SELECT CASE j.type WHEN 'true' THEN 'true' WHEN 'false' THEN 'false'
                                ELSE CAST(j.value AS TEXT) END AS v
                         FROM persons p, json_each(p.properties) j
                         WHERE p.rowid IN ({RECENT_PERSONS})
                           AND j.key = ?3 AND j.type <> 'null'
                     )
                     WHERE instr(lower(v), lower(?4)) > 0
                     GROUP BY v ORDER BY n DESC, v LIMIT ?5"
                ))?
                .query_map(
                    params![project_id, PERSON_SCAN_LIMIT as i64, key, search, limit],
                    read,
                )?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(values)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, ProjectionCatalogError> {
        self.connection
            .lock()
            .map_err(|_| ProjectionCatalogError::Unavailable)
    }
}

fn validate_projection_database(connection: &Connection) -> Result<(), ProjectionCatalogError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| ProjectionCatalogError::InvalidStorage)?;
    if application_id != PROJECTIONS_APPLICATION_ID {
        return Err(ProjectionCatalogError::InvalidStorage);
    }
    let metadata: Option<(String, String)> = connection
        .query_row(
            "SELECT pair_id,database_role FROM database_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| ProjectionCatalogError::InvalidStorage)?;
    if !matches!(metadata, Some((ref pair_id, ref role)) if !pair_id.is_empty() && role == "projections")
    {
        return Err(ProjectionCatalogError::InvalidStorage);
    }
    Ok(())
}
