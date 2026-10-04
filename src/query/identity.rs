//! Persons inside the engine: identity overrides, person properties, and
//! the person rows behind actor lists.

use duckdb::Connection;
use rusqlite::params;
use serde_json::{Map, Value};

use super::QueryError;
use crate::contract::persons::PersonSummary;
use crate::persons::{OverrideChange, PersonStore, PersonStoreError, decode_object};

/// Explicit bound on persons loaded for one person-property key.
pub const MAX_PERSON_PROPERTY_ROWS: usize = 1_000_000;
/// Explicit bound on override batches applied in one sync.
const MAX_SYNC_BATCHES: usize = 1_000;
const MAX_SUMMARY_DISTINCT_IDS: i64 = 10;

/// Incremental mirror of the identity override feed into the shared
/// `person_overrides` DuckDB table.
pub(crate) struct IdentitySync {
    connection: Connection,
    epoch: Option<u64>,
    seq: u64,
}

impl IdentitySync {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            epoch: None,
            seq: 0,
        }
    }

    /// Bring the table up to date; returns `(epoch, seq)` it reflects.
    pub fn sync(&mut self, persons: &PersonStore) -> Result<(u64, u64), QueryError> {
        for _ in 0..MAX_SYNC_BATCHES {
            let batch = persons.overrides_since(self.seq)?;
            if self.epoch != Some(batch.epoch) {
                let fetched_from_start = self.seq == 0;
                self.connection
                    .execute_batch("DELETE FROM main.person_overrides")?;
                self.epoch = Some(batch.epoch);
                self.seq = 0;
                if !fetched_from_start {
                    continue;
                }
            }
            if !batch.changes.is_empty() {
                self.apply(&batch.changes)?;
            }
            self.seq = batch.max_seq;
            if !batch.truncated {
                return Ok((batch.epoch, self.seq));
            }
        }
        Err(QueryError::internal(
            "identity override sync did not converge",
        ))
    }

    fn apply(&self, changes: &[OverrideChange]) -> Result<(), QueryError> {
        self.connection.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS override_stage \
                 (project_id VARCHAR, distinct_id VARCHAR, person_id VARCHAR); \
             DELETE FROM override_stage;",
        )?;
        {
            let mut appender =
                self.connection
                    .appender_to_catalog_and_db("override_stage", "temp", "main")?;
            for change in changes {
                appender.append_row(duckdb::params![
                    change.project_id,
                    change.distinct_id,
                    change.person_id
                ])?;
            }
            appender.flush()?;
        }
        // A change whose person id equals its distinct id removes the
        // override; every other change replaces it.
        self.connection.execute_batch(
            "BEGIN TRANSACTION; \
             DELETE FROM main.person_overrides o USING override_stage s \
                 WHERE o.project_id = s.project_id AND o.distinct_id = s.distinct_id; \
             INSERT INTO main.person_overrides \
                 SELECT project_id, distinct_id, person_id FROM override_stage \
                 WHERE person_id <> distinct_id; \
             COMMIT;",
        )?;
        Ok(())
    }
}

/// Text form of a person property as read from SQLite's `json_each`:
/// identical to the event-property text (strings verbatim, numbers in JSON
/// form, booleans `true`/`false`, containers as JSON, null = missing).
fn sqlite_property_text(kind: &str, value: rusqlite::types::Value) -> Option<String> {
    use rusqlite::types::Value as Sql;
    match (kind, value) {
        ("true", _) => Some("true".to_owned()),
        ("false", _) => Some("false".to_owned()),
        ("null", _) | (_, Sql::Null) => None,
        ("integer", Sql::Integer(number)) => Some(number.to_string()),
        ("real", Sql::Real(number)) => serde_json::Number::from_f64(number).map(|n| n.to_string()),
        (_, Sql::Text(text)) => Some(text),
        (_, Sql::Integer(number)) => Some(number.to_string()),
        (_, Sql::Real(number)) => serde_json::Number::from_f64(number).map(|n| n.to_string()),
        (_, Sql::Blob(_)) => None,
    }
}

/// Load `(person_id, text)` for one person-property key into the
/// connection-local temp table `pp_<index>`.
pub(crate) fn load_person_property(
    conn: &Connection,
    persons: &PersonStore,
    project_id: &str,
    key: &str,
    index: usize,
) -> Result<(), QueryError> {
    super::sql::validate_key(key)?;
    let rows = persons.with_connection(|sqlite| {
        let mut statement = sqlite.prepare(
            "SELECT p.id, j.type, j.value FROM persons p, json_each(p.properties) j
             WHERE p.project_id = ?1 AND j.key = ?2
             LIMIT ?3",
        )?;
        let mut rows = statement.query(params![
            project_id,
            key,
            (MAX_PERSON_PROPERTY_ROWS + 1) as i64
        ])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let kind: String = row.get(1)?;
            let value: rusqlite::types::Value = row.get(2)?;
            if let Some(text) = sqlite_property_text(&kind, value) {
                out.push((id, text));
            }
        }
        Ok::<_, PersonStoreError>(out)
    })?;
    if rows.len() > MAX_PERSON_PROPERTY_ROWS {
        return Err(QueryError::too_large(format!(
            "more than {MAX_PERSON_PROPERTY_ROWS} persons have the property `{key}`; \
             person-property filters and breakdowns are limited to that many persons"
        )));
    }
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE pp_{index} (person_id VARCHAR, value VARCHAR)"
    ))?;
    let mut appender = conn.appender_to_catalog_and_db(&format!("pp_{index}"), "temp", "main")?;
    for (id, text) in &rows {
        appender.append_row(duckdb::params![id, text])?;
    }
    appender.flush()?;
    Ok(())
}

