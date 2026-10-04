//! Engine == oracle on seeded random datasets, plus targeted regressions.
//!
//! Each dataset has identify/alias/merge events (so persons differ from
//! distinct ids), sessions, `$set` person properties, hostile property keys
//! and values, numeric and non-numeric strings, and timestamps on (and one
//! microsecond before) hour/day/week/month boundaries and with ties. Every
//! kind runs through the real engine (DuckDB over Parquet written by
//! `lake::parquet::write_file`, identity from a real projection) and is
//! compared field by field with the brute-force oracle; actor drill-downs
//! are compared too and reconciled with the numbers they open.
//!
//! `HOGLET_ORACLE_SEEDS=<n>` raises the number of datasets per kind.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::oracle::{self, World};
use super::{EngineConfig, QueryEngine, QueryError};
use crate::capture::event::CapturedEvent;
use crate::contract::common::*;
use crate::contract::insight::*;
use crate::persons::PersonStore;
use crate::pipeline::wal::WalCursor;
use crate::source::DirectorySource;

pub(crate) const PROJECT: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_PROJECT: &str = "22222222-2222-4222-8222-222222222222";

pub(crate) fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 15, 12, 34, 56).unwrap()
}

pub(crate) struct Fixture {
    pub dir: tempfile::TempDir,
    pub events: Vec<CapturedEvent>,
    pub engine: QueryEngine,
    pub person_of: HashMap<String, String>,
    pub person_props: HashMap<String, Map<String, Value>>,
    applied: usize,
}

fn projections_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("projections.db")
}

impl Fixture {
    pub fn new(events: Vec<CapturedEvent>) -> Self {
        Self::with_config(events, test_config())
    }

    pub fn with_config(events: Vec<CapturedEvent>, config: EngineConfig) -> Self {
        Self::build(events, config)
    }

    fn build(events: Vec<CapturedEvent>, config: EngineConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let connection = rusqlite::Connection::open(projections_path(&dir)).unwrap();
        crate::projections::initialize_schema(&connection).unwrap();
        drop(connection);
        let persons = Arc::new(PersonStore::open(&projections_path(&dir)).unwrap());
        let source = Arc::new(DirectorySource::new(dir.path().join("lake")));
        let engine = QueryEngine::new(source, persons, config).unwrap();
        let mut fixture = Self {
            dir,
            events: Vec::new(),
            engine,
            person_of: HashMap::new(),
            person_props: HashMap::new(),
            applied: 0,
        };
        fixture.append(events);
        // Another project's events must never leak into this one.
        let mut other = dataset(99, 60);
        for event in &mut other {
            event.event = "purchase".into();
        }
        fixture.write_files(OTHER_PROJECT, &other);
        fixture
    }

    fn write_files(&self, project: &str, events: &[CapturedEvent]) {
        let source = DirectorySource::new(self.dir.path().join("lake"));
        let mut by_day: std::collections::BTreeMap<chrono::NaiveDate, Vec<CapturedEvent>> =
            Default::default();
        for event in events {
            by_day
                .entry(event.timestamp.date_naive())
                .or_default()
                .push(event.clone());
        }
        for (day, events) in by_day {
            let directory = source.partition_dir(project, day);
            std::fs::create_dir_all(&directory).unwrap();
            // Two files per partition where possible: readers union them.
            let half = events.len().div_ceil(2);
            for chunk in events.chunks(half.max(1)) {
                let path = directory.join(format!("{}.parquet", Uuid::new_v4()));
                crate::lake::parquet::write_file(chunk, &path).unwrap();
            }
        }
    }

