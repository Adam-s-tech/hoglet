//! Trends: one aggregate per series per interval bucket.
//!
//! Definitions (also implemented independently by the oracle):
//! - an event belongs to series *i* when its name matches (or the series is
//!   "all events") and it passes the series' and the query's filters;
//! - `total` counts events; `dau` counts distinct persons in the bucket;
//!   `weekly_active`/`monthly_active` count distinct persons with an event
//!   in the trailing 7/30 days ending at the bucket's end (so they read
//!   events before the range start); `unique_session` counts distinct
//!   non-empty `$session_id`s; property maths aggregate the strictly numeric
//!   values of `math_property` (empty buckets are 0; quantiles interpolate
//!   linearly);
//! - `aggregated_value` is the same math over the whole range (the sum of
//!   the buckets for `total`);
//! - breakdowns keep the top `limit` values by event count in the range
//!   (ties by value), fold the rest into `$$_other`, and put missing or
//!   empty values in `$$_none`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::formula::Formula;
use super::range::{self, DAY_US, ResolvedRange};
use super::sql::{BREAKDOWN_OTHER, Params, breakdown_expr, bucket_expr, event_match, numeric_expr};
use super::{Ctx, QueryError};
use crate::contract::common::Interval;
use crate::contract::insight::{EventNode, InsightResult, Math, TrendSeries, TrendsQuery};

pub(crate) struct Plan {
    pub range: ResolvedRange,
    pub buckets: Vec<i64>,
    pub previous: Option<(ResolvedRange, Vec<i64>)>,
    /// Longest trailing window any series needs, in days.
    window_days: Option<i64>,
}

fn window_days(math: Math) -> Option<i64> {
    match math {
        Math::WeeklyActive => Some(7),
        Math::MonthlyActive => Some(30),
        _ => None,
    }
}

impl Plan {
    pub fn load_from(&self) -> i64 {
        let mut from = self.range.from;
        let mut periods = vec![(self.range, self.buckets.first().copied())];
        if let Some((range, buckets)) = &self.previous {
            periods.push((*range, buckets.first().copied()));
        }
        for (period, first) in periods {
            from = from.min(period.from);
            if let (Some(days), Some(first)) = (self.window_days, first) {
                from = from.min(range::next_bucket(first, period.interval) - days * DAY_US);
            }
        }
        from
    }
}

pub(crate) fn plan(
    q: &TrendsQuery,
    now: DateTime<Utc>,
    earliest: impl FnOnce() -> Result<Option<i64>, QueryError>,
) -> Result<Plan, QueryError> {
    let range = range::resolve(&q.date_range, q.interval, now, earliest)?;
    let buckets = range.buckets()?;
    let previous = if q.compare {
        let previous = range.previous();
        let buckets = previous.buckets()?;
        Some((previous, buckets))
    } else {
        None
    };
    Ok(Plan {
        range,
        buckets,
        previous,
        window_days: q.series.iter().filter_map(|s| window_days(s.math)).max(),
    })
}

/// Top breakdown values of one series.
struct Top {
    values: Vec<String>,
    other: bool,
}

/// `(whole range?, bucket, breakdown, value)` as returned by the aggregate.
type Row = (bool, Option<i64>, Option<String>, Option<f64>);

struct Computed {
    breakdown: Option<String>,
    data: Vec<f64>,
    aggregated: f64,
}

fn predicate(
    ctx: &Ctx<'_>,
    q: &TrendsQuery,
    node: &EventNode,
    params: &mut Params,
) -> Result<String, QueryError> {
    Ok(format!(
        "{} AND {} AND {}",
        event_match(node, params),
        ctx.source.filters(&node.properties, params)?,
        ctx.source.filters(&q.properties, params)?
    ))
}

fn breakdown_text(ctx: &Ctx<'_>, q: &TrendsQuery) -> Result<Option<String>, QueryError> {
    q.breakdown
        .as_ref()
        .map(|breakdown| {
            ctx.source
                .property_text(breakdown.source, &breakdown.property)
                .map(|text| breakdown_expr(&text))
        })
        .transpose()
}

