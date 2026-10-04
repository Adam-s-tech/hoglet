//! Persons: list (SQLite only), detail (SQLite + event stats from the lake),
//! and a person's events (the feed restricted to their distinct ids).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, NaiveDate, Utc};
use duckdb::types::Value as Db;
use serde_json::{Map, Value};

use crate::contract::persons::{
    EventListResponse, PersonDetail, PersonListResponse, PersonSummary,
};
use crate::persons::{MAX_DISTINCT_IDS_PER_CALL, PersonCursor, PersonRecord, PersonStore};

use super::events::{self, FeedQuery};
use super::web::SESSION_GAP_MICROS;
use super::{ExploreError, Explorer, from_micros, parquet_source, rfc3339};

/// Distinct ids shown per person in the list.
pub const LISTED_DISTINCT_IDS: usize = 10;
/// Longest accepted cursor.
const MAX_CURSOR_LENGTH: usize = 2_048;

/// `email`, then `name`, then the first distinct id, then the person id.
pub fn display_name(properties: &Map<String, Value>, distinct_ids: &[String], id: &str) -> String {
    for key in ["email", "name"] {
        match properties.get(key) {
            Some(Value::String(text)) if !text.trim().is_empty() => return text.clone(),
            Some(Value::Number(number)) => return number.to_string(),
            _ => {}
        }
    }
    distinct_ids
        .first()
        .cloned()
        .unwrap_or_else(|| id.to_owned())
}

fn summary(
    person: PersonRecord,
    distinct_ids: Vec<String>,
    last_seen: Option<String>,
) -> PersonSummary {
    let listed: Vec<String> = distinct_ids.into_iter().take(LISTED_DISTINCT_IDS).collect();
    PersonSummary {
        display_name: display_name(&person.properties, &listed, &person.id),
        id: person.id,
        distinct_ids: listed,
        properties: Value::Object(person.properties),
        is_identified: person.is_identified,
        created_at: person.created_at,
        last_seen,
    }
}

pub fn encode_cursor(cursor: &PersonCursor) -> String {
    let encoded = serde_json::to_vec(&[&cursor.created_at, &cursor.id]).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(encoded)
}

pub fn decode_cursor(text: &str) -> Result<PersonCursor, ExploreError> {
    let invalid = ExploreError::Invalid {
        field: "cursor",
        message: "The cursor is not one this server issued.",
    };
    if text.len() > MAX_CURSOR_LENGTH {
        return Err(invalid);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| ExploreError::Invalid {
            field: "cursor",
            message: "The cursor is not one this server issued.",
        })?;
    match serde_json::from_slice::<[String; 2]>(&bytes) {
        Ok([created_at, id]) => Ok(PersonCursor { created_at, id }),
        Err(_) => Err(invalid),
    }
}

/// `GET /persons`.
pub fn list(
    store: &PersonStore,
    project_id: &str,
    search: Option<&str>,
    cursor: Option<&PersonCursor>,
    limit: usize,
) -> Result<PersonListResponse, ExploreError> {
    let limit = limit.clamp(1, crate::persons::MAX_PERSON_PAGE);
    // One extra row tells whether another page exists.
    let mut entries =
        store.list_persons(project_id, search, cursor, limit + 1, LISTED_DISTINCT_IDS)?;
    let more = entries.len() > limit;
    entries.truncate(limit);
    let next_cursor = more.then(|| entries.last()).flatten().map(|entry| {
        encode_cursor(&PersonCursor {
            created_at: entry.person.created_at.clone(),
            id: entry.person.id.clone(),
        })
    });
    Ok(PersonListResponse {
        persons: entries
            .into_iter()
            .map(|entry| summary(entry.person, entry.distinct_ids, None))
            .collect(),
        next_cursor,
    })
}

