//! The correctness oracle: every insight kind computed by brute force over
//! the raw events and an identity map, written from the definitions in the
//! kind modules' docs — not from their SQL or their Rust. Slow on purpose:
//! nested loops, sets, exhaustive search.
//!
//! Shared with the engine only: date-range *resolution* (`range::resolve`,
//! `ResolvedRange::buckets`, `previous`), the histogram binning, and the
//! strict-numeric parse — each unit-tested on its own. Bucket membership,
//! filters, property text, identity, and every aggregation are independent.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, TimeZone, Utc};
use serde_json::{Map, Value};

use super::range::{self, ResolvedRange};
use crate::capture::event::CapturedEvent;
use crate::contract::common::{
    BREAKDOWN_NONE, BREAKDOWN_OTHER, Breakdown, DateRange, Interval, PropertyFilter,
    PropertyOperator, PropertySource,
};
use crate::contract::insight::*;
use crate::lake::parquet::promoted_column;

pub struct World<'a> {
    /// Every event of the project, any order.
    pub events: &'a [CapturedEvent],
    /// distinct id → person id; ids not present are their own person.
    pub person_of: &'a HashMap<String, String>,
    /// person id → current properties.
    pub person_props: &'a HashMap<String, Map<String, Value>>,
    pub now: DateTime<Utc>,
}

fn us(time: DateTime<Utc>) -> i64 {
    time.timestamp_micros()
}

fn at(us: i64) -> DateTime<Utc> {
    Utc.timestamp_micros(us).single().unwrap()
}

/// Calendar bucket start, from chrono's calendar (not integer arithmetic).
fn bucket_of(time: DateTime<Utc>, interval: Interval) -> DateTime<Utc> {
    let date = time.date_naive();
    let midnight = |date: NaiveDate| Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
    match interval {
        Interval::Hour => Utc
            .with_ymd_and_hms(date.year(), date.month(), date.day(), time.hour_of(), 0, 0)
            .unwrap(),
        Interval::Day => midnight(date),
        Interval::Week => {
            midnight(date - Duration::days(i64::from(date.weekday().num_days_from_monday())))
        }
        Interval::Month => midnight(NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap()),
    }
}

trait HourOf {
    fn hour_of(&self) -> u32;
}

impl HourOf for DateTime<Utc> {
    fn hour_of(&self) -> u32 {
        chrono::Timelike::hour(self)
    }
}

fn next_of(start: DateTime<Utc>, interval: Interval) -> DateTime<Utc> {
    match interval {
        Interval::Hour => start + Duration::hours(1),
        Interval::Day => start + Duration::days(1),
        Interval::Week => start + Duration::days(7),
        Interval::Month => start.checked_add_months(Months::new(1)).unwrap(),
    }
}

fn previous_of(start: DateTime<Utc>, interval: Interval) -> DateTime<Utc> {
    match interval {
        Interval::Hour => start - Duration::hours(1),
        Interval::Day => start - Duration::days(1),
        Interval::Week => start - Duration::days(7),
        Interval::Month => start.checked_sub_months(Months::new(1)).unwrap(),
    }
}

/// JSON text of a value: strings verbatim, numbers as JSON, booleans,
/// containers as compact JSON, null = missing.
fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        other => Some(other.to_string()),
    }
}

/// Property text as the engine defines it.
pub fn event_text(event: &CapturedEvent, key: &str) -> Option<String> {
    let value = event.properties.get(key)?;
    if promoted_column(key).is_some() {
        // Promoted columns: non-empty strings, numbers, booleans only.
        return match value {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        };
    }
    json_text(value)
}

fn numeric(text: Option<&str>) -> Option<f64> {
    text.and_then(super::sql::parse_numeric)
}

fn lower(text: &str) -> String {
    text.to_lowercase()
}

/// The regex subset the generator uses: `^lit`, `lit$`, `^lit$`, `lit`.
fn regex_match(text: &str, pattern: &str) -> bool {
    let anchored_start = pattern.starts_with('^');
    let anchored_end = pattern.ends_with('$') && pattern.len() > 1;
    let literal = pattern.trim_start_matches('^').trim_end_matches('$');
    assert!(
        literal
            .chars()
            .all(|c| c.is_alphanumeric() || c == '/' || c == '_'),
        "oracle regex subset violated: {pattern}"
    );
    match (anchored_start, anchored_end) {
        (true, true) => text == literal,
        (true, false) => text.starts_with(literal),
        (false, true) => text.ends_with(literal),
        (false, false) => text.contains(literal),
    }
}

fn parse_date(text: &str) -> Option<i64> {
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(time.with_timezone(&Utc).timestamp_micros());
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .map(|date| us(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())))
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => panic!("oracle got a non-scalar filter value {other}"),
    }
}

