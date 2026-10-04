//! Paths: the most common transitions between consecutive nodes.
//!
//! Per person, the events in range (passing the query's filters) in
//! (timestamp, uuid) order become nodes — `$pathname` of `$pageview`s,
//! names of non-`$` events, or both — with consecutive repeats collapsed
//! (a run keeps its first timestamp). With `start_point` the path begins at
//! its first occurrence (persons without it are skipped); with `end_point`
//! it ends at the first occurrence after that (persons who never reach it
//! are skipped). The path is cut to `step_limit` nodes. A link `i_a → i+1_b`
//! counts persons; its time is the mean seconds between the two nodes.

use std::collections::HashMap;

use super::range::ResolvedRange;
use super::sql::Params;
use super::{Ctx, QueryError, i64_column, string_column};
use crate::contract::insight::{InsightResult, PathLink, PathsQuery, PathsType};

/// Collapse, cut and link one person's nodes. `links` maps
/// `(step, source, target)` to `(persons, total µs)`.
pub(crate) fn add_path(
    q: &PathsQuery,
    nodes: &[(String, i64)],
    links: &mut HashMap<(usize, String, String), (u64, i64)>,
) {
    let mut path: Vec<(&str, i64)> = Vec::new();
    for (node, ts) in nodes {
        if path.last().map(|(last, _)| *last) != Some(node.as_str()) {
            path.push((node.as_str(), *ts));
        }
    }
    if let Some(start) = &q.start_point {
        match path.iter().position(|(node, _)| node == start) {
            Some(index) => {
                path.drain(..index);
            }
            None => return,
        }
    }
    if let Some(end) = &q.end_point {
        match path.iter().position(|(node, _)| node == end) {
            Some(index) => path.truncate(index + 1),
            None => return,
        }
    }
    path.truncate(q.step_limit as usize);
    for (step, pair) in path.windows(2).enumerate() {
        let entry = links
            .entry((step, pair[0].0.to_owned(), pair[1].0.to_owned()))
            .or_insert((0, 0));
        entry.0 += 1;
        entry.1 += pair[1].1 - pair[0].1;
    }
}

pub(crate) fn finish(
    q: &PathsQuery,
    links: HashMap<(usize, String, String), (u64, i64)>,
) -> Vec<PathLink> {
    let mut out: Vec<PathLink> = links
        .into_iter()
        .map(|((step, source, target), (value, total))| PathLink {
            source: format!("{}_{source}", step + 1),
            target: format!("{}_{target}", step + 2),
            value,
            average_conversion_time_s: total as f64 / value as f64 / 1e6,
        })
        .collect();
    out.sort_by(|a, b| {
        b.value
            .cmp(&a.value)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.target.cmp(&b.target))
    });
    out.truncate(q.edge_limit as usize);
    out
}

pub(crate) fn run(
    ctx: &Ctx<'_>,
    q: &PathsQuery,
    range: &ResolvedRange,
) -> Result<InsightResult, QueryError> {
    let mut params = Params::new();
    let relation = ctx
        .source
        .relation(&mut params, Some(range.from), Some(range.to), true)?;
    let filters = ctx.source.filters(&q.properties, &mut params)?;
    let node = match q.paths_type {
        PathsType::Pageviews => "CASE WHEN event = '$pageview' THEN pathname END",
        PathsType::CustomEvents => "CASE WHEN NOT starts_with(event, '$') THEN event END",
        PathsType::All => {
            "CASE WHEN event = '$pageview' THEN pathname \
             WHEN NOT starts_with(event, '$') THEN event END"
        }
    };
    let partitions = ctx.person_partitions()?;
    let mut links = HashMap::new();
    let mut current: Option<String> = None;
    let mut nodes: Vec<(String, i64)> = Vec::new();
    for partition in 0..partitions {
        let sql = format!(
            "WITH ev AS ({relation}), \
             x AS (SELECT person_id, ts, uuid, {node} AS node FROM ev \
                   WHERE {filters} AND {}) \
             SELECT person_id, node, ts FROM x WHERE node IS NOT NULL \
             ORDER BY person_id, ts, uuid",
            super::partition_clause(partition, partitions)
        );
        ctx.arrow(&sql, &params, |batch| {
            let persons = string_column(batch, 0)?;
            let names = string_column(batch, 1)?;
            let times = i64_column(batch, 2)?;
            for row in 0..batch.num_rows() {
                let person = persons.value(row);
                if current.as_deref() != Some(person) {
                    add_path(q, &nodes, &mut links);
                    nodes.clear();
                    current = Some(person.to_owned());
                }
                nodes.push((names.value(row).to_owned(), times.value(row)));
            }
            Ok(())
        })?;
        add_path(q, &nodes, &mut links);
        nodes.clear();
        current = None;
    }
    Ok(InsightResult::Paths {
        links: finish(q, links),
    })
}