    /// Append events: new files plus projection updates, in vector order.
    pub fn append(&mut self, events: Vec<CapturedEvent>) {
        self.write_files(PROJECT, &events);
        let mut connection = rusqlite::Connection::open(projections_path(&self.dir)).unwrap();
        let transaction = connection.transaction().unwrap();
        for event in &events {
            self.applied += 1;
            crate::projections::apply_captured_event(
                &transaction,
                PROJECT,
                event,
                WalCursor::new(1, self.applied as u64),
            )
            .unwrap();
        }
        transaction.commit().unwrap();
        self.person_of = connection
            .prepare("SELECT distinct_id, person_id FROM distinct_ids WHERE project_id = ?1")
            .unwrap()
            .query_map([PROJECT], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        self.person_props = connection
            .prepare("SELECT id, properties FROM persons WHERE project_id = ?1")
            .unwrap()
            .query_map([PROJECT], |row| {
                let encoded: String = row.get(1)?;
                Ok((
                    row.get(0)?,
                    serde_json::from_str::<Map<String, Value>>(&encoded).unwrap(),
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        self.events.extend(events);
    }

    pub fn world(&self) -> World<'_> {
        World {
            events: &self.events,
            person_of: &self.person_of,
            person_props: &self.person_props,
            now: now(),
        }
    }

    pub fn run(&self, query: &InsightQuery) -> Result<InsightResult, QueryError> {
        let request = QueryRequest {
            query: query.clone(),
            refresh: false,
        };
        let first = self.engine.run_at(PROJECT, &request, now())?;
        // The second run must come from the cache and say so.
        let second = self.engine.run_at(PROJECT, &request, now())?;
        if !matches!(query, InsightQuery::SqlQuery(_)) {
            assert!(second.meta.cached, "repeat query was not cached");
        }
        assert_eq!(
            serde_json::to_value(&first.result).unwrap(),
            serde_json::to_value(&second.result).unwrap()
        );
        Ok(first.result)
    }

    pub fn actors(&self, query: &InsightQuery, selection: &ActorSelection) -> BTreeSet<String> {
        let ids = self
            .engine
            .actor_ids_at(PROJECT, query, selection, now())
            .unwrap();
        let set: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(set.len(), ids.len(), "duplicate actors");
        set
    }
}

pub(crate) fn test_config() -> EngineConfig {
    EngineConfig {
        memory_limit_mb: 256,
        threads: 2,
        connections: 2,
        timeout: std::time::Duration::from_secs(120),
        // Several person partitions even on small datasets.
        partition_rows: 120,
        ..EngineConfig::default()
    }
}

// ── Datasets ─────────────────────────────────────────────────────

const EVENTS: &[&str] = &[
    "$pageview",
    "$pageview",
    "$pageview",
    "signup",
    "purchase",
    "click",
    "Weird 'name\" ü",
];
pub(crate) const HOSTILE_KEY: &str = "'); DROP TABLE events; --";
pub(crate) const QUOTE_KEY: &str = "$.a\"b";

fn pick<'a, T>(rng: &mut StdRng, items: &'a [T]) -> &'a T {
    items.choose(rng).unwrap()
}

fn timestamp(rng: &mut StdRng) -> DateTime<Utc> {
    let start = now() - Duration::days(45);
    let span = (now() + Duration::hours(2) - start).num_seconds();
    let base = start + Duration::seconds(rng.gen_range(0..span));
    match rng.gen_range(0..10) {
        // Exactly on a day boundary, or a microsecond before it.
        0 => Utc.from_utc_datetime(&base.date_naive().and_hms_opt(0, 0, 0).unwrap()),
        1 => {
            Utc.from_utc_datetime(&base.date_naive().and_hms_opt(0, 0, 0).unwrap())
                - Duration::microseconds(1)
        }
        // Monday midnight / month start.
        2 => {
            let date = base.date_naive();
            let monday = date - Duration::days(i64::from(date.weekday().num_days_from_monday()));
            Utc.from_utc_datetime(&monday.and_hms_opt(0, 0, 0).unwrap())
        }
        3 => {
            let date = base.date_naive().with_day(1).unwrap();
            Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
                - Duration::microseconds(rng.gen_range(0..2))
        }
        // Hour boundary.
        4 => {
            base - Duration::seconds(i64::from(chrono::Timelike::second(&base)))
                - Duration::minutes(i64::from(chrono::Timelike::minute(&base)))
        }
        _ => base + Duration::microseconds(rng.gen_range(0..1_000_000)),
    }
}

/// A seeded random dataset of roughly `size` events.
pub(crate) fn dataset(seed: u64, size: usize) -> Vec<CapturedEvent> {
    let mut rng = StdRng::seed_from_u64(seed);
    let anonymous: Vec<String> = (0..rng.gen_range(8..20))
        .map(|i| format!("anon-{i}"))
        .collect();
    let users: Vec<String> = (0..rng.gen_range(3..7))
        .map(|i| format!("user-{i}"))
        .collect();
    let mut events: Vec<CapturedEvent> = Vec::new();
    for _ in 0..size {
        let mut properties = Map::new();
        let identify = rng.gen_bool(0.08);
        let (name, distinct_id) = if identify {
            let user = pick(&mut rng, &users).clone();
            let anon = pick(&mut rng, &anonymous).clone();
            match rng.gen_range(0..10) {
                0 => {
                    properties.insert("alias".into(), json!(anon));
                    ("$create_alias".to_owned(), user)
                }
                1 => {
                    properties.insert("alias".into(), json!(pick(&mut rng, &users)));
                    ("$merge_dangerously".to_owned(), user)
                }
                _ => {
                    properties.insert("$anon_distinct_id".into(), json!(anon));
                    ("$identify".to_owned(), user)
                }
            }
        } else {
            let distinct = if rng.gen_bool(0.7) {
                pick(&mut rng, &anonymous).clone()
            } else {
                pick(&mut rng, &users).clone()
            };
            (pick(&mut rng, EVENTS).to_string(), distinct)
        };
        let mut ts = timestamp(&mut rng);
        if rng.gen_bool(0.05)
            && let Some(previous) = events.iter().rev().find(|e| e.distinct_id == distinct_id)
        {
            ts = previous.timestamp; // a tie, broken by uuid
        }
        if rng.gen_bool(0.85) {
            let session = match rng.gen_range(0..20) {
                0 => json!(""),
                1 => json!(7),
                n => json!(format!("s{}", n % 12)),
            };
            properties.insert("$session_id".into(), session);
        }
        if rng.gen_bool(0.85) {
            properties.insert(
                "$pathname".into(),
                json!(pick(
                    &mut rng,
                    &["/", "/pricing", "/signup", "/docs", "/blog/a"]
                )),
            );
        }
        if rng.gen_bool(0.75) {
            let browser = match rng.gen_range(0..12) {
                0 => json!(7),
                1 => json!(""),
                _ => json!(pick(&mut rng, &["Chrome", "Firefox", "Safari"])),
            };
            properties.insert("$browser".into(), browser);
        }
        if rng.gen_bool(0.8) {
            let plan = match rng.gen_range(0..10) {
                0 => json!(3),
                1 => json!(true),
                2 => json!(""),
                3 => Value::Null,
                _ => json!(pick(&mut rng, &["free", "pro", "Pro", "enterprise"])),
            };
            properties.insert("plan".into(), plan);
        }
        if rng.gen_bool(0.8) {
            let amount = match rng.gen_range(0..12) {
                0 => json!("12.5"),
                1 => json!("abc"),
                2 => json!(true),
                3 => json!(-4),
                4 => json!(" 7"),
                5 => json!(2.25),
                6 => json!("1e2"),
                _ => json!(rng.gen_range(0..100)),
            };
            properties.insert("amount".into(), amount);
        }
        if rng.gen_bool(0.3) {
            properties.insert(
                HOSTILE_KEY.into(),
                json!(pick(
                    &mut rng,
                    &["x", "y'z", "ünï", "'); DROP TABLE events; --"]
                )),
            );
        }
        if rng.gen_bool(0.3) {
            properties.insert(QUOTE_KEY.into(), json!(pick(&mut rng, &["q", "r"])));
        }
        if rng.gen_bool(0.2) {
            properties.insert(
                "ключ".into(),
                json!(pick(&mut rng, &["значение", "другое"])),
            );
        }
        if rng.gen_bool(0.3) {
            properties.insert(
                "seen_at".into(),
                json!(pick(
                    &mut rng,
                    &[
                        "2026-03-01T10:00:00Z",
                        "2026-02-20",
                        "2026-03-10T00:00:00+02:00",
                        "junk",
                        "12"
                    ]
                )),
            );
        }
        if rng.gen_bool(0.2) {
            properties.insert("a/b~c".into(), json!(rng.gen_range(1..4)));
        }
        if rng.gen_bool(0.15) {
            let mut set = Map::new();
            set.insert(
                "plan".into(),
                json!(pick(&mut rng, &["free", "pro", "Pro"])),
            );
            if rng.gen_bool(0.5) {
                set.insert("email".into(), json!(format!("{distinct_id}@example.com")));
            }
            set.insert("age".into(), json!(rng.gen_range(18..60)));
            properties.insert("$set".into(), Value::Object(set));
        }
        events.push(CapturedEvent {
            uuid: Uuid::from_u128(rng.r#gen()),
            event: name,
            distinct_id,
            token: "phc_test".into(),
            timestamp: ts,
            properties,
        });
    }
    // WAL order: mostly by time, with a little disorder.
    events.sort_by_key(|e| e.timestamp);
    for _ in 0..size / 20 {
        let a = rng.gen_range(0..events.len());
        let b = rng.gen_range(0..events.len());
        events.swap(a, b);
    }
    events
}

// ── Random queries ───────────────────────────────────────────────

fn filter(
    key: &str,
    source: PropertySource,
    operator: PropertyOperator,
    value: Value,
) -> PropertyFilter {
    PropertyFilter {
        key: key.into(),
        source,
        operator,
        value,
    }
}

fn random_filter(rng: &mut StdRng) -> PropertyFilter {
    use PropertyOperator::*;
    use PropertySource::{Event as E, Person as P};
    let all = [
        filter("plan", E, Exact, json!("pro")),
        filter("plan", E, Exact, json!(["pro", "free"])),
        filter("plan", E, IsNot, json!("free")),
        filter("plan", E, Icontains, json!("PR")),
        filter("plan", E, NotIcontains, json!("o")),
        filter("plan", E, Regex, json!("^p")),
        filter("plan", E, NotRegex, json!("e$")),
        filter("plan", E, IsSet, Value::Null),
        filter("plan", E, IsNotSet, Value::Null),
        filter("plan", E, Exact, json!(3)),
        filter("plan", E, Exact, json!(true)),
        filter("amount", E, Gt, json!(10)),
        filter("amount", E, Lte, json!("50")),
        filter("amount", E, Lt, json!(0)),
        filter(HOSTILE_KEY, E, Exact, json!("y'z")),
        filter(HOSTILE_KEY, E, Icontains, json!("'); drop")),
        filter(QUOTE_KEY, E, Exact, json!("q")),
        filter("ключ", E, Exact, json!("значение")),
        filter("seen_at", E, IsDateBefore, json!("2026-03-01")),
        filter("seen_at", E, IsDateAfter, json!("2026-02-25T00:00:00Z")),
        filter("$browser", E, Exact, json!("Chrome")),
        filter("$browser", E, IsNot, json!("Firefox")),
        filter("$browser", E, Exact, json!(7)),
        filter("$pathname", E, Regex, json!("^/p")),
        filter("$session_id", E, IsSet, Value::Null),
        filter("a/b~c", E, Gte, json!(2)),
        filter("plan", P, Exact, json!("pro")),
        filter("plan", P, IsNot, json!("pro")),
        filter("plan", P, IsSet, Value::Null),
        filter("plan", P, Icontains, json!("PR")),
        filter("email", P, Icontains, json!("user-1")),
        filter("email", P, IsNotSet, Value::Null),
        filter("age", P, Gt, json!(30)),
    ];
    pick(rng, &all).clone()
}

fn random_filters(rng: &mut StdRng, probability: f64) -> Vec<PropertyFilter> {
    if !rng.gen_bool(probability) {
        return Vec::new();
    }
    (0..rng.gen_range(1..3))
        .map(|_| random_filter(rng))
        .collect()
}

fn random_event(rng: &mut StdRng) -> Option<String> {
    let all = [
        Some("$pageview"),
        Some("signup"),
        Some("purchase"),
        Some("click"),
        Some("Weird 'name\" ü"),
        Some("$identify"),
        None,
    ];
    pick(rng, &all).map(str::to_owned)
}

fn node(rng: &mut StdRng, math: Math) -> EventNode {
    EventNode {
        event: random_event(rng),
        custom_name: None,
        properties: random_filters(rng, 0.3),
        math,
        math_property: Some("amount".into()),
    }
}

fn random_range(rng: &mut StdRng, hourly_ok: bool) -> (DateRange, Interval) {
    let mut all = vec![
        ("-7d", None, Interval::Day),
        ("-30d", None, Interval::Day),
        ("-14d", None, Interval::Week),
        ("2026-02-01", Some("2026-03-15"), Interval::Week),
        ("mStart", None, Interval::Day),
        ("-2m", None, Interval::Month),
        ("all", None, Interval::Week),
        ("all", None, Interval::Day),
        (
            "2026-03-01T06:00:00Z",
            Some("2026-03-10T18:30:00Z"),
            Interval::Day,
        ),
        ("yStart", None, Interval::Month),
        ("-45d", None, Interval::Day),
        ("2026-02-10", Some("2026-02-28"), Interval::Day),
    ];
    if hourly_ok {
        all.push(("-24h", None, Interval::Hour));
        all.push(("-3d", None, Interval::Hour));
        all.push((
            "2026-03-14T22:00:00Z",
            Some("2026-03-15T03:30:00Z"),
            Interval::Hour,
        ));
    }
    let (from, to, interval) = *pick(rng, &all);
    (
        DateRange {
            date_from: from.into(),
            date_to: to.map(Into::into),
        },
        interval,
    )
}

fn random_breakdown(rng: &mut StdRng, probability: f64) -> Option<Breakdown> {
    if !rng.gen_bool(probability) {
        return None;
    }
    let all = [
        ("plan", PropertySource::Event),
        ("$browser", PropertySource::Event),
        (HOSTILE_KEY, PropertySource::Event),
        ("amount", PropertySource::Event),
        ("$pathname", PropertySource::Event),
        ("plan", PropertySource::Person),
        ("email", PropertySource::Person),
    ];
    let (property, source) = *pick(rng, &all);
    Some(Breakdown {
        property: property.into(),
        source,
        limit: rng.gen_range(1..5),
    })
}

fn random_trends(rng: &mut StdRng) -> TrendsQuery {
    let maths = [
        Math::Total,
        Math::Dau,
        Math::WeeklyActive,
        Math::MonthlyActive,
        Math::UniqueSession,
        Math::Sum,
        Math::Avg,
        Math::Min,
        Math::Max,
        Math::Median,
        Math::P90,
        Math::P95,
        Math::P99,
    ];
    let count = rng.gen_range(1..4);
    let series: Vec<EventNode> = (0..count)
        .map(|_| {
            let math = *pick(rng, &maths);
            node(rng, math)
        })
        .collect();
    let (date_range, interval) = random_range(rng, true);
    let formula = (count >= 2 && rng.gen_bool(0.2))
        .then(|| pick(rng, &["A + B", "A / B * 100", "(A - B) / 2", "-A + 2 * B"]).to_string());
    TrendsQuery {
        series,
        date_range,
        interval,
        properties: random_filters(rng, 0.3),
        breakdown: random_breakdown(rng, 0.4),
        formula,
        compare: rng.gen_bool(0.2),
        display: ChartDisplay::default(),
    }
}

fn random_funnel(rng: &mut StdRng) -> FunnelsQuery {
    let steps = rng.gen_range(2..5);
    let series: Vec<EventNode> = (0..steps)
        .map(|_| EventNode {
            event: pick(
                rng,
                &[
                    Some("$pageview"),
                    Some("signup"),
                    Some("purchase"),
                    Some("click"),
                    None,
                ],
            )
            .map(str::to_owned),
            custom_name: None,
            properties: random_filters(rng, 0.2),
            math: Math::Total,
            math_property: None,
        })
        .collect();
    let funnel_order = *pick(
        rng,
        &[
            FunnelOrder::Ordered,
            FunnelOrder::Strict,
            FunnelOrder::Unordered,
        ],
    );
    let funnel_window = match rng.gen_range(0..4) {
        0 => FunnelWindow {
            interval: rng.gen_range(1..48),
            unit: WindowUnit::Hour,
        },
        1 => FunnelWindow {
            interval: rng.gen_range(1..14),
            unit: WindowUnit::Day,
        },
        2 => FunnelWindow {
            interval: rng.gen_range(30..600),
            unit: WindowUnit::Minute,
        },
        _ => FunnelWindow {
            interval: rng.gen_range(1..3),
            unit: WindowUnit::Week,
        },
    };
    let mut exclusions = Vec::new();
    if funnel_order != FunnelOrder::Unordered && rng.gen_bool(0.3) {
        let to_step = rng.gen_range(1..steps);
        exclusions.push(FunnelExclusion {
            event: pick(rng, &["click", "$pageview", "purchase"]).to_string(),
            from_step: rng.gen_range(0..to_step),
            to_step,
        });
    }
    let (date_range, _) = random_range(rng, false);
    FunnelsQuery {
        series,
        date_range,
        properties: random_filters(rng, 0.2),
        breakdown: random_breakdown(rng, 0.3),
        funnel_window,
        funnel_order,
        exclusions,
    }
}

fn random_retention(rng: &mut StdRng) -> RetentionQuery {
    RetentionQuery {
        target: node(rng, Math::Total),
        returning: node(rng, Math::Total),
        period: *pick(
            rng,
            &[
                RetentionPeriod::Day,
                RetentionPeriod::Week,
                RetentionPeriod::Month,
            ],
        ),
        total_intervals: rng.gen_range(2..11),
        retention_type: *pick(
            rng,
            &[
                RetentionType::RetentionRecurring,
                RetentionType::RetentionFirstTime,
            ],
        ),
        properties: random_filters(rng, 0.2),
    }
}

fn random_lifecycle(rng: &mut StdRng) -> LifecycleQuery {
    let (date_range, interval) = random_range(rng, true);
    LifecycleQuery {
        series: node(rng, Math::Total),
        date_range,
        interval,
        properties: random_filters(rng, 0.2),
    }
}

fn random_stickiness(rng: &mut StdRng) -> StickinessQuery {
    let (date_range, interval) = random_range(rng, true);
    StickinessQuery {
        series: (0..rng.gen_range(1..3))
            .map(|_| node(rng, Math::Total))
            .collect(),
        date_range,
        interval,
        properties: random_filters(rng, 0.2),
    }
}

fn random_paths(rng: &mut StdRng) -> PathsQuery {
    let nodes = [
        "/", "/pricing", "/signup", "/docs", "signup", "click", "purchase",
    ];
    let (date_range, _) = random_range(rng, false);
    PathsQuery {
        paths_type: *pick(
            rng,
            &[
                PathsType::Pageviews,
                PathsType::CustomEvents,
                PathsType::All,
            ],
        ),
        start_point: rng.gen_bool(0.3).then(|| pick(rng, &nodes).to_string()),
        end_point: rng.gen_bool(0.2).then(|| pick(rng, &nodes).to_string()),
        step_limit: rng.gen_range(2..7),
        edge_limit: rng.gen_range(1..30),
        date_range,
        properties: random_filters(rng, 0.2),
    }
}

// ── Comparison ───────────────────────────────────────────────────

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn compare(path: &str, engine: &Value, oracle: &Value) -> Result<(), String> {
    match (engine, oracle) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if close(a, b) {
                Ok(())
            } else {
                Err(format!("{path}: engine {a} != oracle {b}"))
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Err(format!(
                    "{path}: engine has {} items, oracle {}",
                    a.len(),
                    b.len()
                ));
            }
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                compare(&format!("{path}[{index}]"), a, b)?;
            }
            Ok(())
        }
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let other = b
                    .get(key)
                    .ok_or_else(|| format!("{path}.{key}: missing in oracle"))?;
                compare(&format!("{path}.{key}"), value, other)?;
            }
            if a.len() != b.len() {
                return Err(format!("{path}: key sets differ"));
            }
            Ok(())
        }
        (a, b) if a == b => Ok(()),
        (a, b) => Err(format!("{path}: engine {a} != oracle {b}")),
    }
}