fn holds(text: Option<&str>, filter: &PropertyFilter) -> bool {
    let values: Vec<String> = match &filter.value {
        Value::Array(items) => items.iter().map(scalar).collect(),
        Value::Null => Vec::new(),
        other => vec![scalar(other)],
    };
    match filter.operator {
        PropertyOperator::Exact => text.is_some_and(|t| values.iter().any(|v| v == t)),
        PropertyOperator::IsNot => !text.is_some_and(|t| values.iter().any(|v| v == t)),
        PropertyOperator::Icontains => text.is_some_and(|t| lower(t).contains(&lower(&values[0]))),
        PropertyOperator::NotIcontains => {
            !text.is_some_and(|t| lower(t).contains(&lower(&values[0])))
        }
        PropertyOperator::Regex => text.is_some_and(|t| regex_match(t, &values[0])),
        PropertyOperator::NotRegex => !text.is_some_and(|t| regex_match(t, &values[0])),
        PropertyOperator::Gt
        | PropertyOperator::Gte
        | PropertyOperator::Lt
        | PropertyOperator::Lte => {
            let bound = match &filter.value {
                Value::Number(number) => number.as_f64().unwrap(),
                Value::String(text) => super::sql::parse_numeric(text).unwrap(),
                _ => unreachable!(),
            };
            match numeric(text) {
                None => false,
                Some(value) => match filter.operator {
                    PropertyOperator::Gt => value > bound,
                    PropertyOperator::Gte => value >= bound,
                    PropertyOperator::Lt => value < bound,
                    _ => value <= bound,
                },
            }
        }
        PropertyOperator::IsSet => text.is_some(),
        PropertyOperator::IsNotSet => text.is_none(),
        PropertyOperator::IsDateBefore | PropertyOperator::IsDateAfter => {
            let bound = parse_date(filter.value.as_str().unwrap()).unwrap();
            match text.and_then(parse_date) {
                None => false,
                Some(value) => {
                    if filter.operator == PropertyOperator::IsDateBefore {
                        value < bound
                    } else {
                        value > bound
                    }
                }
            }
        }
    }
}

impl World<'_> {
    pub fn person(&self, distinct_id: &str) -> String {
        self.person_of
            .get(distinct_id)
            .cloned()
            .unwrap_or_else(|| distinct_id.to_owned())
    }

    fn person_text(&self, person: &str, key: &str) -> Option<String> {
        self.person_props
            .get(person)
            .and_then(|props| props.get(key))
            .and_then(json_text)
    }

    fn text(&self, event: &CapturedEvent, source: PropertySource, key: &str) -> Option<String> {
        match source {
            PropertySource::Event => event_text(event, key),
            PropertySource::Person => self.person_text(&self.person(&event.distinct_id), key),
        }
    }

    fn passes(&self, event: &CapturedEvent, filters: &[PropertyFilter]) -> bool {
        filters.iter().all(|filter| {
            holds(
                self.text(event, filter.source, &filter.key).as_deref(),
                filter,
            )
        })
    }

    fn matches(&self, event: &CapturedEvent, node: &EventNode, global: &[PropertyFilter]) -> bool {
        node.event.as_ref().is_none_or(|name| &event.event == name)
            && self.passes(event, &node.properties)
            && self.passes(event, global)
    }

    fn breakdown_text(&self, event: &CapturedEvent, breakdown: &Breakdown) -> String {
        self.text(event, breakdown.source, &breakdown.property)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| BREAKDOWN_NONE.to_owned())
    }

    fn earliest(&self) -> Option<i64> {
        self.events.iter().map(|e| us(e.timestamp)).min()
    }

    fn resolve(&self, range: &DateRange, interval: Interval) -> ResolvedRange {
        range::resolve(range, interval, self.now, || Ok(self.earliest())).unwrap()
    }

    fn in_range(&self, event: &CapturedEvent, range: &ResolvedRange) -> bool {
        let ts = us(event.timestamp);
        range.from <= ts && ts < range.to
    }
}

// ── Trends ───────────────────────────────────────────────────────

fn quantile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let h = (sorted.len() - 1) as f64 * p;
    let low = h.floor() as usize;
    let high = h.ceil() as usize;
    sorted[low] + (h - low as f64) * (sorted[high] - sorted[low])
}

fn math_value(world: &World<'_>, node: &EventNode, events: &[&CapturedEvent]) -> f64 {
    let values = || -> Vec<f64> {
        events
            .iter()
            .filter_map(|e| {
                numeric(event_text(e, node.math_property.as_deref().unwrap()).as_deref())
            })
            .collect()
    };
    match node.math {
        Math::Total => events.len() as f64,
        Math::Dau | Math::WeeklyActive | Math::MonthlyActive => events
            .iter()
            .map(|e| world.person(&e.distinct_id))
            .collect::<HashSet<_>>()
            .len() as f64,
        Math::UniqueSession => events
            .iter()
            .filter_map(|e| event_text(e, "$session_id"))
            .collect::<HashSet<_>>()
            .len() as f64,
        Math::Sum => values().iter().sum(),
        Math::Avg => {
            let v = values();
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f64>() / v.len() as f64
            }
        }
        Math::Min => values().into_iter().reduce(f64::min).unwrap_or(0.0),
        Math::Max => values().into_iter().reduce(f64::max).unwrap_or(0.0),
        Math::Median => quantile(&values(), 0.5),
        Math::P90 => quantile(&values(), 0.9),
        Math::P95 => quantile(&values(), 0.95),
        Math::P99 => quantile(&values(), 0.99),
    }
}

