//! The event feed: newest first, keyset-paged by timestamp.
//!
//! The feed never scans the whole lake when recent events exist. It reads
//! day windows walking back from `before` (or from now): first the newest
//! day or two, then windows doubling in length, and stops as soon as it has
//! a full page or the source has no older files.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use duckdb::types::Value as Db;
use serde_json::{Map, Value};

use crate::contract::common::PropertyFilter;
use crate::contract::persons::{EventListResponse, EventRow};

use super::{ExploreError, Explorer, filters, from_micros, parquet_source, rfc3339};

/// Largest page.
pub const MAX_FEED_LIMIT: usize = 200;
/// Page size when the client does not ask.
pub const DEFAULT_FEED_LIMIT: usize = 100;
/// Most day windows one request walks (doubling: far beyond any history).
const MAX_WINDOWS: usize = 24;
/// Longest event-name filter.
pub const MAX_EVENT_NAME: usize = 400;

#[derive(Debug, Clone, Default)]
pub struct FeedQuery {
    pub event: Option<String>,
    /// Restrict to these distinct ids (a person's ids). `Some(empty)` matches
    /// nothing.
    pub distinct_ids: Option<Vec<String>>,
    /// Exclusive upper bound.
    pub before: Option<DateTime<Utc>>,
    pub limit: usize,
    pub filters: Vec<PropertyFilter>,
}

#[derive(Debug, Clone)]
struct RawEvent {
    uuid: String,
    event: String,
    distinct_id: String,
    micros: i64,
    properties: String,
}

/// Day windows `[lo, hi)`, newest first.
struct Windows {
    hi: NaiveDate,
    span: i64,
    issued: usize,
}

