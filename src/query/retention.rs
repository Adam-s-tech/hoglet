//! Retention: cohorts of persons by period, and who came back.
//!
//! Cohorts are the `total_intervals` periods ending with the current one.
//! - recurring: a person is in the cohort of every period in which they
//!   performed the target event;
//! - first_time: a person is in the cohort of the period of their first
//!   ever target event (all history is read);
//! - `values[0]` is the cohort size; `values[k]` counts cohort members who
//!   performed the returning event in period `cohort + k`. Periods after
//!   the current one are omitted.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::range::{self, from_datetime};
use super::sql::{Params, bucket_expr, event_match};
use super::{Ctx, QueryError, i64_column, string_column};
use crate::contract::common::Interval;
use crate::contract::insight::{
    EventNode, InsightResult, RetentionCohort, RetentionPeriod, RetentionQuery, RetentionType,
};

pub(crate) struct Plan {
    pub interval: Interval,
    pub starts: Vec<i64>,
    pub end: i64,
    first_time: bool,
}

impl Plan {
    pub fn load_from(&self) -> Option<i64> {
        if self.first_time {
            None
        } else {
            self.starts.first().copied()
        }
    }
}

pub(crate) fn interval(period: RetentionPeriod) -> Interval {
    match period {
        RetentionPeriod::Day => Interval::Day,
        RetentionPeriod::Week => Interval::Week,
        RetentionPeriod::Month => Interval::Month,
    }
}

pub(crate) fn plan(q: &RetentionQuery, now: DateTime<Utc>) -> Result<Plan, QueryError> {
    let interval = interval(q.period);
    let current = range::trunc(from_datetime(now), interval);
    let mut starts = vec![current];
    for _ in 1..q.total_intervals {
        let previous = range::previous_bucket(starts[starts.len() - 1], interval);
        starts.push(previous);
    }
    starts.reverse();
    Ok(Plan {
        interval,
        starts,
        end: range::next_bucket(current, interval),
        first_time: q.retention_type == RetentionType::RetentionFirstTime,
    })
}

struct Activity {
    persons: Vec<String>,
    /// Cohort members per period (person indexes).
    cohorts: Vec<Vec<u32>>,
    /// Persons with a returning event per period.
    returning: Vec<HashSet<u32>>,
}

