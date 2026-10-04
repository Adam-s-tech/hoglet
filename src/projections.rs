//! Ordered, rebuildable identity and catalog projections.
//!
//! The publication coordinator owns the surrounding SQLite transaction.  It
//! calls [`apply_captured_event`] in WAL order, publishes the event-file
//! generation and advances its checkpoint in that same transaction.  This
//! module intentionally does not own either generations or checkpoints.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Map, Value};

use crate::capture::event::CapturedEvent;
use crate::pipeline::wal::WalCursor;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS persons (
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    is_identified INTEGER NOT NULL DEFAULT 0 CHECK(is_identified IN (0, 1)),
    properties TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(properties)),
    first_seen_key TEXT NOT NULL,
    -- Newest event time of any of the person's distinct ids (RFC 3339 UTC).
    last_seen TEXT,
    PRIMARY KEY (project_id, id)
);

-- Persons list: newest first, keyset-paged.
CREATE INDEX IF NOT EXISTS persons_created
    ON persons(project_id, created_at, id);

CREATE TABLE IF NOT EXISTS distinct_ids (
    project_id TEXT NOT NULL,
    distinct_id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    -- Change sequence for identity overrides (see `identity_state`). Rows
    -- whose person id equals their distinct id and were never repointed keep
    -- seq 0 and never need syncing.
    seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, distinct_id),
    FOREIGN KEY (project_id, person_id)
        REFERENCES persons(project_id, id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS distinct_ids_person
    ON distinct_ids(project_id, person_id);
CREATE INDEX IF NOT EXISTS distinct_ids_seq
    ON distinct_ids(project_id, seq) WHERE seq > 0;

-- Identity override feed. A person id is the first distinct id ever seen for
-- that person, so `distinct_id -> person_id` is the identity for every id
-- that was never merged. Readers therefore only need the rows where
-- `person_id != distinct_id` ("overrides"). Every write that creates or
-- changes such a row stamps it with `seq = identity_state.seq + 1`; a reader
-- that remembers the highest seq it applied can sync incrementally. A
-- change of `epoch` (erasure, rebuild) means: discard and reload everything.
CREATE TABLE IF NOT EXISTS identity_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    seq INTEGER NOT NULL DEFAULT 0,
    epoch INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO identity_state (singleton, seq, epoch) VALUES (1, 0, 0);

CREATE TABLE IF NOT EXISTS groups (
    project_id TEXT NOT NULL,
    group_type TEXT NOT NULL,
    group_key TEXT NOT NULL,
    properties TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(properties)),
    created_at TEXT NOT NULL,
    PRIMARY KEY (project_id, group_type, group_key)
);

CREATE TABLE IF NOT EXISTS event_names (
    project_id TEXT NOT NULL,
    name TEXT NOT NULL,
    last_seen INTEGER NOT NULL,
    count INTEGER NOT NULL CHECK(count > 0),
    PRIMARY KEY (project_id, name)
);

CREATE TABLE IF NOT EXISTS property_keys (
    project_id TEXT NOT NULL,
    source TEXT NOT NULL,
    key TEXT NOT NULL,
    type_guess TEXT NOT NULL,
    last_seen INTEGER NOT NULL,
    count INTEGER NOT NULL CHECK(count > 0),
    PRIMARY KEY (project_id, source, key)
);

CREATE TABLE IF NOT EXISTS property_values (
    project_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    last_seen INTEGER NOT NULL,
    count INTEGER NOT NULL CHECK(count > 0),
    PRIMARY KEY (project_id, key, value)
);
"#;

/// Whether the event changed projections or was already applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

#[derive(Debug)]
pub enum ProjectionError {
    Database(rusqlite::Error),
    InvalidJson(serde_json::Error),
    InvalidProjectId,
    CursorOutOfRange(WalCursor),
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "projection database error: {error}"),
            Self::InvalidJson(error) => write!(formatter, "invalid projected JSON: {error}"),
            Self::InvalidProjectId => formatter.write_str("project id is empty"),
            Self::CursorOutOfRange(cursor) => write!(
                formatter,
                "WAL cursor {}:{} exceeds SQLite's integer range",
                cursor.segment, cursor.byte_offset
            ),
        }
    }
}