fn needs_person(math: Math) -> bool {
    matches!(math, Math::Dau | Math::WeeklyActive | Math::MonthlyActive)
}

fn top_values(
    ctx: &Ctx<'_>,
    q: &TrendsQuery,
    node: &EventNode,
    period: &ResolvedRange,
) -> Result<Option<Top>, QueryError> {
    let (Some(breakdown), Some(text)) = (&q.breakdown, breakdown_text(ctx, q)?) else {
        return Ok(None);
    };
    let mut params = Params::new();
    let relation = ctx
        .source
        .relation(&mut params, Some(period.from), Some(period.to), false)?;
    let pred = predicate(ctx, q, node, &mut params)?;
    let limit = params.int(i64::from(breakdown.limit) + 1);
    let sql = format!(
        "WITH ev AS ({relation}) \
         SELECT bd, count(*) AS c FROM (SELECT {text} AS bd FROM ev WHERE {pred}) \
         GROUP BY bd ORDER BY c DESC, bd ASC LIMIT {limit}"
    );
    let mut values = ctx.rows(&sql, &params, |row| row.get::<_, String>(0))?;
    let other = values.len() > breakdown.limit as usize;
    values.truncate(breakdown.limit as usize);
    Ok(Some(Top { values, other }))
}

/// Breakdown expression with non-top values folded into `$$_other`.
fn mapped_breakdown(text: &str, top: &Top, params: &mut Params) -> String {
    if top.values.is_empty() {
        return format!("'{BREAKDOWN_OTHER}'");
    }
    let placeholders: Vec<String> = top.values.iter().map(|value| params.text(value)).collect();
    format!(
        "(CASE WHEN {text} IN ({}) THEN {text} ELSE '{BREAKDOWN_OTHER}' END)",
        placeholders.join(", ")
    )
}

fn value_expr(ctx: &Ctx<'_>, node: &EventNode) -> Result<String, QueryError> {
    Ok(match node.math {
        Math::Total | Math::WeeklyActive | Math::MonthlyActive => "NULL".to_owned(),
        Math::Dau => "person_id".to_owned(),
        Math::UniqueSession => "session_id".to_owned(),
        _ => {
            let key = node
                .math_property
                .as_deref()
                .ok_or_else(|| QueryError::invalid("property maths need `math_property`"))?;
            numeric_expr(
                &ctx.source
                    .property_text(crate::contract::common::PropertySource::Event, key)?,
            )
        }
    })
}

fn aggregate(math: Math) -> &'static str {
    match math {
        Math::Total | Math::WeeklyActive | Math::MonthlyActive => "count(*)",
        Math::Dau | Math::UniqueSession => "count(DISTINCT v)",
        Math::Sum => "sum(v)",
        Math::Avg => "avg(v)",
        Math::Min => "min(v)",
        Math::Max => "max(v)",
        Math::Median => "quantile_cont(v, 0.5)",
        Math::P90 => "quantile_cont(v, 0.9)",
        Math::P95 => "quantile_cont(v, 0.95)",
        Math::P99 => "quantile_cont(v, 0.99)",
    }
}

fn granule(interval: Interval) -> Interval {
    match interval {
        Interval::Hour => Interval::Hour,
        _ => Interval::Day,
    }
}

