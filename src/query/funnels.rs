//! Funnels, computed exactly per person.
//!
//! DuckDB returns, per person, the time-ordered (timestamp, uuid) events
//! that match any step or exclusion as a step bitmask; Rust then finds each
//! person's furthest step. Definitions (the oracle implements them
//! independently):
//!
//! - all events are inside the date range; step *k* happens at or within
//!   `funnel_window` after the attempt's first step (inclusive);
//! - **ordered**: an attempt starts at any step-0 event (the anchor); each
//!   next step is the earliest later event matching it (one event never
//!   fills two steps);
//! - **strict**: the steps must be consecutive events of the person (every
//!   event in range counts, filtered or not);
//! - **unordered**: an attempt starts at any event matching some step;
//!   scanning forward inside the window, the depth is the number of distinct
//!   steps covered;
//! - an attempt is discarded when an exclusion event happens strictly
//!   between its `from_step` and its `to_step` (or the window end if
//!   `to_step` was not reached);
//! - each person counts with the deepest surviving attempt, ties going to
//!   the earliest anchor; persons without one are not in the funnel;
//! - conversion times are between consecutive steps of that attempt; the
//!   histogram is first → last step of persons who completed the funnel;
//! - a breakdown takes the anchor event's property (first touch) or the
//!   person's property; top `limit` by persons entering, rest `$$_other`.

use std::collections::HashMap;

use super::range::ResolvedRange;
use super::sql::{BREAKDOWN_OTHER, Params, breakdown_expr, event_match};
use super::{Ctx, QueryError, i64_column, string_column};
use crate::contract::common::PropertySource;
use crate::contract::insight::{
    FunnelBreakdownResult, FunnelOrder, FunnelStepResult, FunnelsQuery, HistogramBin, InsightResult,
};
use duckdb::arrow::array::Array;
use duckdb::arrow::datatypes::DataType;