impl std::error::Error for ProjectionError {}

impl From<rusqlite::Error> for ProjectionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for ProjectionError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidJson(error)
    }
}

/// Install projection tables into the shared `projections.db` connection.
///
/// The schema has no generation/checkpoint DDL, which lets the event-lake
/// owner evolve those tables independently.
pub fn initialize_schema(connection: &Connection) -> Result<(), ProjectionError> {
    connection.execute_batch(SCHEMA)?;
    let has_last_seen = connection
        .prepare("SELECT 1 FROM pragma_table_info('persons') WHERE name = 'last_seen'")?
        .exists([])?;
    if !has_last_seen {
        connection.execute_batch("ALTER TABLE persons ADD COLUMN last_seen TEXT")?;
    }
    Ok(())
}

/// Sortable UTC text form used for `persons.last_seen`.
fn sortable_time(time: chrono::DateTime<chrono::Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// Apply one event's rebuildable effects inside the caller's publication
/// transaction.
///
/// Callers must invoke this in WAL order; `$set`, `$set_once`, and identify
/// merges consequently have deterministic last-writer behavior. Every effect
/// is idempotent for a repeated event except catalog counts, which are
/// approximate by design. `_cursor` is accepted for call-site compatibility.
pub fn apply_captured_event(
    transaction: &Transaction<'_>,
    project_id: &str,
    event: &CapturedEvent,
    _cursor: WalCursor,
) -> Result<ApplyOutcome, ProjectionError> {
    if project_id.trim().is_empty() {
        return Err(ProjectionError::InvalidProjectId);
    }
    apply_identity(transaction, project_id, event)?;
    apply_catalog(transaction, project_id, event)?;
    Ok(ApplyOutcome::Applied)
}

/// Identity effects only, in WAL order; the caller aggregates catalog effects
/// in a [`CatalogBatch`] and flushes it once per window.
pub fn apply_identity_effects(
    transaction: &Transaction<'_>,
    project_id: &str,
    event: &CapturedEvent,
) -> Result<(), ProjectionError> {
    if project_id.trim().is_empty() {
        return Err(ProjectionError::InvalidProjectId);
    }
    apply_identity(transaction, project_id, event)
}

fn apply_identity(
    transaction: &Transaction<'_>,
    project_id: &str,
    event: &CapturedEvent,
) -> Result<(), ProjectionError> {
    let person_id = ensure_person(transaction, project_id, &event.distinct_id, event)?;
    transaction.execute(
        "UPDATE persons SET last_seen = ?3
         WHERE project_id = ?1 AND id = ?2 AND (last_seen IS NULL OR last_seen < ?3)",
        params![project_id, person_id, sortable_time(event.timestamp)],
    )?;
    let other_id = |key: &str| {
        event
            .properties
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value != event.distinct_id)
            .map(str::to_owned)
    };

    match event.event.as_str() {
        "$identify" => {
            if let Some(anonymous_id) = other_id("$anon_distinct_id") {
                merge_persons(
                    transaction,
                    project_id,
                    &anonymous_id,
                    &event.distinct_id,
                    event,
                    MergeGuard::RefuseIdentifiedSource,
                )?;
            }
            mark_identified(transaction, project_id, &event.distinct_id)?;
        }
        "$create_alias" => {
            if let Some(alias) = other_id("alias") {
                merge_persons(
                    transaction,
                    project_id,
                    &alias,
                    &event.distinct_id,
                    event,
                    MergeGuard::RefuseBothIdentified,
                )?;
            }
            mark_identified(transaction, project_id, &event.distinct_id)?;
        }
        "$merge_dangerously" => {
            if let Some(alias) = other_id("alias") {
                merge_persons(
                    transaction,
                    project_id,
                    &alias,
                    &event.distinct_id,
                    event,
                    MergeGuard::Always,
                )?;
            }
        }
        "$groupidentify" => apply_group(transaction, project_id, event)?,
        _ => {}
    }

    apply_person_properties(transaction, project_id, &event.distinct_id, event)
}