fn window(math: Math) -> Option<i64> {
    match math {
        Math::WeeklyActive => Some(7),
        Math::MonthlyActive => Some(30),
        _ => None,
    }
}

struct Ranking {
    top: Vec<String>,
    other: bool,
}

fn rank(counts: HashMap<String, u64>, limit: u32) -> Ranking {
    let mut ranked: Vec<(String, u64)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let other = ranked.len() > limit as usize;
    Ranking {
        top: ranked
            .into_iter()
            .take(limit as usize)
            .map(|(v, _)| v)
            .collect(),
        other,
    }
}

fn trends_ranking(
    world: &World<'_>,
    q: &TrendsQuery,
    node: &EventNode,
    period: &ResolvedRange,
) -> Option<Ranking> {
    let breakdown = q.breakdown.as_ref()?;
    let mut counts: HashMap<String, u64> = HashMap::new();
    for event in world.events {
        if world.in_range(event, period) && world.matches(event, node, &q.properties) {
            *counts
                .entry(world.breakdown_text(event, breakdown))
                .or_default() += 1;
        }
    }
    Some(rank(counts, breakdown.limit))
}

fn mapped(world: &World<'_>, q: &TrendsQuery, ranking: &Ranking, event: &CapturedEvent) -> String {
    let value = world.breakdown_text(event, q.breakdown.as_ref().unwrap());
    if ranking.top.contains(&value) {
        value
    } else {
        BREAKDOWN_OTHER.to_owned()
    }
}

/// Events of the series in `[from, to)` with the given breakdown key.
fn series_events<'w>(
    world: &'w World<'_>,
    q: &TrendsQuery,
    node: &EventNode,
    ranking: Option<&Ranking>,
    key: Option<&str>,
    from: i64,
    to: i64,
) -> Vec<&'w CapturedEvent> {
    world
        .events
        .iter()
        .filter(|e| {
            let ts = us(e.timestamp);
            from <= ts && ts < to && world.matches(e, node, &q.properties)
        })
        .filter(|e| match (ranking, key) {
            (Some(ranking), Some(key)) => mapped(world, q, ranking, e) == key,
            _ => true,
        })
        .collect()
}

/// `(breakdown key, data, aggregated value)` of one output series.
type Computed = (Option<String>, Vec<f64>, f64);