/// Exclusion bits start here; steps use bits `0..20`.
const EXCLUSION_BIT: u32 = 32;
const MAX_HISTOGRAM_BINS: i64 = 10;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Event {
    pub ts: i64,
    pub mask: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Exclusion {
    pub bit: u64,
    pub from_step: usize,
    pub to_step: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Attempt {
    pub anchor: usize,
    /// Time each reached step was reached.
    pub times: Vec<i64>,
}

fn ordered_positions(events: &[Event], anchor: usize, steps: usize, window: i64) -> Vec<usize> {
    let start = events[anchor].ts;
    let mut positions = vec![anchor];
    for (index, event) in events.iter().enumerate().skip(anchor + 1) {
        if positions.len() == steps || event.ts - start > window {
            break;
        }
        if event.mask & (1 << positions.len()) != 0 {
            positions.push(index);
        }
    }
    positions
}

fn strict_positions(events: &[Event], anchor: usize, steps: usize, window: i64) -> Vec<usize> {
    let start = events[anchor].ts;
    let mut positions = vec![anchor];
    let mut index = anchor + 1;
    while positions.len() < steps
        && index < events.len()
        && events[index].ts - start <= window
        && events[index].mask & (1 << positions.len()) != 0
    {
        positions.push(index);
        index += 1;
    }
    positions
}

fn unordered_times(events: &[Event], anchor: usize, steps: usize, window: i64) -> Vec<i64> {
    let step_bits = (1_u64 << steps) - 1;
    let start = events[anchor].ts;
    let mut covered = events[anchor].mask & step_bits;
    let mut times = vec![start; covered.count_ones() as usize];
    for event in &events[anchor + 1..] {
        if covered == step_bits || event.ts - start > window {
            break;
        }
        let new = event.mask & step_bits & !covered;
        if new != 0 {
            covered |= new;
            times.extend(std::iter::repeat_n(event.ts, new.count_ones() as usize));
        }
    }
    times
}

fn tainted(events: &[Event], positions: &[usize], window: i64, exclusions: &[Exclusion]) -> bool {
    let start_time = events[positions[0]].ts;
    exclusions.iter().any(|exclusion| {
        let Some(&from) = positions.get(exclusion.from_step) else {
            return false;
        };
        let end = match positions.get(exclusion.to_step) {
            Some(&to) => to,
            None => events
                .iter()
                .enumerate()
                .skip(from + 1)
                .find(|(_, event)| event.ts - start_time > window)
                .map(|(index, _)| index)
                .unwrap_or(events.len()),
        };
        events[from + 1..end.max(from + 1)]
            .iter()
            .any(|event| event.mask & exclusion.bit != 0)
    })
}

/// Deepest step any ordered attempt can reach (ClickHouse `windowFunnel`).
fn ordered_max_depth(events: &[Event], steps: usize, window: i64) -> usize {
    let mut chain_start: Vec<Option<i64>> = vec![None; steps];
    for event in events {
        for step in (1..steps).rev() {
            if event.mask & (1 << step) != 0
                && let Some(start) = chain_start[step - 1]
                && event.ts - start <= window
            {
                chain_start[step] = Some(start);
            }
        }
        if event.mask & 1 != 0 {
            chain_start[0] = Some(event.ts);
        }
    }
    chain_start
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |step| step + 1)
}

/// The person's counting attempt, if any survives exclusions.
pub(crate) fn best_attempt(
    events: &[Event],
    order: FunnelOrder,
    steps: usize,
    window: i64,
    exclusions: &[Exclusion],
) -> Option<Attempt> {
    let anchor_bits: u64 = match order {
        FunnelOrder::Unordered => (1 << steps) - 1,
        _ => 1,
    };
    let ceiling = match order {
        FunnelOrder::Ordered => ordered_max_depth(events, steps, window),
        _ => steps,
    };
    let mut best: Option<Attempt> = None;
    for anchor in 0..events.len() {
        if events[anchor].mask & anchor_bits == 0 {
            continue;
        }
        let times = match order {
            FunnelOrder::Unordered => unordered_times(events, anchor, steps, window),
            FunnelOrder::Ordered | FunnelOrder::Strict => {
                let positions = if order == FunnelOrder::Ordered {
                    ordered_positions(events, anchor, steps, window)
                } else {
                    strict_positions(events, anchor, steps, window)
                };
                if tainted(events, &positions, window, exclusions) {
                    continue;
                }
                positions.iter().map(|index| events[*index].ts).collect()
            }
        };
        let depth = times.len();
        if best.as_ref().is_none_or(|best| depth > best.times.len()) {
            best = Some(Attempt { anchor, times });
        }
        if depth >= ceiling {
            break;
        }
    }
    best
}

pub(crate) struct PersonOutcome {
    pub person: String,
    pub times: Vec<i64>,
    pub breakdown: Option<String>,
}

/// Persons whose equal-timestamp events differ are ordered by uuid in a
/// second, small query; past this many the whole analysis runs that way.
const MAX_TIED_PERSONS: usize = 20_000;

/// Everything about the scan that is the same for every pass.
struct Scan {
    relation: String,
    params: Params,
    steps: usize,
    window: i64,
    order: FunnelOrder,
    mask_sql: String,
    bd_column: String,
    has_breakdown: bool,
    person_breakdown: bool,
    event_prefilter: String,
    keep: &'static str,
    exclusions: Vec<Exclusion>,
}

fn scan_plan(ctx: &Ctx<'_>, q: &FunnelsQuery, range: &ResolvedRange) -> Result<Scan, QueryError> {
    let steps = q.series.len();
    let window = q.funnel_window.seconds().saturating_mul(1_000_000);
    let mut params = Params::new();
    let relation = ctx
        .source
        .relation(&mut params, Some(range.from), Some(range.to), true)?;
    let mut mask_terms = Vec::new();
    for (index, node) in q.series.iter().enumerate() {
        mask_terms.push(format!(
            "(CASE WHEN {} AND {} AND {} THEN {} ELSE 0 END)",
            event_match(node, &mut params),
            ctx.source.filters(&node.properties, &mut params)?,
            ctx.source.filters(&q.properties, &mut params)?,
            1_i64 << index
        ));
    }
    let mut exclusions = Vec::new();
    for (index, exclusion) in q.exclusions.iter().enumerate() {
        let bit = 1_u64 << (EXCLUSION_BIT + index as u32);
        mask_terms.push(format!(
            "(CASE WHEN event = {} THEN {bit} ELSE 0 END)",
            params.text(&exclusion.event)
        ));
        exclusions.push(Exclusion {
            bit,
            from_step: exclusion.from_step,
            to_step: exclusion.to_step,
        });
    }
    let breakdown = q
        .breakdown
        .as_ref()
        .map(|breakdown| {
            ctx.source
                .property_text(breakdown.source, &breakdown.property)
                .map(|text| breakdown_expr(&text))
        })
        .transpose()?;
    let bd_column = breakdown
        .as_ref()
        .map(|text| format!(", {text} AS bd"))
        .unwrap_or_default();
    // Only step and exclusion events matter unless order is strict (where
    // any event in between breaks a sequence) or a step matches all events.
    // Saying so explicitly lets DuckDB skip row groups: files are sorted by
    // (event, timestamp).
    let event_prefilter = if q.funnel_order != FunnelOrder::Strict
        && q.series.iter().all(|node| node.event.is_some())
    {
        let names: Vec<String> = q
            .series
            .iter()
            .filter_map(|node| node.event.as_deref())
            .chain(q.exclusions.iter().map(|exclusion| exclusion.event.as_str()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|name| params.text(name))
            .collect();
        format!("event IN ({})", names.join(", "))
    } else {
        "TRUE".to_owned()
    };
    let keep = if q.funnel_order == FunnelOrder::Strict {
        "TRUE"
    } else {
        "mask <> 0"
    };
    Ok(Scan {
        relation,
        params,
        steps,
        window,
        order: q.funnel_order,
        mask_sql: mask_terms.join(" + "),
        has_breakdown: breakdown.is_some(),
        bd_column,
        person_breakdown: matches!(
            q.breakdown.as_ref().map(|b| b.source),
            Some(PropertySource::Person)
        ),
        event_prefilter,
        keep,
        exclusions,
    })
}

/// One person's events as collected from a scan, in `(timestamp, …)` order.
struct Collected {
    person: String,
    events: Vec<Event>,
    breakdowns: Vec<Option<String>>,
}

impl Collected {
    fn new(person: &str) -> Self {
        Self {
            person: person.to_owned(),
            events: Vec::new(),
            breakdowns: Vec::new(),
        }
    }
}

impl Scan {
    fn outcome(&self, collected: &Collected) -> Option<PersonOutcome> {
        let attempt = best_attempt(
            &collected.events,
            self.order,
            self.steps,
            self.window,
            &self.exclusions,
        )?;
        let index = if self.person_breakdown {
            0
        } else {
            attempt.anchor
        };
        Some(PersonOutcome {
            person: collected.person.clone(),
            breakdown: collected.breakdowns.get(index).cloned().flatten(),
            times: attempt.times,
        })
    }

    /// Do two events with the same timestamp differ in anything the
    /// analysis reads (their order then needs the uuid)?
    fn tie_matters(&self, collected: &Collected) -> bool {
        collected.events.windows(2).enumerate().any(|(index, pair)| {
            pair[0].ts == pair[1].ts
                && (pair[0].mask != pair[1].mask
                    || (self.has_breakdown
                        && collected.breakdowns[index] != collected.breakdowns[index + 1]))
        })
    }

    fn push(&self, collected: &mut Collected, ts: i64, mask: i64, breakdown: Option<Option<String>>) {
        collected.events.push(Event {
            ts,
            mask: mask as u64,
        });
        if let Some(breakdown) = breakdown {
            collected.breakdowns.push(breakdown);
        }
    }
}

fn analyze(
    ctx: &Ctx<'_>,
    q: &FunnelsQuery,
    range: &ResolvedRange,
) -> Result<Vec<PersonOutcome>, QueryError> {
    let scan = scan_plan(ctx, q, range)?;
    match analyze_by_hash(ctx, &scan)? {
        Some(outcomes) => Ok(outcomes),
        None => analyze_ordered(ctx, &scan, false),
    }
}

/// Fast path: DuckDB orders each partition by `(hash(person), timestamp)` —
/// two integers, no strings, no uuid column read — and a person's events are
/// the rows of one hash run (a hash collision between persons is resolved by
/// comparing the ids). Persons with equal-timestamp events that differ are
/// not decided here: their order is the uuid's, so they are re-read by
/// [`analyze_ordered`] restricted to them. `None` when too many persons tie.
fn analyze_by_hash(ctx: &Ctx<'_>, scan: &Scan) -> Result<Option<Vec<PersonOutcome>>, QueryError> {
    let partitions = ctx.person_partitions()?;
    let mut outcomes = Vec::new();
    let mut tied: Vec<String> = Vec::new();
    // The rows of one hash value; almost always a single person.
    let mut run_hash: Option<i64> = None;
    let mut run: Vec<Collected> = Vec::new();
    let mut overflow = false;
    let bd_select = if scan.has_breakdown { ", bd" } else { "" };
    let finish = |run: &mut Vec<Collected>,
                  outcomes: &mut Vec<PersonOutcome>,
                  tied: &mut Vec<String>,
                  overflow: &mut bool| {
        for collected in run.drain(..) {
            if scan.tie_matters(&collected) {
                if tied.len() >= MAX_TIED_PERSONS {
                    *overflow = true;
                } else {
                    tied.push(collected.person);
                }
            } else if let Some(outcome) = scan.outcome(&collected) {
                outcomes.push(outcome);
            }
        }
    };
    for partition in 0..partitions {
        let sql = format!(
            "WITH ev AS ({relation}), \
             x AS (SELECT person_id, (hash(person_id) >> 1::UBIGINT)::BIGINT AS h, ts, \
                          ({mask_sql})::BIGINT AS mask{bd_column} FROM ev \
                   WHERE {part} AND {prefilter}) \
             SELECT person_id, h, ts, mask{bd_select} FROM x WHERE {keep} ORDER BY h, ts",
            relation = scan.relation,
            mask_sql = scan.mask_sql,
            bd_column = scan.bd_column,
            part = super::partition_clause(partition, partitions),
            prefilter = scan.event_prefilter,
            keep = scan.keep,
        );
        let mut types = vec![DataType::Utf8, DataType::Int64, DataType::Int64, DataType::Int64];
        if scan.has_breakdown {
            types.push(DataType::Utf8);
        }
        ctx.arrow_streaming(&sql, &scan.params, &types, |batch| {
            let persons = string_column(batch, 0)?;
            let hashes = i64_column(batch, 1)?;
            let times = i64_column(batch, 2)?;
            let masks = i64_column(batch, 3)?;
            let breakdowns = if scan.has_breakdown {
                Some(string_column(batch, 4)?)
            } else {
                None
            };
            for row in 0..batch.num_rows() {
                let hash = hashes.value(row);
                if run_hash != Some(hash) {
                    finish(&mut run, &mut outcomes, &mut tied, &mut overflow);
                    run_hash = Some(hash);
                }
                let person = persons.value(row);
                let position = match run.iter().position(|c| c.person == person) {
                    Some(position) => position,
                    None => {
                        run.push(Collected::new(person));
                        run.len() - 1
                    }
                };
                scan.push(
                    &mut run[position],
                    times.value(row),
                    masks.value(row),
                    breakdowns
                        .as_ref()
                        .map(|values| (!values.is_null(row)).then(|| values.value(row).to_owned())),
                );
            }
            Ok(())
        })?;
        finish(&mut run, &mut outcomes, &mut tied, &mut overflow);
        run_hash = None;
        if overflow {
            return Ok(None);
        }
    }
    if !tied.is_empty() {
        ctx.temp_text_table("funnel_tied", "person_id", &tied)?;
        outcomes.extend(analyze_ordered(ctx, scan, true)?);
    }
    Ok(Some(outcomes))
}

/// Exact path: rows arrive ordered by `(person, timestamp, uuid)`; a
/// person's events are collected, analysed, and dropped before the next
/// person. `tied_only` restricts it to the persons in `funnel_tied`.
fn analyze_ordered(
    ctx: &Ctx<'_>,
    scan: &Scan,
    tied_only: bool,
) -> Result<Vec<PersonOutcome>, QueryError> {
    let partitions = if tied_only {
        1
    } else {
        ctx.person_partitions()?
    };
    let only = if tied_only {
        "person_id IN (SELECT person_id FROM funnel_tied)"
    } else {
        "TRUE"
    };
    let bd_select = if scan.has_breakdown { ", bd" } else { "" };
    let mut outcomes = Vec::new();
    let mut current: Option<Collected> = None;
    let finish = |current: &mut Option<Collected>, outcomes: &mut Vec<PersonOutcome>| {
        if let Some(collected) = current.take()
            && let Some(outcome) = scan.outcome(&collected)
        {
            outcomes.push(outcome);
        }
    };
    for partition in 0..partitions {
        let sql = format!(
            "WITH ev AS ({relation}), \
             x AS (SELECT person_id, ts, uuid, ({mask_sql})::BIGINT AS mask{bd_column} FROM ev \
                   WHERE {part} AND {prefilter} AND {only}) \
             SELECT person_id, ts, mask{bd_select} FROM x WHERE {keep} \
             ORDER BY person_id, ts, uuid",
            relation = scan.relation,
            mask_sql = scan.mask_sql,
            bd_column = scan.bd_column,
            part = super::partition_clause(partition, partitions),
            prefilter = scan.event_prefilter,
            keep = scan.keep,
        );
        let mut types = vec![DataType::Utf8, DataType::Int64, DataType::Int64];
        if scan.has_breakdown {
            types.push(DataType::Utf8);
        }
        ctx.arrow_streaming(&sql, &scan.params, &types, |batch| {
            let persons = string_column(batch, 0)?;
            let times = i64_column(batch, 1)?;
            let masks = i64_column(batch, 2)?;
            let breakdowns = if scan.has_breakdown {
                Some(string_column(batch, 3)?)
            } else {
                None
            };
            for row in 0..batch.num_rows() {
                let person = persons.value(row);
                if current.as_ref().map(|c| c.person.as_str()) != Some(person) {
                    finish(&mut current, &mut outcomes);
                    current = Some(Collected::new(person));
                }
                if let Some(collected) = current.as_mut() {
                    scan.push(
                        collected,
                        times.value(row),
                        masks.value(row),
                        breakdowns.as_ref().map(|values| {
                            (!values.is_null(row)).then(|| values.value(row).to_owned())
                        }),
                    );
                }
            }
            Ok(())
        })?;
        finish(&mut current, &mut outcomes);
    }
    Ok(outcomes)
}

pub(crate) fn histogram(durations_us: &[i64]) -> Vec<HistogramBin> {
    let (Some(min), Some(max)) = (durations_us.iter().min(), durations_us.iter().max()) else {
        return Vec::new();
    };
    let low = min.div_euclid(1_000_000) * 1_000_000;
    let span = max - low;
    let width_seconds =
        ((span + MAX_HISTOGRAM_BINS * 1_000_000 - 1) / (MAX_HISTOGRAM_BINS * 1_000_000)).max(1);
    let width = width_seconds * 1_000_000;
    let bins = (span / width + 1) as usize;
    let mut counts = vec![0_u64; bins];
    for duration in durations_us {
        let index = (((duration - low) / width) as usize).min(bins - 1);
        counts[index] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(index, count)| {
            let from = low + index as i64 * width;
            HistogramBin {
                from_s: from as f64 / 1e6,
                to_s: (from + width) as f64 / 1e6,
                count,
            }
        })
        .collect()
}

pub(crate) fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    })
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        100.0 * numerator as f64 / denominator as f64
    }
}

