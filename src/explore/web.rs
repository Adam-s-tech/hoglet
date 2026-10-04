//! Web analytics over `$pageview`s (see `contract::web` for definitions).
//!
//! Precise semantics, all in UTC:
//! - Events are filtered first (event-property filters), then resolved to
//!   persons: `person = coalesce(identity override, distinct_id)`.
//! - A session is a `$session_id`; events without one are sessionized per
//!   person, a new session starting after more than 30 minutes of
//!   inactivity. Only sessions containing at least one `$pageview` count.
//! - visitors: distinct persons with a pageview. pageviews: `$pageview`s.
//! - bounce rate: PostHog's definition — sessions with one pageview, no
//!   `$autocapture`, lasting under 10 seconds — over all sessions.
//! - session duration: mean of (last event − first event) per session.
//! - `previous`: the same metric over the preceding period of equal length,
//!   sessionized independently.
//! - Breakdowns: `page` counts pageviews per pathname with the bounce rate of
//!   sessions entering on it; `entry_page`/`exit_page` count sessions by
//!   their first/last pageview's pathname; every other dimension attributes
//!   each session to its first pageview's value (so `referring_domain` is the
//!   session's external referrer, not an internal navigation) and counts the
//!   session's pageviews as `views`.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use duckdb::types::Value as Db;

use crate::contract::common::{BREAKDOWN_NONE, Interval};
use crate::contract::web::{
    WebBreakdown, WebBreakdownRow, WebDimension, WebMetric, WebOverview, WebQuery,
};

use super::dates::{self, day_span};
use super::{ExploreError, Explorer, filters, from_micros, parquet_source, rfc3339};

/// Gap that ends a sessionless visit.
pub const SESSION_GAP_MICROS: i64 = 30 * 60 * 1_000_000;
/// Window of `live_visitors`.
pub const LIVE_WINDOW: Duration = Duration::minutes(5);
/// Largest breakdown.
pub const MAX_BREAKDOWN_LIMIT: usize = 100;
pub const DEFAULT_BREAKDOWN_LIMIT: usize = 10;

/// The promoted column a dimension reads (never JSON).
fn dimension_expression(dimension: WebDimension) -> &'static str {
    match dimension {
        WebDimension::Page | WebDimension::EntryPage | WebDimension::ExitPage => "NULL::VARCHAR",
        WebDimension::ReferringDomain => "coalesce(nullif(e.referring_domain, ''), '$direct')",
        WebDimension::UtmSource => "e.utm_source",
        WebDimension::UtmMedium => "e.utm_medium",
        WebDimension::UtmCampaign => "e.utm_campaign",
        WebDimension::Browser => "e.browser",
        WebDimension::Os => "e.os",
        WebDimension::DeviceType => "e.device_type",
        WebDimension::Country => "e.country",
    }
}

/// The CTE chain shared by overview and breakdown: `tagged` (events with
/// person, period, session id) and `sessions` (pageview sessions).
///
/// Parameters, in order: project id, range start µs, range end µs, filter
/// parameters, current-period start µs.
fn session_ctes(source: &str, dimension: &str, filter_sql: &str) -> String {
    format!(
        "WITH ev AS (
            SELECT epoch_us(e.timestamp) AS ts, e.uuid, e.event, e.session_id, e.pathname,
                   {dimension} AS dim,
                   coalesce(o.person_id, e.distinct_id) AS person
            FROM {source} e
            LEFT JOIN (SELECT distinct_id, person_id FROM explore_overrides
                       WHERE project_id = ?) o
              ON o.distinct_id = e.distinct_id
            WHERE e.timestamp >= make_timestamptz(?) AND e.timestamp < make_timestamptz(?)
                  {filter_sql}
        ),
        p AS (SELECT *, CASE WHEN ts >= ? THEN 1 ELSE 0 END AS period FROM ev),
        gaps AS (
            SELECT *,
                   CASE WHEN ts - lag(ts) OVER (PARTITION BY period, person ORDER BY ts, uuid)
                             <= {SESSION_GAP_MICROS}
                        THEN 0 ELSE 1 END AS new_session
            FROM p WHERE session_id IS NULL
        ),
        tagged AS MATERIALIZED (
            SELECT period, ts, uuid, event, pathname, dim, person, 's' || session_id AS sid
            FROM p WHERE session_id IS NOT NULL
            UNION ALL
            SELECT period, ts, uuid, event, pathname, dim, person,
                   'a' || person || chr(31) || CAST(sum(new_session) OVER (
                       PARTITION BY period, person ORDER BY ts, uuid
                       ROWS UNBOUNDED PRECEDING) AS VARCHAR) AS sid
            FROM gaps
        ),
        sessions AS (
            SELECT period, sid,
                   count(*) FILTER (WHERE event = '$pageview') AS pageviews,
                   count(*) AS events,
                   count(*) FILTER (WHERE event = '$autocapture') AS autocaptures,
                   max(ts) - min(ts) AS duration,
                   arg_min(pathname, (ts, uuid)) FILTER (WHERE event = '$pageview') AS entry_page,
                   arg_max(pathname, (ts, uuid)) FILTER (WHERE event = '$pageview') AS exit_page,
                   arg_min(dim, (ts, uuid)) FILTER (WHERE event = '$pageview') AS entry_dim,
                   arg_min(person, (ts, uuid)) FILTER (WHERE event = '$pageview') AS person
            FROM tagged
            GROUP BY period, sid
            HAVING count(*) FILTER (WHERE event = '$pageview') > 0
        )"
    )
}