fn trends_period(
    world: &World<'_>,
    q: &TrendsQuery,
    rankings: &[Option<Ranking>],
    period: &ResolvedRange,
    compare: Option<&str>,
) -> Vec<TrendSeries> {
    let buckets = period.buckets().unwrap();
    let mut per_series: Vec<Vec<Computed>> = Vec::new();
    for (index, node) in q.series.iter().enumerate() {
        let ranking = rankings[index].as_ref();
        let keys: Vec<Option<String>> = match ranking {
            None => vec![None],
            Some(ranking) if ranking.top.is_empty() && !ranking.other => Vec::new(),
            Some(ranking) => ranking
                .top
                .iter()
                .cloned()
                .map(Some)
                .chain(std::iter::once(Some(BREAKDOWN_OTHER.to_owned())))
                .collect(),
        };
        let mut computed = Vec::new();
        for key in keys {
            let data: Vec<f64> = buckets
                .iter()
                .map(|bucket| {
                    // Events of this bucket, found via the calendar.
                    let events: Vec<&CapturedEvent> = match window(node.math) {
                        None => series_events(
                            world,
                            q,
                            node,
                            ranking,
                            key.as_deref(),
                            period.from,
                            period.to,
                        )
                        .into_iter()
                        .filter(|e| us(bucket_of(e.timestamp, period.interval)) == *bucket)
                        .collect(),
                        Some(days) => {
                            let end = next_of(at(*bucket), period.interval);
                            let start = end - Duration::days(days);
                            series_events(
                                world,
                                q,
                                node,
                                ranking,
                                key.as_deref(),
                                us(start),
                                us(end).min(period.to),
                            )
                        }
                    };
                    math_value(world, node, &events)
                })
                .collect();
            let whole = series_events(
                world,
                q,
                node,
                ranking,
                key.as_deref(),
                period.from,
                period.to,
            );
            let aggregated = math_value(world, node, &whole);
            let is_other = key.as_deref() == Some(BREAKDOWN_OTHER);
            let keep = !is_other
                || ranking.is_some_and(|r| r.other)
                || aggregated != 0.0
                || data.iter().any(|v| *v != 0.0);
            if keep {
                computed.push((key, data, aggregated));
            }
        }
        per_series.push(computed);
    }
    let days: Vec<String> = buckets.iter().map(|b| range::rfc3339(*b)).collect();
    let make = |label: String, series_index, breakdown_value, data, aggregated_value| TrendSeries {
        label,
        series_index,
        breakdown_value,
        compare: compare.map(str::to_owned),
        days: days.clone(),
        labels: buckets
            .iter()
            .map(|b| range::label(*b, period.interval))
            .collect(),
        data,
        aggregated_value,
    };
    let mut out = Vec::new();
    match &q.formula {
        None => {
            for (index, computed) in per_series.into_iter().enumerate() {
                for (key, data, aggregated) in computed {
                    out.push(make(
                        q.series[index].label(),
                        Some(index),
                        key,
                        data,
                        aggregated,
                    ));
                }
            }
        }
        Some(text) => {
            let formula = super::formula::Formula::parse(text, q.series.len()).unwrap();
            let mut keys: Vec<Option<String>> = Vec::new();
            for (key, _, _) in per_series.iter().flatten() {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
            for key in keys {
                let find =
                    |series: &Vec<Computed>| series.iter().find(|(k, _, _)| *k == key).cloned();
                let found: Vec<_> = per_series.iter().map(find).collect();
                let data = (0..buckets.len())
                    .map(|b| {
                        let values: Vec<f64> = found
                            .iter()
                            .map(|f| f.as_ref().map(|f| f.1[b]).unwrap_or(0.0))
                            .collect();
                        formula.evaluate(&values)
                    })
                    .collect();
                let aggregated: Vec<f64> = found
                    .iter()
                    .map(|f| f.as_ref().map(|f| f.2).unwrap_or(0.0))
                    .collect();
                out.push(make(
                    text.clone(),
                    None,
                    key,
                    data,
                    formula.evaluate(&aggregated),
                ));
            }
        }
    }
    out
}

pub fn trends(world: &World<'_>, q: &TrendsQuery) -> InsightResult {
    let range = world.resolve(&q.date_range, q.interval);
    let rankings: Vec<Option<Ranking>> = q
        .series
        .iter()
        .map(|node| trends_ranking(world, q, node, &range))
        .collect();
    let mut series = trends_period(world, q, &rankings, &range, q.compare.then_some("current"));
    if q.compare {
        series.extend(trends_period(
            world,
            q,
            &rankings,
            &range.previous(),
            Some("previous"),
        ));
    }
    InsightResult::Trends { series }
}

pub fn trends_actors(
    world: &World<'_>,
    q: &TrendsQuery,
    series_index: usize,
    day: &str,
    breakdown_value: Option<&str>,
) -> BTreeSet<String> {
    let range = world.resolve(&q.date_range, q.interval);
    let node = &q.series[series_index];
    let bucket = range::parse_instant(day).unwrap();
    let ranking = trends_ranking(world, q, node, &range);
    let key = if q.breakdown.is_some() {
        breakdown_value
    } else {
        None
    };
    let events = match window(node.math) {
        None => series_events(world, q, node, ranking.as_ref(), key, range.from, range.to)
            .into_iter()
            .filter(|e| us(bucket_of(e.timestamp, range.interval)) == bucket)
            .collect::<Vec<_>>(),
        Some(days) => {
            let end = next_of(at(bucket), range.interval);
            series_events(
                world,
                q,
                node,
                ranking.as_ref(),
                key,
                us(end - Duration::days(days)),
                us(end).min(range.to),
            )
        }
    };
    events
        .into_iter()
        .filter(|e| match node.math {
            Math::UniqueSession => event_text(e, "$session_id").is_some(),
            Math::Total | Math::Dau | Math::WeeklyActive | Math::MonthlyActive => true,
            _ => {
                numeric(event_text(e, node.math_property.as_deref().unwrap()).as_deref()).is_some()
            }
        })
        .map(|e| world.person(&e.distinct_id))
        .collect()
}

// ── Funnels ──────────────────────────────────────────────────────

struct Outcome {
    person: String,
    times: Vec<i64>,
    breakdown: Option<String>,
}

/// Exhaustive ordered search from one anchor: deepest sequence, ties to the
/// lexicographically smallest positions.
fn ordered_search(
    events: &[&CapturedEvent],
    matches: &[Vec<bool>],
    anchor: usize,
    steps: usize,
    window: i64,
) -> Vec<usize> {
    fn dfs(
        events: &[&CapturedEvent],
        matches: &[Vec<bool>],
        steps: usize,
        window: i64,
        current: &mut Vec<usize>,
        best: &mut Vec<usize>,
    ) {
        if current.len() > best.len() || (current.len() == best.len() && *current < *best) {
            *best = current.clone();
        }
        if current.len() == steps {
            return;
        }
        let start = us(events[current[0]].timestamp);
        let last = *current.last().unwrap();
        for next in last + 1..events.len() {
            if us(events[next].timestamp) - start > window {
                break;
            }
            if matches[next][current.len()] {
                current.push(next);
                dfs(events, matches, steps, window, current, best);
                current.pop();
            }
        }
    }
    let mut best = vec![anchor];
    dfs(events, matches, steps, window, &mut vec![anchor], &mut best);
    best
}

fn funnel_outcomes(world: &World<'_>, q: &FunnelsQuery) -> Vec<Outcome> {
    let range = world.resolve(&q.date_range, Interval::Day);
    let steps = q.series.len();
    let window = q.funnel_window.seconds() * 1_000_000;
    let mut by_person: BTreeMap<String, Vec<&CapturedEvent>> = BTreeMap::new();
    for event in world.events.iter().filter(|e| world.in_range(e, &range)) {
        by_person
            .entry(world.person(&event.distinct_id))
            .or_default()
            .push(event);
    }
    let mut outcomes = Vec::new();
    for (person, mut events) in by_person {
        events.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then(a.uuid.to_string().cmp(&b.uuid.to_string()))
        });
        let matches: Vec<Vec<bool>> = events
            .iter()
            .map(|e| {
                q.series
                    .iter()
                    .map(|node| world.matches(e, node, &q.properties))
                    .collect()
            })
            .collect();
        let mut best: Option<(usize, Vec<i64>)> = None;
        for anchor in 0..events.len() {
            let t0 = us(events[anchor].timestamp);
            let times: Vec<i64> = match q.funnel_order {
                FunnelOrder::Unordered => {
                    if !matches[anchor].iter().any(|m| *m) {
                        continue;
                    }
                    let mut covered: BTreeSet<usize> = BTreeSet::new();
                    let mut times = Vec::new();
                    for (index, event) in events.iter().enumerate().skip(anchor) {
                        if us(event.timestamp) - t0 > window {
                            break;
                        }
                        for (step, matched) in matches[index].iter().enumerate() {
                            if *matched && covered.insert(step) {
                                times.push(us(event.timestamp));
                            }
                        }
                    }
                    times
                }
                FunnelOrder::Ordered | FunnelOrder::Strict => {
                    if !matches[anchor][0] {
                        continue;
                    }
                    let positions = if q.funnel_order == FunnelOrder::Ordered {
                        ordered_search(&events, &matches, anchor, steps, window)
                    } else {
                        let mut positions = vec![anchor];
                        while positions.len() < steps {
                            let next = anchor + positions.len();
                            if next < events.len()
                                && us(events[next].timestamp) - t0 <= window
                                && matches[next][positions.len()]
                            {
                                positions.push(next);
                            } else {
                                break;
                            }
                        }
                        positions
                    };
                    let tainted = q.exclusions.iter().any(|exclusion| {
                        if positions.len() <= exclusion.from_step {
                            return false;
                        }
                        let from = positions[exclusion.from_step];
                        let end = if positions.len() > exclusion.to_step {
                            positions[exclusion.to_step]
                        } else {
                            (from + 1..events.len())
                                .find(|i| us(events[*i].timestamp) - t0 > window)
                                .unwrap_or(events.len())
                        };
                        (from + 1..end).any(|i| events[i].event == exclusion.event)
                    });
                    if tainted {
                        continue;
                    }
                    positions.iter().map(|i| us(events[*i].timestamp)).collect()
                }
            };
            if best.as_ref().is_none_or(|(_, b)| times.len() > b.len()) {
                best = Some((anchor, times));
            }
        }
        let Some((anchor, times)) = best else {
            continue;
        };
        let breakdown = q.breakdown.as_ref().map(|b| match b.source {
            PropertySource::Event => world.breakdown_text(events[anchor], b),
            PropertySource::Person => world
                .person_text(&person, &b.property)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| BREAKDOWN_NONE.to_owned()),
        });
        outcomes.push(Outcome {
            person,
            times,
            breakdown,
        });
    }
    if let Some(breakdown) = &q.breakdown {
        let mut counts: HashMap<String, u64> = HashMap::new();
        for outcome in &outcomes {
            *counts
                .entry(outcome.breakdown.clone().unwrap())
                .or_default() += 1;
        }
        let ranking = rank(counts, breakdown.limit);
        for outcome in &mut outcomes {
            if !ranking.top.contains(outcome.breakdown.as_ref().unwrap()) {
                outcome.breakdown = Some(BREAKDOWN_OTHER.to_owned());
            }
        }
    }
    outcomes
}