impl Windows {
    fn new(before: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Self {
        match before {
            Some(before) => {
                let last = (before - Duration::microseconds(1)).date_naive();
                Self {
                    hi: last.succ_opt().unwrap_or(last),
                    span: 1,
                    issued: 0,
                }
            }
            // Today and tomorrow: clients with fast clocks land in the future.
            None => {
                let today = now.date_naive();
                Self {
                    hi: today.checked_add_signed(Duration::days(2)).unwrap_or(today),
                    span: 2,
                    issued: 0,
                }
            }
        }
    }

    fn next(&mut self) -> Option<(NaiveDate, NaiveDate)> {
        if self.issued == MAX_WINDOWS || self.hi == NaiveDate::MIN {
            return None;
        }
        let lo = self
            .hi
            .checked_sub_signed(Duration::days(self.span))
            .unwrap_or(NaiveDate::MIN);
        let window = (lo, self.hi);
        self.hi = lo;
        self.span = self.span.saturating_mul(2);
        self.issued += 1;
        Some(window)
    }
}

pub fn feed(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    query: &FeedQuery,
    now: DateTime<Utc>,
) -> Result<EventListResponse, ExploreError> {
    let limit = query.limit.clamp(1, MAX_FEED_LIMIT);
    if query
        .distinct_ids
        .as_ref()
        .is_some_and(|ids| ids.is_empty())
    {
        return Ok(EventListResponse {
            events: Vec::new(),
            next_before: None,
        });
    }
    let compiled = filters::compile(&query.filters, "e", now)?;
    filters::validate_regexes(connection, &compiled)?;

    let mut collected: Vec<RawEvent> = Vec::with_capacity(limit + 1);
    let mut windows = Windows::new(query.before, now);
    while let Some((lo, hi)) = windows.next() {
        let files = explorer.source().files(project_id, lo, hi);
        if let Some(source) = parquet_source(&files)? {
            let wanted = limit + 1 - collected.len();
            collected.extend(read_window(connection, &source, query, &compiled, wanted)?);
            drop(files);
            if collected.len() > limit {
                break;
            }
        }
        if explorer
            .source()
            .files(project_id, NaiveDate::MIN, lo)
            .is_empty()
        {
            break;
        }
    }

    let (page, next_before) = cut_page(collected, limit);
    let ids: Vec<String> = {
        let mut ids: Vec<String> = page.iter().map(|row| row.distinct_id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let resolved = explorer.persons().resolve_distinct_ids(project_id, &ids)?;
    let events = page
        .into_iter()
        .map(|row| {
            let person_id = resolved
                .get(&row.distinct_id)
                .cloned()
                .unwrap_or_else(|| row.distinct_id.clone());
            EventRow {
                uuid: row.uuid,
                event: row.event,
                distinct_id: row.distinct_id,
                person_id,
                timestamp: rfc3339(from_micros(row.micros)),
                properties: decode_properties(&row.properties),
            }
        })
        .collect();
    Ok(EventListResponse {
        events,
        next_before: next_before.map(rfc3339),
    })
}

fn decode_properties(encoded: &str) -> Value {
    match serde_json::from_str::<Value>(encoded) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => Value::Object(Map::new()),
    }
}

fn read_window(
    connection: &duckdb::Connection,
    source: &str,
    query: &FeedQuery,
    compiled: &filters::Compiled,
    wanted: usize,
) -> Result<Vec<RawEvent>, ExploreError> {
    let mut sql = format!(
        "SELECT e.uuid, e.event, e.distinct_id, epoch_us(e.timestamp), e.properties
         FROM {source} e WHERE true"
    );
    let mut params: Vec<Db> = Vec::new();
    if let Some(before) = query.before {
        sql.push_str(" AND e.timestamp < make_timestamptz(?)");
        params.push(Db::BigInt(before.timestamp_micros()));
    }
    if let Some(event) = &query.event {
        sql.push_str(" AND e.event = ?");
        params.push(Db::Text(event.clone()));
    }
    if let Some(ids) = &query.distinct_ids {
        let marks = vec!["?"; ids.len()].join(", ");
        sql.push_str(&format!(" AND e.distinct_id IN ({marks})"));
        params.extend(ids.iter().cloned().map(Db::Text));
    }
    sql.push_str(&compiled.sql);
    params.extend(compiled.params.iter().cloned());
    sql.push_str(" ORDER BY e.timestamp DESC, e.uuid DESC LIMIT ?");
    params.push(Db::BigInt(wanted as i64));

    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(duckdb::params_from_iter(params), |row| {
            Ok(RawEvent {
                uuid: row.get(0)?,
                event: row.get(1)?,
                distinct_id: row.get(2)?,
                micros: row.get(3)?,
                properties: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Split `limit + 1` (or fewer) newest-first rows into a page and the
/// exclusive `before` of the next page.
///
/// Rows sharing the page's last timestamp with the first row of the next
/// page are deferred to the next page, so a strict `timestamp < before`
/// cursor never skips them. Only if an entire page shares one microsecond
/// does the cursor move past it (dropping the rest of that microsecond).
fn cut_page(mut rows: Vec<RawEvent>, limit: usize) -> (Vec<RawEvent>, Option<DateTime<Utc>>) {
    if rows.len() <= limit {
        return (rows, None);
    }
    rows.truncate(limit + 1);
    let boundary = rows[limit].micros;
    rows.truncate(limit);
    let last = rows[limit - 1].micros;
    if last != boundary {
        return (rows, Some(from_micros(last)));
    }
    let keep = rows.iter().take_while(|row| row.micros != boundary).count();
    if keep == 0 {
        return (rows, Some(from_micros(boundary)));
    }
    rows.truncate(keep);
    (rows, Some(from_micros(boundary + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(micros: i64) -> RawEvent {
        RawEvent {
            uuid: micros.to_string(),
            event: "e".into(),
            distinct_id: "d".into(),
            micros,
            properties: "{}".into(),
        }
    }

    #[test]
    fn page_cut_defers_ties() {
        let (page, next) = cut_page(vec![raw(9), raw(8), raw(7), raw(7)], 3);
        assert_eq!(page.len(), 2);
        assert_eq!(next.unwrap().timestamp_micros(), 8);
        let (page, next) = cut_page(vec![raw(9), raw(8), raw(7), raw(6)], 3);
        assert_eq!(page.len(), 3);
        assert_eq!(next.unwrap().timestamp_micros(), 7);
        let (page, next) = cut_page(vec![raw(9), raw(8)], 3);
        assert_eq!(page.len(), 2);
        assert!(next.is_none());
    }

    #[test]
    fn windows_widen_from_before() {
        let before = DateTime::parse_from_rfc3339("2026-03-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut windows = Windows::new(Some(before), before);
        let day = |text: &str| NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
        assert_eq!(windows.next(), Some((day("2026-03-09"), day("2026-03-10"))));
        assert_eq!(windows.next(), Some((day("2026-03-07"), day("2026-03-09"))));
        assert_eq!(windows.next(), Some((day("2026-03-03"), day("2026-03-07"))));
    }
}