pub(crate) fn step_results(q: &FunnelsQuery, persons: &[&PersonOutcome]) -> Vec<FunnelStepResult> {
    let steps = q.series.len();
    let counts: Vec<u64> = (0..steps)
        .map(|step| persons.iter().filter(|p| p.times.len() > step).count() as u64)
        .collect();
    (0..steps)
        .map(|step| {
            let (average, median_time) = if step == 0 {
                (None, None)
            } else {
                let mut durations: Vec<f64> = persons
                    .iter()
                    .filter(|p| p.times.len() > step)
                    .map(|p| (p.times[step] - p.times[step - 1]) as f64 / 1e6)
                    .collect();
                let average = (!durations.is_empty())
                    .then(|| durations.iter().sum::<f64>() / durations.len() as f64);
                (average, median(&mut durations))
            };
            FunnelStepResult {
                order: step,
                name: q.series[step].label(),
                count: counts[step],
                conversion_from_previous: if step == 0 {
                    ratio(counts[0], counts[0])
                } else {
                    ratio(counts[step], counts[step - 1])
                },
                conversion_from_start: ratio(counts[step], counts[0]),
                dropped_off: if step == 0 {
                    0
                } else {
                    counts[step - 1] - counts[step]
                },
                average_conversion_time_s: average,
                median_conversion_time_s: median_time,
            }
        })
        .collect()
}