fn funnel_steps(q: &FunnelsQuery, members: &[&Outcome]) -> Vec<FunnelStepResult> {
    let n = q.series.len();
    let count = |k: usize| members.iter().filter(|o| o.times.len() > k).count() as u64;
    let ratio = |a: u64, b: u64| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    (0..n)
        .map(|k| {
            let durations: Vec<f64> = if k == 0 {
                Vec::new()
            } else {
                members
                    .iter()
                    .filter(|o| o.times.len() > k)
                    .map(|o| (o.times[k] - o.times[k - 1]) as f64 / 1e6)
                    .collect()
            };
            FunnelStepResult {
                order: k,
                name: q.series[k].label(),
                count: count(k),
                conversion_from_previous: if k == 0 {
                    ratio(count(0), count(0))
                } else {
                    ratio(count(k), count(k - 1))
                },
                conversion_from_start: ratio(count(k), count(0)),
                dropped_off: if k == 0 { 0 } else { count(k - 1) - count(k) },
                average_conversion_time_s: (!durations.is_empty())
                    .then(|| durations.iter().sum::<f64>() / durations.len() as f64),
                median_conversion_time_s: (!durations.is_empty())
                    .then(|| quantile(&durations, 0.5)),
            }
        })
        .collect()
}

pub fn funnels(world: &World<'_>, q: &FunnelsQuery) -> InsightResult {
    let outcomes = funnel_outcomes(world, q);
    let all: Vec<&Outcome> = outcomes.iter().collect();
    let n = q.series.len();
    let durations: Vec<i64> = outcomes
        .iter()
        .filter(|o| o.times.len() == n)
        .map(|o| o.times[n - 1] - o.times[0])
        .collect();
    let mut breakdowns = Vec::new();
    if q.breakdown.is_some() {
        // Order: top values by persons desc, value asc; `$$_other` last.
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for outcome in &outcomes {
            *counts
                .entry(outcome.breakdown.clone().unwrap())
                .or_default() += 1;
        }
        let other = counts.remove(BREAKDOWN_OTHER);
        let mut values: Vec<(String, u64)> = counts.into_iter().collect();
        values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut order: Vec<String> = values.into_iter().map(|(v, _)| v).collect();
        if other.is_some() {
            order.push(BREAKDOWN_OTHER.to_owned());
        }
        for value in order {
            let members: Vec<&Outcome> = outcomes
                .iter()
                .filter(|o| o.breakdown.as_deref() == Some(value.as_str()))
                .collect();
            breakdowns.push(FunnelBreakdownResult {
                breakdown_value: value,
                steps: funnel_steps(q, &members),
            });
        }
    }
    InsightResult::Funnels {
        steps: funnel_steps(q, &all),
        breakdowns,
        time_to_convert: super::funnels::histogram(&durations),
    }
}

