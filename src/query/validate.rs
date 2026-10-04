//! Explicit limits on everything a request controls. Nothing past here
//! allocates proportionally to unchecked request input.

use super::QueryError;
use super::sql::{MAX_EVENT_NAME_BYTES, validate_key};
use crate::contract::common::{Breakdown, PropertyFilter};
use crate::contract::insight::{ActorSelection, EventNode, FunnelOrder, InsightQuery, Math};

pub const MAX_SERIES: usize = 20;
pub const MAX_FUNNEL_STEPS: usize = 20;
pub const MAX_EXCLUSIONS: usize = 10;
pub const MAX_FILTERS_PER_LIST: usize = 50;
pub const MAX_BREAKDOWN_LIMIT: u32 = 300;
pub const MAX_FORMULA_BYTES: usize = 1_000;
pub const MAX_RETENTION_INTERVALS: u32 = 100;
pub const MAX_PATH_STEPS: u32 = 20;
pub const MAX_PATH_EDGES: u32 = 1_000;
pub const MAX_SQL_BYTES: usize = 20_000;
/// Ten years.
pub const MAX_FUNNEL_WINDOW_SECONDS: i64 = 10 * 366 * 86_400;
pub const MAX_LABEL_BYTES: usize = 400;

fn filters(list: &[PropertyFilter]) -> Result<(), QueryError> {
    if list.len() > MAX_FILTERS_PER_LIST {
        return Err(QueryError::invalid(format!(
            "at most {MAX_FILTERS_PER_LIST} property filters per list"
        )));
    }
    for filter in list {
        validate_key(&filter.key)?;
    }
    Ok(())
}

fn node(node: &EventNode) -> Result<(), QueryError> {
    if let Some(event) = &node.event
        && event.len() > MAX_EVENT_NAME_BYTES
    {
        return Err(QueryError::invalid(format!(
            "event names are limited to {MAX_EVENT_NAME_BYTES} bytes"
        )));
    }
    if node
        .custom_name
        .as_ref()
        .is_some_and(|name| name.len() > MAX_LABEL_BYTES)
    {
        return Err(QueryError::invalid("custom_name is too long"));
    }
    if let Some(property) = &node.math_property {
        validate_key(property)?;
    }
    filters(&node.properties)
}

fn breakdown(breakdown: &Option<Breakdown>) -> Result<(), QueryError> {
    if let Some(breakdown) = breakdown {
        validate_key(&breakdown.property)?;
        if breakdown.limit == 0 || breakdown.limit > MAX_BREAKDOWN_LIMIT {
            return Err(QueryError::invalid(format!(
                "breakdown limit must be between 1 and {MAX_BREAKDOWN_LIMIT}"
            )));
        }
    }
    Ok(())
}

fn series(list: &[EventNode], max: usize, what: &str) -> Result<(), QueryError> {
    if list.is_empty() || list.len() > max {
        return Err(QueryError::invalid(format!(
            "{what} needs 1 to {max} entries"
        )));
    }
    list.iter().try_for_each(node)
}

