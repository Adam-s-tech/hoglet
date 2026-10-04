//! Stickiness: how many persons were active in exactly 1, 2, … intervals
//! of the range (events inside `[from, to)`, counted per bucket).

use super::range::ResolvedRange;
use super::sql::{Params, bucket_expr, event_match};
use super::{Ctx, QueryError};
use crate::contract::common::Interval;
use crate::contract::insight::{EventNode, InsightResult, StickinessQuery, StickinessSeries};

fn per_person(
    ctx: &Ctx<'_>,
    q: &StickinessQuery,
    node: &EventNode,
    range: &ResolvedRange,
    params: &mut Params,
) -> Result<String, QueryError> {
    let relation = ctx
        .source
        .relation(params, Some(range.from), Some(range.to), true)?;
    Ok(format!(
        "WITH ev AS ({relation}) \
         SELECT person_id, count(DISTINCT {}) AS n FROM ev \
         WHERE {} AND {} AND {} GROUP BY person_id",
        bucket_expr(range.interval, "ts"),
        event_match(node, params),
        ctx.source.filters(&node.properties, params)?,
        ctx.source.filters(&q.properties, params)?
    ))
}

fn unit(interval: Interval) -> &'static str {
    match interval {
        Interval::Hour => "hour",
        Interval::Day => "day",
        Interval::Week => "week",
        Interval::Month => "month",
    }
}

pub(crate) fn run(
    ctx: &Ctx<'_>,
    q: &StickinessQuery,
    range: &ResolvedRange,
) -> Result<InsightResult, QueryError> {
    let intervals = range.buckets()?.len();
    let mut series = Vec::with_capacity(q.series.len());
    for (index, node) in q.series.iter().enumerate() {
        ctx.check_deadline()?;
        let mut params = Params::new();
        let inner = per_person(ctx, q, node, range, &mut params)?;
        let sql = format!("SELECT n, count(*) FROM ({inner}) GROUP BY n");
        let mut data = vec![0_u64; intervals];
        for (n, count) in ctx.rows(&sql, &params, |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })? {
            if n >= 1
                && let Some(slot) = data.get_mut(n as usize - 1)
            {
                *slot = count as u64;
            }
        }
        series.push(StickinessSeries {
            label: node.label(),
            series_index: index,
            data,
            labels: (1..=intervals)
                .map(|n| {
                    format!(
                        "{n} {}{}",
                        unit(range.interval),
                        if n == 1 { "" } else { "s" }
                    )
                })
                .collect(),
        });
    }
    Ok(InsightResult::Stickiness { series })
}

pub(crate) fn actors(
    ctx: &Ctx<'_>,
    q: &StickinessQuery,
    range: &ResolvedRange,
    series_index: usize,
    intervals: u32,
) -> Result<Vec<String>, QueryError> {
    let node = q
        .series
        .get(series_index)
        .ok_or_else(|| QueryError::invalid("series_index is out of range"))?;
    let mut params = Params::new();
    let inner = per_person(ctx, q, node, range, &mut params)?;
    let sql = format!(
        "SELECT person_id FROM ({inner}) WHERE n = {} ORDER BY person_id",
        params.int(i64::from(intervals))
    );
    ctx.rows(&sql, &params, |row| row.get::<_, String>(0))
}