/// Assign each person their breakdown bucket (`$$_other` past the top
/// `limit`). Returns the ordered bucket list.
fn assign_breakdowns(q: &FunnelsQuery, outcomes: &mut [PersonOutcome]) -> Vec<String> {
    let Some(breakdown) = &q.breakdown else {
        return Vec::new();
    };
    let mut counts: HashMap<String, u64> = HashMap::new();
    for outcome in outcomes.iter_mut() {
        let value = outcome
            .breakdown
            .take()
            .unwrap_or_else(|| super::sql::BREAKDOWN_NONE.to_owned());
        *counts.entry(value.clone()).or_default() += 1;
        outcome.breakdown = Some(value);
    }
    let mut ranked: Vec<(String, u64)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let limit = breakdown.limit as usize;
    let other = ranked.len() > limit;
    let mut values: Vec<String> = ranked.into_iter().take(limit).map(|(v, _)| v).collect();
    for outcome in outcomes.iter_mut() {
        if let Some(value) = &outcome.breakdown
            && !values.contains(value)
        {
            outcome.breakdown = Some(BREAKDOWN_OTHER.to_owned());
        }
    }
    if other {
        values.push(BREAKDOWN_OTHER.to_owned());
    }
    values
}

pub(crate) fn run(
    ctx: &Ctx<'_>,
    q: &FunnelsQuery,
    range: &ResolvedRange,
) -> Result<InsightResult, QueryError> {
    let mut outcomes = analyze(ctx, q, range)?;
    let values = assign_breakdowns(q, &mut outcomes);
    let all: Vec<&PersonOutcome> = outcomes.iter().collect();
    let steps = step_results(q, &all);
    let last = q.series.len() - 1;
    let durations: Vec<i64> = outcomes
        .iter()
        .filter(|p| p.times.len() == q.series.len())
        .map(|p| p.times[last] - p.times[0])
        .collect();
    let breakdowns = values
        .iter()
        .map(|value| {
            let members: Vec<&PersonOutcome> = outcomes
                .iter()
                .filter(|p| p.breakdown.as_deref() == Some(value))
                .collect();
            FunnelBreakdownResult {
                breakdown_value: value.clone(),
                steps: step_results(q, &members),
            }
        })
        .collect();
    Ok(InsightResult::Funnels {
        steps,
        breakdowns,
        time_to_convert: histogram(&durations),
    })
}

