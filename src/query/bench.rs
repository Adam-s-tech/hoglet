//! Throughput check on ~10M events. Ignored by default; run with
//! `cargo test --release -p hoglet bench_ten_million -- --ignored --nocapture`.
//! `HOGLET_BENCH_EVENTS` overrides the event count.

use std::sync::Arc;
use std::time::Instant;

use chrono::{Duration, TimeZone, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Map, json};
use uuid::Uuid;

use super::{EngineConfig, QueryEngine};
use crate::capture::event::CapturedEvent;
use crate::contract::common::*;
use crate::contract::insight::*;
use crate::persons::PersonStore;
use crate::source::DirectorySource;

const PROJECT: &str = "33333333-3333-4333-8333-333333333333";
const DAYS: i64 = 30;

#[test]
#[ignore = "benchmark: run explicitly in release mode"]
fn bench_ten_million() {
    let total: usize = std::env::var("HOGLET_BENCH_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000_000);
    let persons_count = 300_000_u32;
    let dir = tempfile::tempdir().unwrap();
    let source = DirectorySource::new(dir.path().join("lake"));
    let now = Utc.with_ymd_and_hms(2026, 3, 31, 12, 0, 0).unwrap();
    let start = now - Duration::days(DAYS);
    let mut rng = StdRng::seed_from_u64(7);

    let generated = Instant::now();
    let per_day = total / DAYS as usize;
    for day in 0..DAYS {
        let date = (start + Duration::days(day)).date_naive();
        let directory = source.partition_dir(PROJECT, date);
        std::fs::create_dir_all(&directory).unwrap();
        let midnight = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
        let events: Vec<CapturedEvent> = (0..per_day)
            .map(|_| {
                let person = rng.gen_range(0..persons_count);
                let roll = rng.gen_range(0..100);
                let name = match roll {
                    0..60 => "$pageview",
                    60..70 => "signup",
                    70..75 => "purchase",
                    _ => "click",
                };
                let mut properties = Map::new();
                properties.insert("$session_id".into(), json!(format!("s{}-{}", person, day)));
                properties.insert(
                    "$pathname".into(),
                    json!(["/", "/pricing", "/signup", "/docs"][rng.gen_range(0..4)]),
                );
                properties.insert(
                    "$browser".into(),
                    json!(["Chrome", "Firefox", "Safari"][rng.gen_range(0..3)]),
                );
                properties.insert("plan".into(), json!(["free", "pro"][rng.gen_range(0..2)]));
                properties.insert("amount".into(), json!(rng.gen_range(0..500)));
                CapturedEvent {
                    uuid: Uuid::from_u128(rng.r#gen()),
                    event: name.into(),
                    distinct_id: format!("anon-{person}"),
                    token: "phc_bench".into(),
                    timestamp: midnight + Duration::microseconds(rng.gen_range(0..86_400_000_000)),
                    properties,
                }
            })
            .collect();
        crate::lake::parquet::write_file(&events, &directory.join(format!("{day}.parquet")))
            .unwrap();
    }
    println!(
        "generated {} events in {:?}",
        per_day * DAYS as usize,
        generated.elapsed()
    );

    // 50k identity overrides: anon-i → user-i.
    let projections = dir.path().join("projections.db");
    let connection = rusqlite::Connection::open(&projections).unwrap();
    crate::projections::initialize_schema(&connection).unwrap();
    connection
        .execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 49999)
             INSERT INTO persons (project_id, id, created_at, properties, first_seen_key)
             SELECT '33333333-3333-4333-8333-333333333333', 'user-' || i, '2026-01-01T00:00:00Z',
                    '{\"plan\":\"pro\"}', 'user-' || i FROM n;
             WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 49999)
             INSERT INTO distinct_ids (project_id, distinct_id, person_id, seq)
             SELECT '33333333-3333-4333-8333-333333333333', 'anon-' || i, 'user-' || i, 1 FROM n;
             UPDATE identity_state SET seq = 1;",
        )
        .unwrap();
    drop(connection);

    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    for (label, config) in [
        (
            "dev box",
            EngineConfig {
                memory_limit_mb: 8_192,
                threads,
                connections: 1,
                ..EngineConfig::default()
            },
        ),
        (
            "small VPS (2 threads, 512MB)",
            EngineConfig {
                memory_limit_mb: 512,
                threads: 2,
                connections: 1,
                temp_directory: Some(dir.path().join("spill")),
                ..EngineConfig::default()
            },
        ),
    ] {
        let engine = QueryEngine::new(
            Arc::new(DirectorySource::new(dir.path().join("lake"))),
            Arc::new(PersonStore::open(&projections).unwrap()),
            config,
        )
        .unwrap();
        let range = DateRange {
            date_from: "-30d".into(),
            date_to: None,
        };
        let pageviews = |math| EventNode {
            event: Some("$pageview".into()),
            custom_name: None,
            properties: Vec::new(),
            math,
            math_property: None,
        };
        let trends = |math| {
            InsightQuery::TrendsQuery(TrendsQuery {
                series: vec![pageviews(math)],
                date_range: range.clone(),
                interval: Interval::Day,
                properties: Vec::new(),
                breakdown: None,
                formula: None,
                compare: false,
                display: ChartDisplay::default(),
            })
        };
        let step = |name: &str| EventNode {
            event: Some(name.into()),
            custom_name: None,
            properties: Vec::new(),
            math: Math::Total,
            math_property: None,
        };
        let funnel = InsightQuery::FunnelsQuery(FunnelsQuery {
            series: vec![step("$pageview"), step("signup"), step("purchase")],
            date_range: range.clone(),
            properties: Vec::new(),
            breakdown: None,
            funnel_window: FunnelWindow::default(),
            funnel_order: FunnelOrder::Ordered,
            exclusions: Vec::new(),
        });
        for (name, query) in [
            ("trends total", trends(Math::Total)),
            ("trends dau", trends(Math::Dau)),
            ("trends wau", trends(Math::WeeklyActive)),
            ("funnel 3 steps", funnel),
        ] {
            let mut timings = Vec::new();
            for _ in 0..3 {
                let started = Instant::now();
                let response = engine
                    .run_at(
                        PROJECT,
                        &QueryRequest {
                            query: query.clone(),
                            refresh: true,
                        },
                        now,
                    )
                    .unwrap();
                timings.push(started.elapsed());
                std::hint::black_box(response);
            }
            println!("[{label}] {name}: {timings:?}");
        }
    }
}
