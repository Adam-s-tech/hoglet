//! Lifecycle: for each bucket, the persons who are new, returning,
//! resurrecting, or went dormant.
//!
//! A person is *active* in a bucket when they performed the (filtered)
//! event in it. For each bucket in range:
//! - **new**: active, and their first ever matching event is in this bucket
//!   (all history is read);
//! - **returning**: active now and in the previous bucket;
//! - **resurrecting**: active now, not in the previous bucket, not new;
//! - **dormant**: active in the previous bucket but not now (negative).
//!
//! The bucket before the first one is read so the first bucket classifies
//! like every other.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::range::{self, ResolvedRange};
use super::sql::{Params, bucket_expr, event_match};
use super::{Ctx, QueryError, i64_column, string_column};
use crate::contract::insight::{InsightResult, LifecycleQuery, LifecycleStatus};

pub(crate) struct Plan {
    pub range: ResolvedRange,
    pub buckets: Vec<i64>,
}

pub(crate) fn plan(
    q: &LifecycleQuery,
    now: DateTime<Utc>,
    earliest: impl FnOnce() -> Result<Option<i64>, QueryError>,
) -> Result<Plan, QueryError> {
    let range = range::resolve(&q.date_range, q.interval, now, earliest)?;
    let buckets = range.buckets()?;
    Ok(Plan { range, buckets })
}

/// Counts per bucket `[new, returning, resurrecting, dormant]`, plus the
/// persons of one `keep` cell.
fn classify(
    ctx: &Ctx<'_>,
    q: &LifecycleQuery,
    plan: &Plan,
    keep: Option<(LifecycleStatus, usize)>,
) -> Result<(Vec<[i64; 4]>, Vec<String>), QueryError> {
    let buckets = &plan.buckets;
    let index: HashMap<i64, usize> = buckets.iter().enumerate().map(|(i, b)| (*b, i)).collect();
    let previous = range::previous_bucket(buckets[0], plan.range.interval);
    let mut params = Params::new();
    let relation = ctx
        .source
        .relation(&mut params, None, Some(plan.range.to), true)?;
    let pred = format!(
        "{} AND {} AND {}",
        event_match(&q.series, &mut params),
        ctx.source.filters(&q.series.properties, &mut params)?,
        ctx.source.filters(&q.properties, &mut params)?
    );
    let previous_param = params.int(previous);
    let bucket = bucket_expr(plan.range.interval, "ts");
    let partitions = ctx.person_partitions()?;
    let sql = |partition: u64| {
        format!(
            "WITH ev AS ({relation}), \
         m AS (SELECT person_id, ts FROM ev WHERE {pred} AND {}), \
         f AS (SELECT person_id, min(ts) AS first_ts FROM m GROUP BY person_id), \
         a AS (SELECT DISTINCT person_id, {bucket} AS b FROM m WHERE ts >= {previous_param}) \
         SELECT a.person_id, a.b, f.first_ts FROM a JOIN f USING (person_id) \
         ORDER BY a.person_id",
            super::partition_clause(partition, partitions)
        )
    };
    let mut counts = vec![[0_i64; 4]; buckets.len()];
    let mut kept = Vec::new();
    let mut current: Option<String> = None;
    let mut active: HashSet<i64> = HashSet::new();
    let mut first = 0_i64;
    let mut finish = |person: &str, active: &HashSet<i64>, first: i64| {
        let first_bucket = range::trunc(first, plan.range.interval);
        for (k, start) in buckets.iter().enumerate() {
            let now = active.contains(start);
            let before = if k == 0 {
                active.contains(&previous)
            } else {
                active.contains(&buckets[k - 1])
            };
            let status = if now {
                if first_bucket == *start {
                    Some(LifecycleStatus::New)
                } else if before {
                    Some(LifecycleStatus::Returning)
                } else {
                    Some(LifecycleStatus::Resurrecting)
                }
            } else if before {
                Some(LifecycleStatus::Dormant)
            } else {
                None
            };
            if let Some(status) = status {
                let slot = match status {
                    LifecycleStatus::New => 0,
                    LifecycleStatus::Returning => 1,
                    LifecycleStatus::Resurrecting => 2,
                    LifecycleStatus::Dormant => 3,
                };
                counts[k][slot] += 1;
                if keep == Some((status, k)) {
                    kept.push(person.to_owned());
                }
            }
        }
    };
    for partition in 0..partitions {
        ctx.arrow(&sql(partition), &params, |batch| {
            let persons = string_column(batch, 0)?;
            let bucket_values = i64_column(batch, 1)?;
            let firsts = i64_column(batch, 2)?;
            for row in 0..batch.num_rows() {
                let person = persons.value(row);
                if current.as_deref() != Some(person) {
                    if let Some(done) = current.take() {
                        finish(&done, &active, first);
                    }
                    current = Some(person.to_owned());
                    active.clear();
                }
                let bucket = bucket_values.value(row);
                if bucket == previous || index.contains_key(&bucket) {
                    active.insert(bucket);
                }
                first = firsts.value(row);
            }
            Ok(())
        })?;
    }
    if let Some(done) = current.take() {
        finish(&done, &active, first);
    }
    Ok((counts, kept))
}

pub(crate) fn run(
    ctx: &Ctx<'_>,
    q: &LifecycleQuery,
    plan: &Plan,
) -> Result<InsightResult, QueryError> {
    let (counts, _) = classify(ctx, q, plan, None)?;
    Ok(InsightResult::Lifecycle {
        days: plan.buckets.iter().map(|b| range::rfc3339(*b)).collect(),
        labels: plan
            .buckets
            .iter()
            .map(|b| range::label(*b, plan.range.interval))
            .collect(),
        new: counts.iter().map(|c| c[0]).collect(),
        returning: counts.iter().map(|c| c[1]).collect(),
        resurrecting: counts.iter().map(|c| c[2]).collect(),
        dormant: counts.iter().map(|c| -c[3]).collect(),
    })
}

pub(crate) fn actors(
    ctx: &Ctx<'_>,
    q: &LifecycleQuery,
    plan: &Plan,
    status: LifecycleStatus,
    day: &str,
) -> Result<Vec<String>, QueryError> {
    let bucket = range::parse_instant(day)
        .and_then(|day| plan.buckets.iter().position(|b| *b == day))
        .ok_or_else(|| QueryError::invalid("day is not a bucket of this query"))?;
    Ok(classify(ctx, q, plan, Some((status, bucket)))?.1)
}