pub(crate) fn actors(
    ctx: &Ctx<'_>,
    q: &FunnelsQuery,
    range: &ResolvedRange,
    step: usize,
    converted: bool,
    breakdown_value: Option<&str>,
) -> Result<Vec<String>, QueryError> {
    if step >= q.series.len() {
        return Err(QueryError::invalid("step is out of range"));
    }
    let mut outcomes = analyze(ctx, q, range)?;
    assign_breakdowns(q, &mut outcomes);
    Ok(outcomes
        .into_iter()
        .filter(|p| {
            let depth = p.times.len();
            let in_cell = if converted {
                depth > step
            } else {
                step > 0 && depth == step
            };
            let in_breakdown = match (breakdown_value, q.breakdown.is_some()) {
                (Some(value), true) => p.breakdown.as_deref() == Some(value),
                _ => true,
            };
            in_cell && in_breakdown
        })
        .map(|p| p.person)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(spec: &[(i64, u64)]) -> Vec<Event> {
        spec.iter()
            .map(|(ts, mask)| Event {
                ts: *ts,
                mask: *mask,
            })
            .collect()
    }

    #[test]
    fn ordered_finds_a_later_anchor_that_converts() {
        // A(0) ... window passes ... A(100) B(105): earliest-anchor chaining
        // would miss this conversion.
        let e = events(&[(0, 1), (100, 1), (105, 2)]);
        let attempt = best_attempt(&e, FunnelOrder::Ordered, 2, 10, &[]).unwrap();
        assert_eq!(attempt.times, vec![100, 105]);
        assert_eq!(attempt.anchor, 1);
    }

    #[test]
    fn one_event_never_fills_two_steps_and_strict_needs_adjacency() {
        let e = events(&[(0, 0b11)]);
        assert_eq!(
            best_attempt(&e, FunnelOrder::Ordered, 2, 10, &[])
                .unwrap()
                .times
                .len(),
            1
        );
        let e = events(&[(0, 1), (1, 0), (2, 2)]);
        assert_eq!(
            best_attempt(&e, FunnelOrder::Strict, 2, 10, &[])
                .unwrap()
                .times
                .len(),
            1
        );
        assert_eq!(
            best_attempt(&e, FunnelOrder::Ordered, 2, 10, &[])
                .unwrap()
                .times
                .len(),
            2
        );
    }

    #[test]
    fn exclusions_discard_tainted_attempts_only() {
        let exclusion = Exclusion {
            bit: 1 << EXCLUSION_BIT,
            from_step: 0,
            to_step: 1,
        };
        let x = 1 << EXCLUSION_BIT;
        let e = events(&[(0, 1), (1, x), (2, 2), (10, 1), (11, 2)]);
        let attempt = best_attempt(&e, FunnelOrder::Ordered, 2, 100, &[exclusion]).unwrap();
        assert_eq!(attempt.times, vec![10, 11]);
        let e = events(&[(0, 1), (1, x)]);
        assert!(best_attempt(&e, FunnelOrder::Ordered, 2, 100, &[exclusion]).is_none());
    }

    #[test]
    fn unordered_counts_distinct_steps_in_window() {
        let e = events(&[(0, 0b100), (5, 0b001), (7, 0b100), (50, 0b010)]);
        let attempt = best_attempt(&e, FunnelOrder::Unordered, 3, 10, &[]).unwrap();
        assert_eq!(attempt.times, vec![0, 5]);
        let attempt = best_attempt(&e, FunnelOrder::Unordered, 3, 100, &[]).unwrap();
        assert_eq!(attempt.times, vec![0, 5, 50]);
    }

    #[test]
    fn histogram_bins_cover_every_duration() {
        let bins = histogram(&[0, 500_000, 3_000_000, 25_000_000]);
        assert_eq!(bins.iter().map(|b| b.count).sum::<u64>(), 4);
        assert_eq!(bins[0].from_s, 0.0);
        assert!(bins.last().unwrap().to_s > 25.0);
        assert_eq!(histogram(&[]).len(), 0);
        assert_eq!(histogram(&[7_000_000]).len(), 1);
    }
}