pub fn funnel_actors(
    world: &World<'_>,
    q: &FunnelsQuery,
    step: usize,
    converted: bool,
    breakdown_value: Option<&str>,
) -> BTreeSet<String> {
    funnel_outcomes(world, q)
        .into_iter()
        .filter(|o| {
            let depth = o.times.len();
            (if converted {
                depth > step
            } else {
                step > 0 && depth == step
            }) && match (breakdown_value, &q.breakdown) {
                (Some(value), Some(_)) => o.breakdown.as_deref() == Some(value),
                _ => true,
            }
        })
        .map(|o| o.person)
        .collect()
}

// ── Retention ────────────────────────────────────────────────────

/// Cohort starts, their interval, and `cells[cohort][offset]` = persons.
type RetentionCells = (Vec<DateTime<Utc>>, Interval, Vec<Vec<BTreeSet<String>>>);

fn retention_cells(world: &World<'_>, q: &RetentionQuery) -> RetentionCells {
    let interval = match q.period {
        RetentionPeriod::Day => Interval::Day,
        RetentionPeriod::Week => Interval::Week,
        RetentionPeriod::Month => Interval::Month,
    };
    let current = bucket_of(world.now, interval);
    let mut starts = vec![current];
    for _ in 1..q.total_intervals {
        let previous = previous_of(*starts.last().unwrap(), interval);
        starts.push(previous);
    }
    starts.reverse();
    let end = next_of(current, interval);
    let period_of = |event: &CapturedEvent| -> Option<usize> {
        if event.timestamp >= end {
            return None;
        }
        let bucket = bucket_of(event.timestamp, interval);
        starts.iter().position(|s| *s == bucket)
    };
    let n = starts.len();
    let mut members: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    match q.retention_type {
        RetentionType::RetentionRecurring => {
            for event in world.events {
                if world.matches(event, &q.target, &q.properties)
                    && let Some(period) = period_of(event)
                {
                    members[period].insert(world.person(&event.distinct_id));
                }
            }
        }
        RetentionType::RetentionFirstTime => {
            let mut first: HashMap<String, &CapturedEvent> = HashMap::new();
            for event in world.events {
                if event.timestamp < end && world.matches(event, &q.target, &q.properties) {
                    let person = world.person(&event.distinct_id);
                    let entry = first.entry(person).or_insert(event);
                    if event.timestamp < entry.timestamp {
                        *entry = event;
                    }
                }
            }
            for (person, event) in first {
                if let Some(period) = period_of(event) {
                    members[period].insert(person);
                }
            }
        }
    }
    let mut returning: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    for event in world.events {
        if world.matches(event, &q.returning, &q.properties)
            && let Some(period) = period_of(event)
        {
            returning[period].insert(world.person(&event.distinct_id));
        }
    }
    let cells = (0..n)
        .map(|c| {
            (0..n - c)
                .map(|k| {
                    if k == 0 {
                        members[c].clone()
                    } else {
                        members[c]
                            .intersection(&returning[c + k])
                            .cloned()
                            .collect()
                    }
                })
                .collect()
        })
        .collect();
    (starts, interval, cells)
}