fn display_name(properties: &Map<String, Value>, distinct_ids: &[String], id: &str) -> String {
    for key in ["email", "name"] {
        if let Some(Value::String(text)) = properties.get(key)
            && !text.is_empty()
        {
            return text.clone();
        }
    }
    distinct_ids
        .first()
        .cloned()
        .unwrap_or_else(|| id.to_owned())
}

/// Person rows for `ids`, in order. A person not (yet) in the projection is
/// still returned, as its own single distinct id, so lists reconcile with
/// the counts they open.
pub(crate) fn person_summaries(
    persons: &PersonStore,
    project_id: &str,
    ids: &[String],
) -> Result<Vec<PersonSummary>, QueryError> {
    Ok(persons.with_connection(|sqlite| {
        let mut person = sqlite.prepare_cached(
            "SELECT properties, is_identified, created_at FROM persons
             WHERE project_id = ?1 AND id = ?2",
        )?;
        let mut distinct = sqlite.prepare_cached(
            "SELECT distinct_id FROM distinct_ids
             WHERE project_id = ?1 AND person_id = ?2
             ORDER BY distinct_id LIMIT ?3",
        )?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let row = person
                .query_map(params![project_id, id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .next()
                .transpose()?;
            let distinct_ids: Vec<String> = distinct
                .query_map(params![project_id, id, MAX_SUMMARY_DISTINCT_IDS], |row| {
                    row.get(0)
                })?
                .collect::<Result<_, _>>()?;
            out.push(match row {
                Some((properties, is_identified, created_at)) => {
                    let properties = decode_object(&properties)?;
                    PersonSummary {
                        id: id.clone(),
                        display_name: display_name(&properties, &distinct_ids, id),
                        distinct_ids,
                        properties: Value::Object(properties),
                        is_identified,
                        created_at,
                        last_seen: None,
                    }
                }
                None => PersonSummary {
                    id: id.clone(),
                    display_name: id.clone(),
                    distinct_ids: vec![id.clone()],
                    properties: Value::Object(Map::new()),
                    is_identified: false,
                    created_at: String::new(),
                    last_seen: None,
                },
            });
        }
        Ok::<_, PersonStoreError>(out)
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(sync: &IdentitySync) -> Vec<(String, String, String)> {
        let mut statement = sync
            .connection
            .prepare(
                "SELECT project_id, distinct_id, person_id FROM main.person_overrides ORDER BY ALL",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn sync_is_incremental_deletes_self_mappings_and_reloads_on_epoch_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projections.db");
        let sqlite = rusqlite::Connection::open(&path).unwrap();
        crate::projections::initialize_schema(&sqlite).unwrap();
        let persons = PersonStore::open(&path).unwrap();
        let duck = Connection::open_in_memory().unwrap();
        duck.execute_batch(
            "CREATE TABLE person_overrides (project_id VARCHAR, distinct_id VARCHAR, person_id VARCHAR)",
        )
        .unwrap();
        let mut sync = IdentitySync::new(duck);

        let set = |sql: &str| sqlite.execute_batch(sql).unwrap();
        set(
            "INSERT INTO persons (project_id, id, created_at, first_seen_key) VALUES
                ('p', 'u', 't', 'u'), ('p', 'v', 't', 'v');
             INSERT INTO distinct_ids (project_id, distinct_id, person_id, seq) VALUES
                ('p', 'u', 'u', 0), ('p', 'a', 'u', 1), ('p', 'b', 'u', 2);
             UPDATE identity_state SET seq = 2;",
        );
        assert_eq!(sync.sync(&persons).unwrap(), (0, 2));
        assert_eq!(overrides(&sync).len(), 2);

        // seq 3: `a` moves to `v`; `b` points at itself again (removed).
        set(
            "UPDATE distinct_ids SET person_id = 'v', seq = 3 WHERE distinct_id = 'a';
             INSERT INTO persons (project_id, id, created_at, first_seen_key)
                 VALUES ('p', 'b', 't', 'b');
             UPDATE distinct_ids SET person_id = 'b', seq = 3 WHERE distinct_id = 'b';
             UPDATE identity_state SET seq = 3;",
        );
        assert_eq!(sync.sync(&persons).unwrap(), (0, 3));
        assert_eq!(overrides(&sync), vec![("p".into(), "a".into(), "v".into())]);

        // Epoch change (erasure/rebuild): everything is reloaded from seq 0.
        set("DELETE FROM distinct_ids WHERE distinct_id = 'a';
             UPDATE identity_state SET epoch = 1;");
        assert_eq!(sync.sync(&persons).unwrap(), (1, 3));
        assert!(overrides(&sync).is_empty());
    }
}