fn compute(
    ctx: &Ctx<'_>,
    q: &TrendsQuery,
    node: &EventNode,
    period: &ResolvedRange,
    buckets: &[i64],
    top: Option<&Top>,
) -> Result<Vec<Computed>, QueryError> {
    let index: HashMap<i64, usize> = buckets.iter().enumerate().map(|(i, b)| (*b, i)).collect();
    let text = breakdown_text(ctx, q)?;
    let mut by_key: HashMap<Option<String>, Computed> = HashMap::new();

    let rows: Vec<Row>;
    let mut aggregated_rows: Vec<(Option<String>, Option<f64>)> = Vec::new();
    if let Some(days) = window_days(node.math) {
        let windows: Vec<Vec<i64>> = buckets
            .iter()
            .map(|bucket| {
                let end = range::next_bucket(*bucket, period.interval);
                vec![*bucket, end - days * DAY_US, end]
            })
            .collect();
        ctx.temp_i64_table("trend_windows", &["b", "ws", "we"], &windows)?;
        let load_from = windows
            .iter()
            .map(|window| window[1])
            .min()
            .unwrap_or(period.from)
            .min(period.from);
        let mut params = Params::new();
        let relation = ctx
            .source
            .relation(&mut params, Some(load_from), Some(period.to), true)?;
        let pred = predicate(ctx, q, node, &mut params)?;
        let bd = match (&text, top) {
            (Some(text), Some(top)) => mapped_breakdown(text, top, &mut params),
            _ => "NULL::VARCHAR".to_owned(),
        };
        // A person active at granule `g` counts for every window whose end
        // `we` lies in (g, g + days]. Each person's coverage is merged into
        // disjoint islands, so counting islands per window counts distinct
        // persons without a DISTINCT over (person × window) rows.
        let span = days * DAY_US;
        let sql = format!(
            "WITH ev AS ({relation}), \
             pairs AS (SELECT DISTINCT person_id, {} AS g, {bd} AS bd FROM ev WHERE {pred}), \
             marked AS (SELECT person_id, bd, g, \
                 CASE WHEN g > max(g) OVER (PARTITION BY person_id, bd ORDER BY g \
                     ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) + {span} \
                 THEN 1 ELSE 0 END AS starts FROM pairs), \
             numbered AS (SELECT person_id, bd, g, sum(starts) OVER (PARTITION BY person_id, bd \
                 ORDER BY g ROWS UNBOUNDED PRECEDING) AS island FROM marked), \
             islands AS (SELECT bd, min(g) AS s, max(g) + {span} AS e \
                 FROM numbered GROUP BY person_id, bd, island) \
             SELECT w.b, i.bd, count(*)::DOUBLE \
             FROM trend_windows w JOIN islands i ON w.we > i.s AND w.we <= i.e \
             GROUP BY w.b, i.bd",
            bucket_expr(granule(period.interval), "ts")
        );
        rows = ctx.rows(&sql, &params, |row| {
            Ok((
                false,
                row.get::<_, Option<i64>>(0)?,
                row.get(1)?,
                row.get(2)?,
            ))
        })?;
        let mut params = Params::new();
        let relation =
            ctx.source
                .relation(&mut params, Some(period.from), Some(period.to), true)?;
        let pred = predicate(ctx, q, node, &mut params)?;
        let bd = match (&text, top) {
            (Some(text), Some(top)) => mapped_breakdown(text, top, &mut params),
            _ => "NULL::VARCHAR".to_owned(),
        };
        let sql = format!(
            "WITH ev AS ({relation}) \
             SELECT bd, count(DISTINCT person_id)::DOUBLE FROM \
             (SELECT person_id, {bd} AS bd FROM ev WHERE {pred}) GROUP BY bd"
        );
        aggregated_rows = ctx.rows(&sql, &params, |row| Ok((row.get(0)?, row.get(1)?)))?;
    } else {
        let mut params = Params::new();
        let relation = ctx.source.relation(
            &mut params,
            Some(period.from),
            Some(period.to),
            needs_person(node.math),
        )?;
        let pred = predicate(ctx, q, node, &mut params)?;
        let bd = match (&text, top) {
            (Some(text), Some(top)) => mapped_breakdown(text, top, &mut params),
            _ => "NULL::VARCHAR".to_owned(),
        };
        let value = value_expr(ctx, node)?;
        let sql = format!(
            "WITH ev AS ({relation}), \
             x AS (SELECT {} AS b, {bd} AS bd, {value} AS v FROM ev WHERE {pred}) \
             SELECT GROUPING(b) = 1 AS whole, b, bd, ({})::DOUBLE AS value FROM x \
             GROUP BY GROUPING SETS ((b, bd), (bd))",
            bucket_expr(period.interval, "ts"),
            aggregate(node.math)
        );
        rows = ctx.rows(&sql, &params, |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
    }

    let ensure = |map: &mut HashMap<Option<String>, Computed>, key: Option<String>| {
        map.entry(key.clone()).or_insert_with(|| Computed {
            breakdown: key,
            data: vec![0.0; buckets.len()],
            aggregated: 0.0,
        });
    };
    for (whole, bucket, key, value) in rows {
        ensure(&mut by_key, key.clone());
        let Some(computed) = by_key.get_mut(&key) else {
            continue;
        };
        let value = value.unwrap_or(0.0);
        if whole {
            computed.aggregated = value;
        } else if let Some(position) = bucket.and_then(|bucket| index.get(&bucket)) {
            computed.data[*position] = value;
        }
    }
    for (key, value) in aggregated_rows {
        ensure(&mut by_key, key.clone());
        if let Some(computed) = by_key.get_mut(&key) {
            computed.aggregated = value.unwrap_or(0.0);
        }
    }

    // Stable output order: no breakdown → one series (zero-filled); with a
    // breakdown → top values by rank, then `$$_other`.
    let mut out = Vec::new();
    match top {
        None => {
            out.push(by_key.remove(&None).unwrap_or(Computed {
                breakdown: None,
                data: vec![0.0; buckets.len()],
                aggregated: 0.0,
            }));
        }
        Some(top) => {
            for value in &top.values {
                let key = Some(value.clone());
                out.push(by_key.remove(&key).unwrap_or(Computed {
                    breakdown: key,
                    data: vec![0.0; buckets.len()],
                    aggregated: 0.0,
                }));
            }
            // `$$_other` appears when the ranking folded values, or when this
            // period has non-zero data outside the top values.
            let other = Some(BREAKDOWN_OTHER.to_owned());
            let computed = by_key.remove(&other).unwrap_or(Computed {
                breakdown: other,
                data: vec![0.0; buckets.len()],
                aggregated: 0.0,
            });
            let nonzero =
                computed.aggregated != 0.0 || computed.data.iter().any(|value| *value != 0.0);
            if top.other || nonzero {
                out.push(computed);
            }
        }
    }
    Ok(out)
}

fn to_series(
    label: String,
    series_index: Option<usize>,
    computed: Computed,
    buckets: &[i64],
    interval: Interval,
    compare: Option<&str>,
) -> TrendSeries {
    TrendSeries {
        label,
        series_index,
        breakdown_value: computed.breakdown,
        compare: compare.map(str::to_owned),
        days: buckets
            .iter()
            .map(|bucket| range::rfc3339(*bucket))
            .collect(),
        labels: buckets
            .iter()
            .map(|bucket| range::label(*bucket, interval))
            .collect(),
        data: computed.data,
        aggregated_value: computed.aggregated,
    }
}

fn period_series(
    ctx: &Ctx<'_>,
    q: &TrendsQuery,
    tops: &[Option<Top>],
    period: &ResolvedRange,
    buckets: &[i64],
    compare: Option<&str>,
) -> Result<Vec<TrendSeries>, QueryError> {
    let mut per_series: Vec<Vec<Computed>> = Vec::with_capacity(q.series.len());
    for (index, node) in q.series.iter().enumerate() {
        ctx.check_deadline()?;
        let top = tops.get(index).and_then(Option::as_ref);
        if q.breakdown.is_some() && top.is_some_and(|top| top.values.is_empty() && !top.other) {
            per_series.push(Vec::new());
            continue;
        }
        per_series.push(compute(ctx, q, node, period, buckets, top)?);
    }
    let mut out = Vec::new();
    match &q.formula {
        None => {
            for (index, computed) in per_series.into_iter().enumerate() {
                for computed in computed {
                    out.push(to_series(
                        q.series[index].label(),
                        Some(index),
                        computed,
                        buckets,
                        period.interval,
                        compare,
                    ));
                }
            }
        }
        Some(text) => {
            let formula = Formula::parse(text, q.series.len())?;
            let mut keys: Vec<Option<String>> = Vec::new();
            for computed in per_series.iter().flatten() {
                if !keys.contains(&computed.breakdown) {
                    keys.push(computed.breakdown.clone());
                }
            }
            for key in keys {
                let lookup: Vec<Option<&Computed>> = per_series
                    .iter()
                    .map(|list| list.iter().find(|c| c.breakdown == key))
                    .collect();
                let data = (0..buckets.len())
                    .map(|bucket| {
                        let values: Vec<f64> = lookup
                            .iter()
                            .map(|c| c.map(|c| c.data[bucket]).unwrap_or(0.0))
                            .collect();
                        formula.evaluate(&values)
                    })
                    .collect();
                let aggregated: Vec<f64> = lookup
                    .iter()
                    .map(|c| c.map(|c| c.aggregated).unwrap_or(0.0))
                    .collect();
                out.push(to_series(
                    text.clone(),
                    None,
                    Computed {
                        breakdown: key,
                        data,
                        aggregated: formula.evaluate(&aggregated),
                    },
                    buckets,
                    period.interval,
                    compare,
                ));
            }
        }
    }
    Ok(out)
}

pub(crate) fn run(
    ctx: &Ctx<'_>,
    q: &TrendsQuery,
    plan: &Plan,
) -> Result<InsightResult, QueryError> {
    let mut tops = Vec::with_capacity(q.series.len());
    for node in &q.series {
        tops.push(top_values(ctx, q, node, &plan.range)?);
    }
    let current = q.compare.then_some("current");
    let mut series = period_series(ctx, q, &tops, &plan.range, &plan.buckets, current)?;
    if let Some((previous, buckets)) = &plan.previous {
        series.extend(period_series(
            ctx,
            q,
            &tops,
            previous,
            buckets,
            Some("previous"),
        )?);
    }
    Ok(InsightResult::Trends { series })
}

pub(crate) fn actors(
    ctx: &Ctx<'_>,
    q: &TrendsQuery,
    plan: &Plan,
    series_index: usize,
    day: &str,
    breakdown_value: Option<&str>,
) -> Result<Vec<String>, QueryError> {
    let node = q
        .series
        .get(series_index)
        .ok_or_else(|| QueryError::invalid("series_index is out of range"))?;
    let bucket = range::parse_instant(day)
        .filter(|bucket| plan.buckets.contains(bucket))
        .ok_or_else(|| QueryError::invalid("day is not a bucket of this query"))?;
    let top = top_values(ctx, q, node, &plan.range)?;
    let text = breakdown_text(ctx, q)?;
    let mut params = Params::new();
    let (from, to) = match window_days(node.math) {
        Some(days) => {
            let end = range::next_bucket(bucket, plan.range.interval);
            (end - days * DAY_US, end.min(plan.range.to))
        }
        None => (plan.range.from, plan.range.to),
    };
    let relation = ctx
        .source
        .relation(&mut params, Some(from), Some(to), true)?;
    let pred = predicate(ctx, q, node, &mut params)?;
    let bd = match (&text, &top) {
        (Some(text), Some(top)) => mapped_breakdown(text, top, &mut params),
        _ => "NULL::VARCHAR".to_owned(),
    };
    let mut conditions = Vec::new();
    if window_days(node.math).is_none() {
        conditions.push(format!(
            "{} = {}",
            bucket_expr(plan.range.interval, "ts"),
            params.int(bucket)
        ));
    }
    match node.math {
        Math::UniqueSession => conditions.push("session_id IS NOT NULL".to_owned()),
        Math::Sum
        | Math::Avg
        | Math::Min
        | Math::Max
        | Math::Median
        | Math::P90
        | Math::P95
        | Math::P99 => conditions.push(format!("{} IS NOT NULL", value_expr(ctx, node)?)),
        _ => {}
    }
    if let (Some(value), true) = (breakdown_value, q.breakdown.is_some()) {
        conditions.push(format!("{bd} = {}", params.text(value)));
    }
    conditions.push(pred);
    let sql = format!(
        "WITH ev AS ({relation}) SELECT DISTINCT person_id FROM ev WHERE {} ORDER BY person_id",
        conditions.join(" AND ")
    );
    ctx.rows(&sql, &params, |row| row.get::<_, String>(0))
}