/// Resolved request: range, previous-period start, filters.
struct Plan {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    /// Start of the previous period, when one exists.
    previous_from: Option<DateTime<Utc>>,
    filters: filters::Compiled,
}

fn plan(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    query: &WebQuery,
    now: DateTime<Utc>,
    with_previous: bool,
) -> Result<Plan, ExploreError> {
    let range = dates::resolve(&query.date_from, query.date_to.as_deref(), now)?;
    let compiled = filters::compile(&query.properties, "e", now)?;
    filters::validate_regexes(connection, &compiled)?;
    let (from, previous_from) = match range.from {
        Some(from) => {
            let previous = with_previous
                .then(|| from.checked_sub_signed(range.to - from))
                .flatten();
            (from, previous)
        }
        None => (
            oldest_event(explorer, connection, project_id, range.to)?
                .unwrap_or_else(|| dates::truncate(range.to, Interval::Day)),
            None,
        ),
    };
    if from >= range.to {
        return Err(ExploreError::Invalid {
            field: "date_from",
            message: "date_from must be before date_to.",
        });
    }
    Ok(Plan {
        from,
        to: range.to,
        previous_from,
        filters: compiled,
    })
}

fn oldest_event(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    to: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ExploreError> {
    let (_, to_day) = day_span(to - Duration::microseconds(1), to);
    let files = explorer.source().files(project_id, NaiveDate::MIN, to_day);
    let Some(source) = parquet_source(&files)? else {
        return Ok(None);
    };
    let oldest: Option<i64> = connection.query_row(
        &format!("SELECT min(epoch_us(timestamp)) FROM {source}"),
        [],
        |row| row.get(0),
    )?;
    Ok(oldest.map(from_micros))
}

fn session_params(project_id: &str, lo: DateTime<Utc>, plan: &Plan) -> Vec<Db> {
    let mut params = vec![
        Db::Text(project_id.to_owned()),
        Db::BigInt(lo.timestamp_micros()),
        Db::BigInt(plan.to.timestamp_micros()),
    ];
    params.extend(plan.filters.params.iter().cloned());
    params.push(Db::BigInt(plan.from.timestamp_micros()));
    params
}

fn metric(value: f64, previous: Option<f64>) -> WebMetric {
    WebMetric { value, previous }
}

#[derive(Debug, Default, Clone, Copy)]
struct PeriodStats {
    visitors: f64,
    pageviews: f64,
    sessions: f64,
    bounces: f64,
    duration_us: f64,
}

impl PeriodStats {
    fn bounce_rate(&self) -> f64 {
        if self.sessions > 0.0 {
            100.0 * self.bounces / self.sessions
        } else {
            0.0
        }
    }
}

pub fn overview(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    query: &WebQuery,
    now: DateTime<Utc>,
) -> Result<WebOverview, ExploreError> {
    let plan = plan(explorer, connection, project_id, query, now, true)?;
    let (interval, buckets) = dates::choose_interval(query.interval, plan.from, plan.to)?;
    let lo = plan.previous_from.unwrap_or(plan.from);
    let (first_day, last_day) = day_span(lo, plan.to);

    let mut periods = [PeriodStats::default(); 2];
    let mut visitors_series = vec![0_u64; buckets.len()];
    let mut pageviews_series = vec![0_u64; buckets.len()];
    let files = explorer.source().files(project_id, first_day, last_day);
    if let Some(source) = parquet_source(&files)? {
        explorer.sync_overrides(connection)?;
        let unit = dates::unit(interval);
        let sql = format!(
            "{ctes},
            page_stats AS (
                SELECT period, count(DISTINCT person) AS visitors, count(*) AS pageviews
                FROM tagged WHERE event = '$pageview' GROUP BY period
            ),
            session_stats AS (
                SELECT period, count(*) AS sessions,
                       count(*) FILTER (WHERE pageviews = 1 AND autocaptures = 0 AND duration < 10000000) AS bounces,
                       avg(duration) AS duration
                FROM sessions GROUP BY period
            ),
            series AS (
                SELECT epoch_us(date_trunc('{unit}', make_timestamp(ts))) AS bucket,
                       count(DISTINCT person) AS visitors, count(*) AS pageviews
                FROM tagged WHERE period = 1 AND event = '$pageview' GROUP BY bucket
            )
            SELECT 'page', period, visitors::DOUBLE, pageviews::DOUBLE, NULL::DOUBLE
              FROM page_stats
            UNION ALL
            SELECT 'session', period, sessions::DOUBLE, bounces::DOUBLE, duration::DOUBLE
              FROM session_stats
            UNION ALL
            SELECT 'series', bucket, visitors::DOUBLE, pageviews::DOUBLE, NULL::DOUBLE
              FROM series",
            ctes = session_ctes(&source, "NULL::VARCHAR", &plan.filters.sql),
        );
        let bucket_index: std::collections::HashMap<i64, usize> = buckets
            .iter()
            .enumerate()
            .map(|(index, at)| (at.timestamp_micros(), index))
            .collect();
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(duckdb::params_from_iter(session_params(
            project_id, lo, &plan,
        )))?;
        while let Some(row) = rows.next()? {
            let kind: String = row.get(0)?;
            let key: i64 = row.get(1)?;
            let a: f64 = row.get(2)?;
            let b: f64 = row.get(3)?;
            let c: Option<f64> = row.get(4)?;
            match kind.as_str() {
                "page" | "session" => {
                    let Some(stats) = periods.get_mut(usize::try_from(key).unwrap_or(2)) else {
                        continue;
                    };
                    if kind == "page" {
                        stats.visitors = a;
                        stats.pageviews = b;
                    } else {
                        stats.sessions = a;
                        stats.bounces = b;
                        stats.duration_us = c.unwrap_or(0.0);
                    }
                }
                _ => {
                    if let Some(&index) = bucket_index.get(&key) {
                        visitors_series[index] = a as u64;
                        pageviews_series[index] = b as u64;
                    }
                }
            }
        }
    }
    drop(files);

    let live_visitors = live_visitors(explorer, connection, project_id, &plan.filters, now)?;
    let [previous, current] = periods;
    let previous = plan.previous_from.map(|_| previous);
    Ok(WebOverview {
        visitors: metric(current.visitors, previous.map(|p| p.visitors)),
        pageviews: metric(current.pageviews, previous.map(|p| p.pageviews)),
        sessions: metric(current.sessions, previous.map(|p| p.sessions)),
        bounce_rate: metric(current.bounce_rate(), previous.map(|p| p.bounce_rate())),
        session_duration_s: metric(
            current.duration_us / 1e6,
            previous.map(|p| p.duration_us / 1e6),
        ),
        interval,
        days: buckets.into_iter().map(rfc3339).collect(),
        visitors_series,
        pageviews_series,
        live_visitors,
    })
}

fn live_visitors(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    compiled: &filters::Compiled,
    now: DateTime<Utc>,
) -> Result<u64, ExploreError> {
    let since = now - LIVE_WINDOW;
    let first = since.date_naive();
    let last = now
        .date_naive()
        .checked_add_signed(Duration::days(2))
        .unwrap_or(first);
    let files = explorer.source().files(project_id, first, last);
    let Some(source) = parquet_source(&files)? else {
        return Ok(0);
    };
    explorer.sync_overrides(connection)?;
    let sql = format!(
        "SELECT count(DISTINCT coalesce(o.person_id, e.distinct_id))
         FROM {source} e
         LEFT JOIN (SELECT distinct_id, person_id FROM explore_overrides
                    WHERE project_id = ?) o
           ON o.distinct_id = e.distinct_id
         WHERE e.event = '$pageview' AND e.timestamp >= make_timestamptz(?)
               AND e.timestamp <= make_timestamptz(?) {filters}",
        filters = compiled.sql
    );
    let mut params = vec![
        Db::Text(project_id.to_owned()),
        Db::BigInt(since.timestamp_micros()),
        Db::BigInt(now.timestamp_micros()),
    ];
    params.extend(compiled.params.iter().cloned());
    let count: i64 =
        connection.query_row(&sql, duckdb::params_from_iter(params), |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or(0))
}

pub fn breakdown(
    explorer: &Explorer,
    connection: &duckdb::Connection,
    project_id: &str,
    query: &WebQuery,
    dimension: WebDimension,
    limit: usize,
    now: DateTime<Utc>,
) -> Result<WebBreakdown, ExploreError> {
    let limit = limit.clamp(1, MAX_BREAKDOWN_LIMIT);
    let plan = plan(explorer, connection, project_id, query, now, false)?;
    let (first_day, last_day) = day_span(plan.from, plan.to);
    let files = explorer.source().files(project_id, first_day, last_day);
    let Some(source) = parquet_source(&files)? else {
        return Ok(WebBreakdown {
            dimension,
            rows: Vec::new(),
        });
    };
    explorer.sync_overrides(connection)?;
    let bounce = "100.0 * count(*) FILTER (WHERE pageviews = 1 AND autocaptures = 0 AND duration < 10000000) / count(*)";
    let select = match dimension {
        WebDimension::Page => format!(
            "views AS (
                SELECT coalesce(pathname, '{BREAKDOWN_NONE}') AS value,
                       count(DISTINCT person) AS visitors, count(*) AS views
                FROM tagged WHERE event = '$pageview' GROUP BY 1
            ),
            bounce AS (
                SELECT coalesce(entry_page, '{BREAKDOWN_NONE}') AS value, {bounce} AS bounce_rate
                FROM sessions GROUP BY 1
            )
            SELECT v.value, v.visitors, v.views, b.bounce_rate
            FROM views v LEFT JOIN bounce b ON b.value = v.value"
        ),
        WebDimension::EntryPage | WebDimension::ExitPage => {
            let column = if dimension == WebDimension::EntryPage {
                "entry_page"
            } else {
                "exit_page"
            };
            format!(
                "rows AS (
                    SELECT coalesce({column}, '{BREAKDOWN_NONE}') AS value,
                           count(DISTINCT person) AS visitors, count(*) AS views,
                           {bounce} AS bounce_rate
                    FROM sessions GROUP BY 1
                )
                SELECT value, visitors, views, bounce_rate FROM rows"
            )
        }
        _ => format!(
            "rows AS (
                SELECT coalesce(entry_dim, '{BREAKDOWN_NONE}') AS value,
                       count(DISTINCT person) AS visitors, sum(pageviews) AS views,
                       NULL::DOUBLE AS bounce_rate
                FROM sessions GROUP BY 1
            )
            SELECT value, visitors, views, bounce_rate FROM rows"
        ),
    };
    let sql = format!(
        "{ctes}, {select} ORDER BY 2 DESC, 3 DESC, 1 LIMIT ?",
        ctes = session_ctes(&source, dimension_expression(dimension), &plan.filters.sql),
    );
    let mut params = session_params(project_id, plan.from, &plan);
    params.push(Db::BigInt(limit as i64));
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(duckdb::params_from_iter(params), |row| {
            let visitors: i64 = row.get(1)?;
            let views: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            Ok(WebBreakdownRow {
                value: row.get(0)?,
                visitors: u64::try_from(visitors).unwrap_or(0),
                views: u64::try_from(views).unwrap_or(0),
                bounce_rate: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WebBreakdown { dimension, rows })
}