pub fn query(query: &InsightQuery) -> Result<(), QueryError> {
    match query {
        InsightQuery::TrendsQuery(q) => {
            series(&q.series, MAX_SERIES, "series")?;
            filters(&q.properties)?;
            breakdown(&q.breakdown)?;
            for node in &q.series {
                let property_math = matches!(
                    node.math,
                    Math::Sum
                        | Math::Avg
                        | Math::Min
                        | Math::Max
                        | Math::Median
                        | Math::P90
                        | Math::P95
                        | Math::P99
                );
                if property_math && node.math_property.is_none() {
                    return Err(QueryError::invalid("property maths need `math_property`"));
                }
            }
            if let Some(formula) = &q.formula {
                if formula.len() > MAX_FORMULA_BYTES {
                    return Err(QueryError::invalid("formula is too long"));
                }
                super::formula::Formula::parse(formula, q.series.len())?;
            }
        }
        InsightQuery::FunnelsQuery(q) => {
            series(&q.series, MAX_FUNNEL_STEPS, "funnel steps")?;
            filters(&q.properties)?;
            breakdown(&q.breakdown)?;
            if q.funnel_window.interval == 0
                || q.funnel_window.seconds() > MAX_FUNNEL_WINDOW_SECONDS
            {
                return Err(QueryError::invalid(
                    "funnel_window must be positive and at most ten years",
                ));
            }
            if q.exclusions.len() > MAX_EXCLUSIONS {
                return Err(QueryError::invalid(format!(
                    "at most {MAX_EXCLUSIONS} funnel exclusions"
                )));
            }
            if !q.exclusions.is_empty() && q.funnel_order == FunnelOrder::Unordered {
                return Err(QueryError::invalid(
                    "exclusions are not supported for unordered funnels",
                ));
            }
            for exclusion in &q.exclusions {
                if exclusion.event.len() > MAX_EVENT_NAME_BYTES {
                    return Err(QueryError::invalid("exclusion event name is too long"));
                }
                if exclusion.from_step >= exclusion.to_step || exclusion.to_step >= q.series.len() {
                    return Err(QueryError::invalid(
                        "exclusions need from_step < to_step < number of steps",
                    ));
                }
            }
        }
        InsightQuery::RetentionQuery(q) => {
            node(&q.target)?;
            node(&q.returning)?;
            filters(&q.properties)?;
            if q.total_intervals == 0 || q.total_intervals > MAX_RETENTION_INTERVALS {
                return Err(QueryError::invalid(format!(
                    "total_intervals must be between 1 and {MAX_RETENTION_INTERVALS}"
                )));
            }
        }
        InsightQuery::LifecycleQuery(q) => {
            node(&q.series)?;
            filters(&q.properties)?;
        }
        InsightQuery::StickinessQuery(q) => {
            series(&q.series, MAX_SERIES, "series")?;
            filters(&q.properties)?;
        }
        InsightQuery::PathsQuery(q) => {
            filters(&q.properties)?;
            if !(2..=MAX_PATH_STEPS).contains(&q.step_limit) {
                return Err(QueryError::invalid(format!(
                    "step_limit must be between 2 and {MAX_PATH_STEPS}"
                )));
            }
            if !(1..=MAX_PATH_EDGES).contains(&q.edge_limit) {
                return Err(QueryError::invalid(format!(
                    "edge_limit must be between 1 and {MAX_PATH_EDGES}"
                )));
            }
            for point in [&q.start_point, &q.end_point].into_iter().flatten() {
                if point.len() > MAX_EVENT_NAME_BYTES {
                    return Err(QueryError::invalid("path points are too long"));
                }
            }
        }
        InsightQuery::SqlQuery(q) => {
            if q.query.trim().is_empty() || q.query.len() > MAX_SQL_BYTES {
                return Err(QueryError::invalid(format!(
                    "SQL must be 1 to {MAX_SQL_BYTES} bytes"
                )));
            }
        }
    }
    Ok(())
}

pub fn selection(query: &InsightQuery, selection: &ActorSelection) -> Result<(), QueryError> {
    let matches = matches!(
        (query, selection),
        (
            InsightQuery::TrendsQuery(_),
            ActorSelection::TrendsPoint { .. }
        ) | (
            InsightQuery::FunnelsQuery(_),
            ActorSelection::FunnelStep { .. }
        ) | (
            InsightQuery::RetentionQuery(_),
            ActorSelection::RetentionCell { .. }
        ) | (
            InsightQuery::LifecycleQuery(_),
            ActorSelection::LifecycleCell { .. }
        ) | (
            InsightQuery::StickinessQuery(_),
            ActorSelection::StickinessBar { .. }
        ) | (InsightQuery::PathsQuery(_), ActorSelection::PathsLink { .. })
    );
    if !matches {
        return Err(QueryError::invalid(
            "the actor selection does not match the query kind",
        ));
    }
    Ok(())
}