fn assert_same(
    context: &str,
    query: &InsightQuery,
    engine: &InsightResult,
    oracle: &InsightResult,
) {
    let engine = serde_json::to_value(engine).unwrap();
    let oracle = serde_json::to_value(oracle).unwrap();
    if let Err(difference) = compare("result", &engine, &oracle) {
        panic!(
            "{context}: engine != oracle at {difference}\nquery: {}\nengine: {engine}\noracle: {oracle}",
            serde_json::to_string(query).unwrap()
        );
    }
}

fn seeds() -> u64 {
    std::env::var("HOGLET_ORACLE_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12)
}

const QUERIES_PER_DATASET: usize = 10;

/// Run `make` against the oracle for every seed; `check_actors` drills down.
fn property_test(
    salt: u64,
    make: impl Fn(&mut StdRng) -> InsightQuery,
    oracle_result: impl Fn(&World<'_>, &InsightQuery) -> InsightResult,
    check_actors: impl Fn(&mut StdRng, &Fixture, &InsightQuery, &InsightResult),
) -> usize {
    property_test_on(
        salt,
        seeds(),
        Fixture::new,
        make,
        oracle_result,
        check_actors,
    )
}

fn property_test_on(
    salt: u64,
    seeds: u64,
    fixture: impl Fn(Vec<CapturedEvent>) -> Fixture,
    make: impl Fn(&mut StdRng) -> InsightQuery,
    oracle_result: impl Fn(&World<'_>, &InsightQuery) -> InsightResult,
    check_actors: impl Fn(&mut StdRng, &Fixture, &InsightQuery, &InsightResult),
) -> usize {
    let mut cases = 0;
    let mut informative = 0;
    for seed in 0..seeds {
        let mut rng = StdRng::seed_from_u64(seed * 1_000 + salt);
        let fixture = fixture(dataset(seed, rng.gen_range(150..450)));
        for case in 0..QUERIES_PER_DATASET {
            let query = make(&mut rng);
            let context = format!("seed {seed} case {case}");
            let engine = fixture.run(&query).unwrap_or_else(|error| {
                panic!(
                    "{context}: engine failed: {error:?} for {}",
                    serde_json::to_string(&query).unwrap()
                )
            });
            let expected = oracle_result(&fixture.world(), &query);
            assert_same(&context, &query, &engine, &expected);
            check_actors(&mut rng, &fixture, &query, &engine);
            cases += 1;
            if nontrivial(&engine) {
                informative += 1;
            }
        }
    }
    // A comparison of empty results proves nothing: most cases must have
    // data.
    assert!(
        informative * 2 >= cases,
        "only {informative} of {cases} cases had non-empty results"
    );
    cases
}

fn nontrivial(result: &InsightResult) -> bool {
    match result {
        InsightResult::Trends { series } => series.iter().any(|s| s.data.iter().any(|v| *v != 0.0)),
        InsightResult::Funnels { steps, .. } => steps.first().is_some_and(|s| s.count > 0),
        InsightResult::Retention { cohorts, .. } => cohorts.iter().any(|c| c.size > 0),
        InsightResult::Lifecycle {
            new,
            returning,
            resurrecting,
            dormant,
            ..
        } => new
            .iter()
            .chain(returning)
            .chain(resurrecting)
            .chain(dormant)
            .any(|v| *v != 0),
        InsightResult::Stickiness { series } => {
            series.iter().any(|s| s.data.iter().any(|v| *v > 0))
        }
        InsightResult::Paths { links } => !links.is_empty(),
        InsightResult::Sql { rows, .. } => !rows.is_empty(),
    }
}

fn assert_actors(
    query: &InsightQuery,
    selection: &ActorSelection,
    engine: &BTreeSet<String>,
    oracle: &BTreeSet<String>,
) {
    assert_eq!(
        engine,
        oracle,
        "actors differ for {selection:?} in {}",
        serde_json::to_string(query).unwrap()
    );
}

#[test]
fn trends_match_the_oracle() {
    let cases = property_test(
        1,
        |rng| InsightQuery::TrendsQuery(random_trends(rng)),
        |world, query| match query {
            InsightQuery::TrendsQuery(q) => oracle::trends(world, q),
            _ => unreachable!(),
        },
        |rng, fixture, query, result| {
            let (InsightQuery::TrendsQuery(q), InsightResult::Trends { series }) = (query, result)
            else {
                unreachable!()
            };
            let current: Vec<&TrendSeries> = series
                .iter()
                .filter(|s| s.compare.as_deref() != Some("previous"))
                .collect();
            for _ in 0..2 {
                let Some(s) = current.choose(rng) else { return };
                let Some(series_index) = s.series_index else {
                    continue;
                };
                let bucket = rng.gen_range(0..s.days.len());
                let selection = ActorSelection::TrendsPoint {
                    series_index,
                    day: s.days[bucket].clone(),
                    breakdown_value: s.breakdown_value.clone(),
                };
                let engine = fixture.actors(query, &selection);
                let expected = oracle::trends_actors(
                    &fixture.world(),
                    q,
                    series_index,
                    &s.days[bucket],
                    s.breakdown_value.as_deref(),
                );
                assert_actors(query, &selection, &engine, &expected);
                if matches!(
                    q.series[series_index].math,
                    Math::Dau | Math::WeeklyActive | Math::MonthlyActive
                ) {
                    assert_eq!(
                        engine.len() as f64,
                        s.data[bucket],
                        "actors reconcile with the point"
                    );
                }
            }
        },
    );
    assert_eq!(cases as u64, seeds() * QUERIES_PER_DATASET as u64);
}

#[test]
fn funnels_match_the_oracle() {
    property_test(
        2,
        |rng| InsightQuery::FunnelsQuery(random_funnel(rng)),
        |world, query| match query {
            InsightQuery::FunnelsQuery(q) => oracle::funnels(world, q),
            _ => unreachable!(),
        },
        |rng, fixture, query, result| {
            let (
                InsightQuery::FunnelsQuery(q),
                InsightResult::Funnels {
                    steps, breakdowns, ..
                },
            ) = (query, result)
            else {
                unreachable!()
            };
            for _ in 0..2 {
                let step = rng.gen_range(0..q.series.len());
                let converted = rng.gen_bool(0.5);
                let breakdown = breakdowns.choose(rng).filter(|_| rng.gen_bool(0.5));
                let selection = ActorSelection::FunnelStep {
                    step,
                    converted,
                    breakdown_value: breakdown.map(|b| b.breakdown_value.clone()),
                };
                let engine = fixture.actors(query, &selection);
                let expected = oracle::funnel_actors(
                    &fixture.world(),
                    q,
                    step,
                    converted,
                    breakdown.map(|b| b.breakdown_value.as_str()),
                );
                assert_actors(query, &selection, &engine, &expected);
                let cell = &breakdown.map(|b| &b.steps).unwrap_or(steps)[step];
                let expected_count = if converted {
                    cell.count
                } else {
                    cell.dropped_off
                };
                assert_eq!(
                    engine.len() as u64,
                    expected_count,
                    "funnel actors reconcile"
                );
            }
        },
    );
}

#[test]
fn retention_matches_the_oracle() {
    property_test(
        3,
        |rng| InsightQuery::RetentionQuery(random_retention(rng)),
        |world, query| match query {
            InsightQuery::RetentionQuery(q) => oracle::retention(world, q),
            _ => unreachable!(),
        },
        |rng, fixture, query, result| {
            let (InsightQuery::RetentionQuery(q), InsightResult::Retention { cohorts, .. }) =
                (query, result)
            else {
                unreachable!()
            };
            for cohort in cohorts {
                assert_eq!(cohort.values[0], cohort.size);
            }
            let cohort = cohorts.choose(rng).unwrap();
            let interval = rng.gen_range(0..cohort.values.len()) as u32;
            let selection = ActorSelection::RetentionCell {
                cohort_date: cohort.date.clone(),
                interval,
            };
            let engine = fixture.actors(query, &selection);
            let expected = oracle::retention_actors(&fixture.world(), q, &cohort.date, interval);
            assert_actors(query, &selection, &engine, &expected);
            assert_eq!(engine.len() as u64, cohort.values[interval as usize]);
        },
    );
}

#[test]
fn lifecycle_matches_the_oracle() {
    property_test(
        4,
        |rng| InsightQuery::LifecycleQuery(random_lifecycle(rng)),
        |world, query| match query {
            InsightQuery::LifecycleQuery(q) => oracle::lifecycle(world, q),
            _ => unreachable!(),
        },
        |rng, fixture, query, result| {
            let (
                InsightQuery::LifecycleQuery(q),
                InsightResult::Lifecycle {
                    days,
                    new,
                    returning,
                    resurrecting,
                    dormant,
                    ..
                },
            ) = (query, result)
            else {
                unreachable!()
            };
            for _ in 0..2 {
                let k = rng.gen_range(0..days.len());
                let (status, count) = *pick(
                    rng,
                    &[
                        (LifecycleStatus::New, new[k]),
                        (LifecycleStatus::Returning, returning[k]),
                        (LifecycleStatus::Resurrecting, resurrecting[k]),
                        (LifecycleStatus::Dormant, -dormant[k]),
                    ],
                );
                let selection = ActorSelection::LifecycleCell {
                    status,
                    day: days[k].clone(),
                };
                let engine = fixture.actors(query, &selection);
                let expected = oracle::lifecycle_actors(&fixture.world(), q, status, &days[k]);
                assert_actors(query, &selection, &engine, &expected);
                assert_eq!(engine.len() as i64, count);
            }
        },
    );
}

#[test]
fn stickiness_matches_the_oracle() {
    property_test(
        5,
        |rng| InsightQuery::StickinessQuery(random_stickiness(rng)),
        |world, query| match query {
            InsightQuery::StickinessQuery(q) => oracle::stickiness(world, q),
            _ => unreachable!(),
        },
        |rng, fixture, query, result| {
            let (InsightQuery::StickinessQuery(q), InsightResult::Stickiness { series }) =
                (query, result)
            else {
                unreachable!()
            };
            let s = series.choose(rng).unwrap();
            let intervals = rng.gen_range(1..=s.data.len()) as u32;
            let selection = ActorSelection::StickinessBar {
                series_index: s.series_index,
                intervals,
            };
            let engine = fixture.actors(query, &selection);
            let expected =
                oracle::stickiness_actors(&fixture.world(), q, s.series_index, intervals);
            assert_actors(query, &selection, &engine, &expected);
            assert_eq!(engine.len() as u64, s.data[intervals as usize - 1]);
        },
    );
}

#[test]
fn paths_match_the_oracle() {
    property_test(
        6,
        |rng| InsightQuery::PathsQuery(random_paths(rng)),
        |world, query| match query {
            InsightQuery::PathsQuery(q) => oracle::paths(world, q),
            _ => unreachable!(),
        },
        |_, _, _, _| {},
    );
}

#[test]
fn sql_counts_match_the_oracle() {
    for seed in 0..seeds().min(4) {
        let fixture = Fixture::new(dataset(seed, 300));
        let query = InsightQuery::SqlQuery(SqlQuery {
            query: "SELECT count(*) AS events, count(DISTINCT person_id) AS persons FROM events"
                .into(),
        });
        let InsightResult::Sql { columns, rows, .. } = fixture.run(&query).unwrap() else {
            unreachable!()
        };
        assert_eq!(columns, vec!["events", "persons"]);
        assert_eq!(rows[0][0], json!(fixture.events.len()));
        assert_eq!(
            rows[0][1],
            json!(oracle::distinct_persons(&fixture.world()))
        );
    }
}