fn mark_identified(
    transaction: &Transaction<'_>,
    project_id: &str,
    distinct_id: &str,
) -> Result<(), ProjectionError> {
    transaction.execute(
        "UPDATE persons SET is_identified = 1
         WHERE project_id = ?1
           AND id = (SELECT person_id FROM distinct_ids
                     WHERE project_id = ?1 AND distinct_id = ?2)",
        params![project_id, distinct_id],
    )?;
    Ok(())
}

/// Allocate the next identity override sequence number.
fn next_identity_seq(transaction: &Transaction<'_>) -> Result<i64, ProjectionError> {
    transaction.execute(
        "UPDATE identity_state SET seq = seq + 1 WHERE singleton = 1",
        [],
    )?;
    Ok(transaction.query_row(
        "SELECT seq FROM identity_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?)
}

fn ensure_person(
    transaction: &Transaction<'_>,
    project_id: &str,
    distinct_id: &str,
    event: &CapturedEvent,
) -> Result<String, ProjectionError> {
    if let Some(person_id) = transaction
        .query_row(
            "SELECT person_id FROM distinct_ids
             WHERE project_id=?1 AND distinct_id=?2",
            params![project_id, distinct_id],
            |row| row.get(0),
        )
        .optional()?
    {
        return Ok(person_id);
    }

    // Capture validation bounds distinct ids, making the first identity key a
    // compact and deterministic person id during every rebuild.
    let person_id = distinct_id.to_owned();
    transaction.execute(
        "INSERT INTO persons(
             project_id, id, created_at, is_identified, properties, first_seen_key
         ) VALUES (?1, ?2, ?3, 0, '{}', ?2)",
        params![project_id, person_id, event.timestamp.to_rfc3339()],
    )?;
    transaction.execute(
        "INSERT INTO distinct_ids(project_id, distinct_id, person_id, seq)
         VALUES (?1, ?2, ?3, 0)",
        params![project_id, distinct_id, person_id],
    )?;
    Ok(person_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeGuard {
    /// `$identify` / `$create_alias`: never fold an already identified person
    /// into another one — two logged-in users sharing a device stay apart.
    RefuseIdentifiedSource,
    /// `$create_alias`: SDKs disagree on which side is `alias` (posthog-node
    /// aliases the anonymous id into the user, posthog-python the reverse).
    /// Merge unless both people are identified; an identified person always
    /// survives.
    RefuseBothIdentified,
    /// `$merge_dangerously`: the caller asserted these are one human.
    Always,
}

/// Fold the person of `losing_distinct_id` into the person of
/// `winning_distinct_id`. The winner's properties win conflicts; the earliest
/// `created_at` survives; every distinct id of the loser is repointed.
fn merge_persons(
    transaction: &Transaction<'_>,
    project_id: &str,
    losing_distinct_id: &str,
    winning_distinct_id: &str,
    event: &CapturedEvent,
    guard: MergeGuard,
) -> Result<(), ProjectionError> {
    let mut winner = ensure_person(transaction, project_id, winning_distinct_id, event)?;
    let mut loser = ensure_person(transaction, project_id, losing_distinct_id, event)?;
    if winner == loser {
        return Ok(());
    }

    let identified = |person: &str| -> Result<bool, ProjectionError> {
        Ok(transaction.query_row(
            "SELECT is_identified FROM persons WHERE project_id=?1 AND id=?2",
            params![project_id, person],
            |row| row.get(0),
        )?)
    };
    let loser_is_identified = identified(&loser)?;
    match guard {
        MergeGuard::RefuseIdentifiedSource if loser_is_identified => return Ok(()),
        MergeGuard::RefuseBothIdentified if loser_is_identified => {
            if identified(&winner)? {
                return Ok(());
            }
            std::mem::swap(&mut winner, &mut loser);
        }
        _ => {}
    }

    let (winner_json, winner_created, winner_first_seen): (String, String, String) = transaction
        .query_row(
            "SELECT properties, created_at, first_seen_key
             FROM persons WHERE project_id=?1 AND id=?2",
            params![project_id, winner],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    let (loser_json, loser_created, loser_first_seen): (String, String, String) = transaction
        .query_row(
            "SELECT properties, created_at, first_seen_key
             FROM persons WHERE project_id=?1 AND id=?2",
            params![project_id, loser],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

    let mut merged = decode_properties(&loser_json)?;
    for (key, value) in decode_properties(&winner_json)? {
        merged.insert(key, value);
    }
    // The older person keeps its flag bucketing key, so a flag value seen
    // while anonymous survives login (experience continuity).
    let (created_at, first_seen_key) = if loser_created < winner_created {
        (loser_created, loser_first_seen)
    } else {
        (winner_created, winner_first_seen)
    };

    let seq = next_identity_seq(transaction)?;
    transaction.execute(
        "UPDATE distinct_ids SET person_id=?1, seq=?2
         WHERE project_id=?3 AND person_id=?4",
        params![winner, seq, project_id, loser],
    )?;
    transaction.execute(
        "UPDATE persons
         SET last_seen = (SELECT max(last_seen) FROM persons
                          WHERE project_id = ?1 AND id IN (?2, ?3))
         WHERE project_id = ?1 AND id = ?2",
        params![project_id, winner, loser],
    )?;
    transaction.execute(
        "UPDATE persons
         SET properties=?1, created_at=?2, first_seen_key=?3, is_identified=1
         WHERE project_id=?4 AND id=?5",
        params![
            Value::Object(merged).to_string(),
            created_at,
            first_seen_key,
            project_id,
            winner,
        ],
    )?;
    transaction.execute(
        "DELETE FROM persons WHERE project_id=?1 AND id=?2",
        params![project_id, loser],
    )?;
    Ok(())
}

fn apply_group(
    transaction: &Transaction<'_>,
    project_id: &str,
    event: &CapturedEvent,
) -> Result<(), ProjectionError> {
    let (Some(group_type), Some(group_key)) = (
        event.properties.get("$group_type").and_then(Value::as_str),
        event.properties.get("$group_key").and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }),
    ) else {
        return Ok(());
    };
    let existing: Option<String> = transaction
        .query_row(
            "SELECT properties FROM groups
             WHERE project_id=?1 AND group_type=?2 AND group_key=?3",
            params![project_id, group_type, group_key],
            |row| row.get(0),
        )
        .optional()?;
    let mut properties = match &existing {
        Some(encoded) => decode_properties(encoded)?,
        None => Map::new(),
    };
    if let Some(set) = event.properties.get("$group_set").and_then(Value::as_object) {
        for (key, value) in set {
            properties.insert(key.clone(), value.clone());
        }
    }
    transaction.execute(
        "INSERT INTO groups(project_id, group_type, group_key, properties, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(project_id, group_type, group_key)
         DO UPDATE SET properties = excluded.properties",
        params![
            project_id,
            group_type,
            group_key,
            Value::Object(properties).to_string(),
            event.timestamp.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn apply_person_properties(
    transaction: &Transaction<'_>,
    project_id: &str,
    distinct_id: &str,
    event: &CapturedEvent,
) -> Result<(), ProjectionError> {
    let set_once = event.properties.get("$set_once").and_then(Value::as_object);
    let set = event.properties.get("$set").and_then(Value::as_object);
    let unset = event.properties.get("$unset").and_then(Value::as_array);
    if set_once.is_none() && set.is_none() && unset.is_none() {
        return Ok(());
    }

    let person_id = ensure_person(transaction, project_id, distinct_id, event)?;
    let encoded: String = transaction.query_row(
        "SELECT properties FROM persons WHERE project_id=?1 AND id=?2",
        params![project_id, person_id],
        |row| row.get(0),
    )?;
    let mut properties = decode_properties(&encoded)?;

    if let Some(values) = set_once {
        for (key, value) in values {
            properties
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }
    if let Some(values) = set {
        for (key, value) in values {
            properties.insert(key.clone(), value.clone());
        }
    }
    if let Some(keys) = unset {
        for key in keys.iter().filter_map(Value::as_str) {
            properties.remove(key);
        }
    }

    transaction.execute(
        "UPDATE persons SET properties=?1 WHERE project_id=?2 AND id=?3",
        params![Value::Object(properties).to_string(), project_id, person_id],
    )?;
    Ok(())
}

/// Remove a person and every distinct id that resolves to them (GDPR
/// erasure). Returns the removed distinct ids. Bumps the identity epoch so
/// every reader of identity overrides reloads from scratch.
pub fn erase_person(
    transaction: &Transaction<'_>,
    project_id: &str,
    person_id: &str,
) -> Result<Vec<String>, ProjectionError> {
    let distinct_ids: Vec<String> = transaction
        .prepare("SELECT distinct_id FROM distinct_ids WHERE project_id = ?1 AND person_id = ?2")?
        .query_map(params![project_id, person_id], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    transaction.execute(
        "DELETE FROM distinct_ids WHERE project_id = ?1 AND person_id = ?2",
        params![project_id, person_id],
    )?;
    transaction.execute(
        "DELETE FROM persons WHERE project_id = ?1 AND id = ?2",
        params![project_id, person_id],
    )?;
    transaction.execute(
        "UPDATE identity_state SET epoch = epoch + 1, seq = seq + 1 WHERE singleton = 1",
        [],
    )?;
    Ok(distinct_ids)
}

fn decode_properties(encoded: &str) -> Result<Map<String, Value>, ProjectionError> {
    let decoded: Value = serde_json::from_str(encoded)?;
    decoded
        .as_object()
        .cloned()
        .ok_or_else(|| ProjectionError::InvalidJson(json_object_expected_error()))
}

fn json_object_expected_error() -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom("person properties must be a JSON object")
}

fn apply_catalog(
    transaction: &Transaction<'_>,
    project_id: &str,
    event: &CapturedEvent,
) -> Result<(), ProjectionError> {
    let mut batch = CatalogBatch::default();
    batch.add(project_id, event);
    batch.flush(transaction)
}

/// Distinct values remembered per property key; values beyond this are not
/// catalogued (they remain in the events). Bounds the catalog for
/// high-cardinality keys like ids and URLs.
pub const MAX_VALUES_PER_KEY: i64 = 500;
/// Longer values are never catalogued.
pub const MAX_CATALOG_VALUE_CHARS: usize = 200;
/// Keys whose values are identifiers, not categories.
const UNCATALOGUED_VALUE_KEYS: &[&str] = &[
    "$session_id",
    "$window_id",
    "$insert_id",
    "$pageview_id",
    "$device_id",
    "$anon_distinct_id",
    "$ai_trace_id",
    "$ai_span_id",
    "$ai_generation_id",
    "$time",
    "$sent_at",
    "token",
    "distinct_id",
    "$set",
    "$set_once",
    "$unset",
    "$groups",
    "$elements",
    "$elements_chain",
];

#[derive(Default)]
struct Seen {
    count: i64,
    last_seen: i64,
}

/// Catalog effects of a whole publication window, aggregated in memory and
/// written once per distinct name/key/value instead of once per event.
#[derive(Default)]
pub struct CatalogBatch {
    names: std::collections::HashMap<(String, String), Seen>,
    keys: std::collections::HashMap<(String, String), (Seen, &'static str)>,
    values: std::collections::HashMap<(String, String, String), Seen>,
}

impl CatalogBatch {
    pub fn add(&mut self, project_id: &str, event: &CapturedEvent) {
        let at = event.timestamp.timestamp_millis();
        let bump = |seen: &mut Seen| {
            seen.count += 1;
            seen.last_seen = seen.last_seen.max(at);
        };
        bump(
            self.names
                .entry((project_id.to_owned(), event.event.clone()))
                .or_default(),
        );
        for (key, value) in &event.properties {
            let entry = self
                .keys
                .entry((project_id.to_owned(), key.clone()))
                .or_insert_with(|| (Seen::default(), value_type(value)));
            bump(&mut entry.0);
            entry.1 = value_type(value);
            if value.is_null()
                || value.is_object()
                || value.is_array()
                || UNCATALOGUED_VALUE_KEYS.contains(&key.as_str())
            {
                continue;
            }
            let text = value_text(value);
            if text.chars().count() > MAX_CATALOG_VALUE_CHARS {
                continue;
            }
            bump(
                self.values
                    .entry((project_id.to_owned(), key.clone(), text))
                    .or_default(),
            );
        }
    }

    pub fn flush(self, transaction: &Transaction<'_>) -> Result<(), ProjectionError> {
        let mut names = transaction.prepare_cached(
            "INSERT INTO event_names(project_id, name, last_seen, count)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, name) DO UPDATE SET
                 last_seen=max(event_names.last_seen, excluded.last_seen),
                 count=event_names.count + excluded.count",
        )?;
        for ((project_id, name), seen) in &self.names {
            names.execute(params![project_id, name, seen.last_seen, seen.count])?;
        }
        let mut keys = transaction.prepare_cached(
            "INSERT INTO property_keys(project_id, source, key, type_guess, last_seen, count)
             VALUES (?1, 'event', ?2, ?3, ?4, ?5)
             ON CONFLICT(project_id, source, key) DO UPDATE SET
                 type_guess=excluded.type_guess,
                 last_seen=max(property_keys.last_seen, excluded.last_seen),
                 count=property_keys.count + excluded.count",
        )?;
        for ((project_id, key), (seen, kind)) in &self.keys {
            keys.execute(params![project_id, key, kind, seen.last_seen, seen.count])?;
        }
        let mut existing = transaction.prepare_cached(
            "SELECT count FROM property_values WHERE project_id=?1 AND key=?2 AND value=?3",
        )?;
        let mut cardinality = transaction.prepare_cached(
            "SELECT count(*) FROM property_values WHERE project_id=?1 AND key=?2",
        )?;
        let mut upsert = transaction.prepare_cached(
            "INSERT INTO property_values(project_id, key, value, last_seen, count)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(project_id, key, value) DO UPDATE SET
                 last_seen=max(property_values.last_seen, excluded.last_seen),
                 count=property_values.count + excluded.count",
        )?;
        let mut sizes: std::collections::HashMap<(String, String), i64> =
            std::collections::HashMap::new();
        // Most frequent first, so the cap keeps the values that matter.
        let mut values: Vec<_> = self.values.into_iter().collect();
        values.sort_by_key(|(_, seen)| std::cmp::Reverse(seen.count));
        for ((project_id, key, value), seen) in values {
            let known = existing
                .query_row(params![project_id, key, value], |row| row.get::<_, i64>(0))
                .optional()?
                .is_some();
            if !known {
                let size = match sizes.get(&(project_id.clone(), key.clone())) {
                    Some(size) => *size,
                    None => cardinality.query_row(params![project_id, key], |row| row.get(0))?,
                };
                if size >= MAX_VALUES_PER_KEY {
                    continue;
                }
                sizes.insert((project_id.clone(), key.clone()), size + 1);
            }
            upsert.execute(params![project_id, key, value, seen.last_seen, seen.count])?;
        }
        Ok(())
    }
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}