pub fn retention(world: &World<'_>, q: &RetentionQuery) -> InsightResult {
    let (starts, interval, cells) = retention_cells(world, q);
    InsightResult::Retention {
        period: q.period,
        cohorts: starts
            .iter()
            .zip(cells)
            .map(|(start, row)| RetentionCohort {
                date: range::rfc3339(us(*start)),
                label: range::label(us(*start), interval),
                size: row[0].len() as u64,
                values: row.iter().map(|cell| cell.len() as u64).collect(),
            })
            .collect(),
    }
}

pub fn retention_actors(
    world: &World<'_>,
    q: &RetentionQuery,
    cohort_date: &str,
    interval: u32,
) -> BTreeSet<String> {
    let (starts, _, cells) = retention_cells(world, q);
    let start = range::parse_instant(cohort_date).unwrap();
    let cohort = starts.iter().position(|s| us(*s) == start).unwrap();
    cells[cohort][interval as usize].clone()
}

// ── Lifecycle ────────────────────────────────────────────────────

/// `cells[k][status]` = persons.
fn lifecycle_cells(
    world: &World<'_>,
    q: &LifecycleQuery,
) -> (ResolvedRange, Vec<i64>, Vec<[BTreeSet<String>; 4]>) {
    let range = world.resolve(&q.date_range, q.interval);
    let buckets = range.buckets().unwrap();
    let mut first: HashMap<String, DateTime<Utc>> = HashMap::new();
    let mut active: HashMap<String, HashSet<i64>> = HashMap::new();
    for event in world.events {
        if us(event.timestamp) >= range.to || !world.matches(event, &q.series, &q.properties) {
            continue;
        }
        let person = world.person(&event.distinct_id);
        let entry = first.entry(person.clone()).or_insert(event.timestamp);
        if event.timestamp < *entry {
            *entry = event.timestamp;
        }
        active
            .entry(person)
            .or_default()
            .insert(us(bucket_of(event.timestamp, q.interval)));
    }
    let previous_first = us(previous_of(at(buckets[0]), q.interval));
    let mut cells: Vec<[BTreeSet<String>; 4]> =
        (0..buckets.len()).map(|_| Default::default()).collect();
    for (person, set) in &active {
        let first_bucket = us(bucket_of(first[person], q.interval));
        for (k, bucket) in buckets.iter().enumerate() {
            let previous = if k == 0 {
                previous_first
            } else {
                buckets[k - 1]
            };
            let now = set.contains(bucket);
            let before = set.contains(&previous);
            let status = match (now, before) {
                (true, _) if first_bucket == *bucket => Some(0),
                (true, true) => Some(1),
                (true, false) => Some(2),
                (false, true) => Some(3),
                (false, false) => None,
            };
            if let Some(status) = status {
                cells[k][status].insert(person.clone());
            }
        }
    }
    (range, buckets, cells)
}

pub fn lifecycle(world: &World<'_>, q: &LifecycleQuery) -> InsightResult {
    let (range, buckets, cells) = lifecycle_cells(world, q);
    let column = |status: usize, sign: i64| -> Vec<i64> {
        cells
            .iter()
            .map(|c| sign * c[status].len() as i64)
            .collect()
    };
    InsightResult::Lifecycle {
        days: buckets.iter().map(|b| range::rfc3339(*b)).collect(),
        labels: buckets
            .iter()
            .map(|b| range::label(*b, range.interval))
            .collect(),
        new: column(0, 1),
        returning: column(1, 1),
        resurrecting: column(2, 1),
        dormant: column(3, -1),
    }
}

pub fn lifecycle_actors(
    world: &World<'_>,
    q: &LifecycleQuery,
    status: LifecycleStatus,
    day: &str,
) -> BTreeSet<String> {
    let (_, buckets, cells) = lifecycle_cells(world, q);
    let day = range::parse_instant(day).unwrap();
    let k = buckets.iter().position(|b| *b == day).unwrap();
    let slot = match status {
        LifecycleStatus::New => 0,
        LifecycleStatus::Returning => 1,
        LifecycleStatus::Resurrecting => 2,
        LifecycleStatus::Dormant => 3,
    };
    cells[k][slot].clone()
}

// ── Stickiness ───────────────────────────────────────────────────