fn predicate(
    ctx: &Ctx<'_>,
    q: &RetentionQuery,
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

fn pairs(
    ctx: &Ctx<'_>,
    sql: &str,
    params: &Params,
    interned: &mut HashMap<String, u32>,
    persons: &mut Vec<String>,
    mut each: impl FnMut(u32, i64),
) -> Result<(), QueryError> {
    ctx.arrow(sql, params, |batch| {
        let ids = string_column(batch, 0)?;
        let buckets = i64_column(batch, 1)?;
        for row in 0..batch.num_rows() {
            let id = ids.value(row);
            let index = match interned.get(id) {
                Some(index) => *index,
                None => {
                    let index = persons.len() as u32;
                    persons.push(id.to_owned());
                    interned.insert(id.to_owned(), index);
                    index
                }
            };
            each(index, buckets.value(row));
        }
        Ok(())
    })
}

fn activity(ctx: &Ctx<'_>, q: &RetentionQuery, plan: &Plan) -> Result<Activity, QueryError> {
    let index: HashMap<i64, usize> = plan
        .starts
        .iter()
        .enumerate()
        .map(|(i, start)| (*start, i))
        .collect();
    let periods = plan.starts.len();
    let mut interned = HashMap::new();
    let mut persons = Vec::new();
    let mut cohorts: Vec<Vec<u32>> = vec![Vec::new(); periods];
    let mut returning: Vec<HashSet<u32>> = vec![HashSet::new(); periods];
    let bucket = bucket_expr(plan.interval, "ts");
    let start = plan.starts[0];

    let partitions = ctx.person_partitions()?;
    for partition in 0..partitions {
        let part = super::partition_clause(partition, partitions);
        let mut params = Params::new();
        let target_sql = if plan.first_time {
            let relation = ctx
                .source
                .relation(&mut params, None, Some(plan.end), true)?;
            let pred = predicate(ctx, q, &q.target, &mut params)?;
            let start = params.int(start);
            format!(
                "WITH ev AS ({relation}), \
             f AS (SELECT person_id, min(ts) AS first_ts FROM ev WHERE {pred} AND {part} \
                   GROUP BY person_id) \
             SELECT person_id, {} FROM f WHERE first_ts >= {start}",
                bucket_expr(plan.interval, "first_ts")
            )
        } else {
            let relation = ctx
                .source
                .relation(&mut params, Some(start), Some(plan.end), true)?;
            let pred = predicate(ctx, q, &q.target, &mut params)?;
            format!(
                "WITH ev AS ({relation}) SELECT DISTINCT person_id, {bucket} FROM ev \
             WHERE {pred} AND {part}"
            )
        };
        pairs(
            ctx,
            &target_sql,
            &params,
            &mut interned,
            &mut persons,
            |person, bucket| {
                if let Some(period) = index.get(&bucket) {
                    cohorts[*period].push(person);
                }
            },
        )?;

        let mut params = Params::new();
        let relation = ctx
            .source
            .relation(&mut params, Some(start), Some(plan.end), true)?;
        let pred = predicate(ctx, q, &q.returning, &mut params)?;
        let returning_sql = format!(
            "WITH ev AS ({relation}) SELECT DISTINCT person_id, {bucket} FROM ev \
         WHERE {pred} AND {part}"
        );
        pairs(
            ctx,
            &returning_sql,
            &params,
            &mut interned,
            &mut persons,
            |person, bucket| {
                if let Some(period) = index.get(&bucket) {
                    returning[*period].insert(person);
                }
            },
        )?;
    }
    for cohort in &mut cohorts {
        cohort.sort_unstable();
        cohort.dedup();
    }
    Ok(Activity {
        persons,
        cohorts,
        returning,
    })
}

fn cell(activity: &Activity, cohort: usize, offset: usize) -> Vec<u32> {
    let members = &activity.cohorts[cohort];
    if offset == 0 {
        return members.clone();
    }
    let returning = &activity.returning[cohort + offset];
    members
        .iter()
        .copied()
        .filter(|person| returning.contains(person))
        .collect()
}

pub(crate) fn run(
    ctx: &Ctx<'_>,
    q: &RetentionQuery,
    plan: &Plan,
) -> Result<InsightResult, QueryError> {
    let activity = activity(ctx, q, plan)?;
    let periods = plan.starts.len();
    let cohorts = (0..periods)
        .map(|cohort| RetentionCohort {
            date: range::rfc3339(plan.starts[cohort]),
            label: range::label(plan.starts[cohort], plan.interval),
            size: activity.cohorts[cohort].len() as u64,
            values: (0..periods - cohort)
                .map(|offset| cell(&activity, cohort, offset).len() as u64)
                .collect(),
        })
        .collect();
    Ok(InsightResult::Retention {
        period: q.period,
        cohorts,
    })
}

pub(crate) fn actors(
    ctx: &Ctx<'_>,
    q: &RetentionQuery,
    plan: &Plan,
    cohort_date: &str,
    interval: u32,
) -> Result<Vec<String>, QueryError> {
    let cohort = range::parse_instant(cohort_date)
        .and_then(|start| plan.starts.iter().position(|s| *s == start))
        .ok_or_else(|| QueryError::invalid("cohort_date is not a cohort of this query"))?;
    let offset = interval as usize;
    if cohort + offset >= plan.starts.len() {
        return Err(QueryError::invalid(
            "interval is out of range for this cohort",
        ));
    }
    let activity = activity(ctx, q, plan)?;
    Ok(cell(&activity, cohort, offset)
        .into_iter()
        .map(|person| activity.persons[person as usize].clone())
        .collect())
}