/// A person by id, or by any of their distinct ids (a merged-away person id
/// is a distinct id of the survivor).
pub fn find(store: &PersonStore, project_id: &str, id: &str) -> Result<PersonRecord, ExploreError> {
    if let Some(person) = store.person(project_id, id)? {
        return Ok(person);
    }
    store
        .person_for_distinct_id(project_id, id)?
        .ok_or(ExploreError::NotFound)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PersonStats {
    pub event_count: u64,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    pub session_count: u64,
}

/// All-time event stats of a set of distinct ids.
pub fn stats(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    distinct_ids: &[String],
) -> Result<PersonStats, ExploreError> {
    if distinct_ids.is_empty() {
        return Ok(PersonStats::default());
    }
    let files = explorer
        .source()
        .files(project_id, NaiveDate::MIN, NaiveDate::MAX);
    let Some(source) = parquet_source(&files)? else {
        return Ok(PersonStats::default());
    };
    let marks = vec!["?"; distinct_ids.len()].join(", ");
    let sql = format!(
        "WITH e AS MATERIALIZED (
            SELECT epoch_us(timestamp) AS ts, uuid, session_id FROM {source}
            WHERE distinct_id IN ({marks})
        ),
        gaps AS (
            SELECT CASE WHEN ts - lag(ts) OVER (ORDER BY ts, uuid) <= {SESSION_GAP_MICROS}
                        THEN 0 ELSE 1 END AS new_session
            FROM e WHERE session_id IS NULL
        )
        SELECT (SELECT count(*) FROM e),
               (SELECT min(ts) FROM e),
               (SELECT max(ts) FROM e),
               (SELECT count(DISTINCT session_id) FROM e)
                 + (SELECT coalesce(sum(new_session), 0) FROM gaps)"
    );
    let params: Vec<Db> = distinct_ids.iter().cloned().map(Db::Text).collect();
    let (count, first, last, sessions): (i64, Option<i64>, Option<i64>, i64) = connection
        .query_row(&sql, duckdb::params_from_iter(params), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
    drop(files);
    Ok(PersonStats {
        event_count: u64::try_from(count).unwrap_or(0),
        first_seen: first,
        last_seen: last,
        session_count: u64::try_from(sessions).unwrap_or(0),
    })
}

/// `GET /persons/{id}`.
pub fn detail(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    id: &str,
) -> Result<PersonDetail, ExploreError> {
    let person = find(explorer.persons(), project_id, id)?;
    let distinct_ids =
        explorer
            .persons()
            .distinct_ids_of(project_id, &person.id, MAX_DISTINCT_IDS_PER_CALL)?;
    let stats = stats(explorer, connection, project_id, &distinct_ids)?;
    let last_seen = stats.last_seen.map(|micros| rfc3339(from_micros(micros)));
    Ok(PersonDetail {
        person: summary(person, distinct_ids.clone(), last_seen.clone()),
        distinct_ids,
        event_count: stats.event_count,
        first_seen: stats.first_seen.map(|micros| rfc3339(from_micros(micros))),
        last_seen,
        session_count: stats.session_count,
    })
}

/// `GET /persons/{id}/events`.
pub fn person_events(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    id: &str,
    before: Option<DateTime<Utc>>,
    limit: usize,
    now: DateTime<Utc>,
) -> Result<EventListResponse, ExploreError> {
    let person = find(explorer.persons(), project_id, id)?;
    let distinct_ids =
        explorer
            .persons()
            .distinct_ids_of(project_id, &person.id, MAX_DISTINCT_IDS_PER_CALL)?;
    events::feed(
        explorer,
        connection,
        project_id,
        &FeedQuery {
            distinct_ids: Some(distinct_ids),
            before,
            limit,
            ..FeedQuery::default()
        },
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_precedence() {
        let mut properties = Map::new();
        let ids = vec!["anon-1".to_owned()];
        assert_eq!(display_name(&properties, &ids, "p"), "anon-1");
        assert_eq!(display_name(&properties, &[], "p"), "p");
        properties.insert("name".into(), Value::String("Ada".into()));
        assert_eq!(display_name(&properties, &ids, "p"), "Ada");
        properties.insert("email".into(), Value::String("ada@example.com".into()));
        assert_eq!(display_name(&properties, &ids, "p"), "ada@example.com");
    }

    #[test]
    fn cursor_round_trips_and_rejects_garbage() {
        let cursor = PersonCursor {
            created_at: "2026-01-01T00:00:00+00:00".into(),
            id: "a|b\"c".into(),
        };
        assert_eq!(decode_cursor(&encode_cursor(&cursor)).unwrap(), cursor);
        assert!(decode_cursor("!!!").is_err());
        assert!(decode_cursor(&URL_SAFE_NO_PAD.encode(b"[1,2]")).is_err());
    }
}