fn sticky_counts(
    world: &World<'_>,
    q: &StickinessQuery,
    node: &EventNode,
) -> (usize, HashMap<String, usize>) {
    let range = world.resolve(&q.date_range, q.interval);
    let buckets = range.buckets().unwrap().len();
    let mut active: HashMap<String, HashSet<DateTime<Utc>>> = HashMap::new();
    for event in world.events {
        if world.in_range(event, &range) && world.matches(event, node, &q.properties) {
            active
                .entry(world.person(&event.distinct_id))
                .or_default()
                .insert(bucket_of(event.timestamp, q.interval));
        }
    }
    (
        buckets,
        active.into_iter().map(|(p, s)| (p, s.len())).collect(),
    )
}

pub fn stickiness(world: &World<'_>, q: &StickinessQuery) -> InsightResult {
    let unit = match q.interval {
        Interval::Hour => "hour",
        Interval::Day => "day",
        Interval::Week => "week",
        Interval::Month => "month",
    };
    let series = q
        .series
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let (buckets, counts) = sticky_counts(world, q, node);
            let mut data = vec![0_u64; buckets];
            for n in counts.values() {
                data[n - 1] += 1;
            }
            StickinessSeries {
                label: node.label(),
                series_index: index,
                data,
                labels: (1..=buckets)
                    .map(|n| format!("{n} {unit}{}", if n == 1 { "" } else { "s" }))
                    .collect(),
            }
        })
        .collect();
    InsightResult::Stickiness { series }
}

pub fn stickiness_actors(
    world: &World<'_>,
    q: &StickinessQuery,
    series_index: usize,
    intervals: u32,
) -> BTreeSet<String> {
    let (_, counts) = sticky_counts(world, q, &q.series[series_index]);
    counts
        .into_iter()
        .filter(|(_, n)| *n == intervals as usize)
        .map(|(p, _)| p)
        .collect()
}

// ── Paths ────────────────────────────────────────────────────────

pub fn paths(world: &World<'_>, q: &PathsQuery) -> InsightResult {
    let range = world.resolve(&q.date_range, Interval::Day);
    let mut by_person: BTreeMap<String, Vec<&CapturedEvent>> = BTreeMap::new();
    for event in world.events {
        if world.in_range(event, &range) && world.passes(event, &q.properties) {
            by_person
                .entry(world.person(&event.distinct_id))
                .or_default()
                .push(event);
        }
    }
    let node_of = |event: &CapturedEvent| -> Option<String> {
        let pageview = event.event == "$pageview";
        let custom = !event.event.starts_with('$');
        match q.paths_type {
            PathsType::Pageviews if pageview => event_text(event, "$pathname"),
            PathsType::CustomEvents if custom => Some(event.event.clone()),
            PathsType::All if pageview => event_text(event, "$pathname"),
            PathsType::All if custom => Some(event.event.clone()),
            _ => None,
        }
    };
    let mut links: BTreeMap<(usize, String, String), (u64, i64)> = BTreeMap::new();
    for (_, mut events) in by_person {
        events.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then(a.uuid.to_string().cmp(&b.uuid.to_string()))
        });
        let mut path: Vec<(String, i64)> = Vec::new();
        for event in events {
            if let Some(node) = node_of(event)
                && path.last().map(|(n, _)| n) != Some(&node)
            {
                path.push((node, us(event.timestamp)));
            }
        }
        if let Some(start) = &q.start_point {
            let Some(i) = path.iter().position(|(n, _)| n == start) else {
                continue;
            };
            path = path[i..].to_vec();
        }
        if let Some(end) = &q.end_point {
            let Some(i) = path.iter().position(|(n, _)| n == end) else {
                continue;
            };
            path = path[..=i].to_vec();
        }
        path.truncate(q.step_limit as usize);
        for i in 0..path.len().saturating_sub(1) {
            let entry = links
                .entry((i, path[i].0.clone(), path[i + 1].0.clone()))
                .or_default();
            entry.0 += 1;
            entry.1 += path[i + 1].1 - path[i].1;
        }
    }
    let mut out: Vec<PathLink> = links
        .into_iter()
        .map(|((i, a, b), (value, total))| PathLink {
            source: format!("{}_{a}", i + 1),
            target: format!("{}_{b}", i + 2),
            value,
            average_conversion_time_s: total as f64 / value as f64 / 1e6,
        })
        .collect();
    out.sort_by(|a, b| {
        b.value
            .cmp(&a.value)
            .then(a.source.cmp(&b.source))
            .then(a.target.cmp(&b.target))
    });
    out.truncate(q.edge_limit as usize);
    InsightResult::Paths { links: out }
}

/// Persons with any event in the project (for SQL cross-checks).
pub fn distinct_persons(world: &World<'_>) -> usize {
    world
        .events
        .iter()
        .map(|e| world.person(&e.distinct_id))
        .collect::<HashSet<_>>()
        .len()
}
