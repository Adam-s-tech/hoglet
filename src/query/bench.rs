//! Throughput check on ~10M events. Ignored by default; run it in release
//! mode, ideally under a one-core cgroup:
//!
//! ```text
//! CARGO_PROFILE_RELEASE_LTO=false cargo test --release --lib --no-run
//! systemd-run --user --scope -q -p CPUQuota=100% -p MemoryMax=1500M \
//!     -p MemorySwapMax=0 <test binary> bench_ten_million --ignored --nocapture
//! ```
//!
//! Environment:
//! - `HOGLET_BENCH_EVENTS`: event count (default 10M).
//! - `HOGLET_BENCH_DIR`: keep the generated data there and reuse it on the
//!   next run (generation takes minutes; queries take seconds).
//! - `HOGLET_BENCH_LAYOUT`: `compacted` (default; what production reads: the
//!   compactor's output), `raw` (unsorted publication files) or `both`.
//! - `HOGLET_BENCH_ONLY`: comma-separated substrings of query names to run.
//! - `HOGLET_BENCH_RUNS`: runs per query (default 3; the median is reported).
//! - `HOGLET_BENCH_MEM_MB` / `HOGLET_BENCH_THREADS`: engine limits
//!   (defaults: the production defaults).
//! - `HOGLET_BENCH_PARTITION_ROWS`: rows per person partition.
//! - `HOGLET_QUERY_PROFILE=1`: print DuckDB's EXPLAIN ANALYZE of every
//!   statement the engine runs (see `Ctx::profile`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, TimeZone, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Map, json};
use uuid::Uuid;

use super::{EngineConfig, QueryEngine};
use crate::capture::event::CapturedEvent;
use crate::contract::common::*;
use crate::contract::insight::*;
use crate::contract::web::WebQuery;
use crate::explore::{Explorer, web};
use crate::persons::PersonStore;
use crate::source::DirectorySource;

const PROJECT: &str = "33333333-3333-4333-8333-333333333333";
const DAYS: i64 = 30;
const PERSONS: u32 = 300_000;

/// `HOGLET_BENCH_IDS=uuid` uses 36-character ids (what SDKs generate)
/// instead of the short `anon-N` / `user-N` ones.
fn long_ids() -> bool {
    std::env::var("HOGLET_BENCH_IDS").is_ok_and(|style| style == "uuid")
}

fn anon_id(person: u32) -> String {
    if long_ids() {
        Uuid::from_u128(u128::from(person) * 0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835 + 1)
            .to_string()
    } else {
        format!("anon-{person}")
    }
}

fn user_id(person: u32) -> String {
    if long_ids() {
        Uuid::from_u128(u128::from(person) * 0xD1B5_4A32_D192_ED03_8CB9_2BA7_2F3D_8DD7 + 2)
            .to_string()
    } else {
        format!("user-{person}")
    }
}

fn generate_raw(lake: &Path, total: usize) {
    let source = DirectorySource::new(lake);
    let now = Utc.with_ymd_and_hms(2026, 3, 31, 12, 0, 0).unwrap();
    let start = now - Duration::days(DAYS);
    let mut rng = StdRng::seed_from_u64(7);
    let per_day = total / DAYS as usize;
    for day in 0..DAYS {
        let date = (start + Duration::days(day)).date_naive();
        let directory = source.partition_dir(PROJECT, date);
        std::fs::create_dir_all(&directory).unwrap();
        let midnight = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
        let events: Vec<CapturedEvent> = (0..per_day)
            .map(|_| {
                let person = rng.gen_range(0..PERSONS);
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
                    distinct_id: anon_id(person),
                    token: "phc_bench".into(),
                    timestamp: midnight + Duration::microseconds(rng.gen_range(0..86_400_000_000)),
                    properties,
                }
            })
            .collect();
        crate::lake::parquet::write_file(&events, &directory.join(format!("{day}.parquet")))
            .unwrap();
    }
}

/// Rewrite every raw partition file the way the compactor does.
fn compact(raw: &Path, compacted: &Path) {
    let duck = duckdb::Connection::open_in_memory().unwrap();
    duck.execute_batch(
        "SET memory_limit='192MB'; SET threads=1; SET preserve_insertion_order=false;",
    )
    .unwrap();
    let literal = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "''"));
    for day in std::fs::read_dir(raw.join(PROJECT)).unwrap() {
        let day = day.unwrap();
        let target = compacted.join(PROJECT).join(day.file_name());
        std::fs::create_dir_all(&target).unwrap();
        let sources: Vec<String> = std::fs::read_dir(day.path())
            .unwrap()
            .map(|file| literal(&file.unwrap().path()))
            .collect();
        duck.execute(
            &crate::lake::compactor::merge_sql(
                &sources.join(", "),
                &literal(&target.join("c000000000001.parquet")),
            ),
            [],
        )
        .unwrap();
    }
}

fn load_identity(projections: &Path) {
    let connection = rusqlite::Connection::open(projections).unwrap();
    crate::projections::initialize_schema(&connection).unwrap();
    // 50k identity overrides: anon id i -> user id i.
    let transaction = connection.unchecked_transaction().unwrap();
    {
        let mut person = transaction
            .prepare(
                "INSERT INTO persons (project_id, id, created_at, properties, first_seen_key)
                 VALUES (?1, ?2, '2026-01-01T00:00:00Z', '{\"plan\":\"pro\"}', ?2)",
            )
            .unwrap();
        let mut distinct = transaction
            .prepare(
                "INSERT INTO distinct_ids (project_id, distinct_id, person_id, seq)
                 VALUES (?1, ?2, ?3, 1)",
            )
            .unwrap();
        for i in 0..50_000 {
            person
                .execute(rusqlite::params![PROJECT, user_id(i)])
                .unwrap();
            distinct
                .execute(rusqlite::params![PROJECT, anon_id(i), user_id(i)])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    connection
        .execute_batch("UPDATE identity_state SET seq = 1;")
        .unwrap();
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn median(samples: &mut [StdDuration]) -> StdDuration {
    samples.sort();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "benchmark: run explicitly in release mode"]
fn bench_ten_million() {
    let total = env_usize("HOGLET_BENCH_EVENTS", 10_000_000);
    let runs = env_usize("HOGLET_BENCH_RUNS", 3).max(1);
    let layout = std::env::var("HOGLET_BENCH_LAYOUT").unwrap_or_else(|_| "compacted".into());
    let only: Vec<String> = std::env::var("HOGLET_BENCH_ONLY")
        .map(|list| list.split(',').map(str::to_owned).collect())
        .unwrap_or_default();

    let scratch;
    let root = match std::env::var("HOGLET_BENCH_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            scratch = tempfile::tempdir().unwrap();
            scratch.path().to_path_buf()
        }
    };
    std::fs::create_dir_all(&root).unwrap();
    let raw = root.join("lake_raw");
    let compacted = root.join("lake_compacted");
    let projections = root.join("projections.db");
    let ready = root.join(format!("ready-{total}"));
    if !ready.exists() {
        for stale in [&raw, &compacted] {
            let _ = std::fs::remove_dir_all(stale);
        }
        let _ = std::fs::remove_file(&projections);
        let generated = Instant::now();
        generate_raw(&raw, total);
        println!(
            "generated {} events in {:?}",
            total / DAYS as usize * DAYS as usize,
            generated.elapsed()
        );
        let compacting = Instant::now();
        compact(&raw, &compacted);
        println!("compacted in {:?}", compacting.elapsed());
        load_identity(&projections);
        std::fs::write(&ready, b"").unwrap();
    }
    let now = Utc.with_ymd_and_hms(2026, 3, 31, 12, 0, 0).unwrap();

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
    let queries = [
        ("trends total", trends(Math::Total)),
        ("trends dau", trends(Math::Dau)),
        ("trends wau", trends(Math::WeeklyActive)),
        ("funnel 3 steps", funnel),
    ];
    let selected = |name: &str| only.is_empty() || only.iter().any(|part| name.contains(part));
    let web_query = WebQuery {
        date_from: "-30d".into(),
        date_to: None,
        interval: None,
        properties: Vec::new(),
    };

    let mut config = EngineConfig {
        memory_limit_mb: env_usize("HOGLET_BENCH_MEM_MB", 512) as u32,
        threads: env_usize("HOGLET_BENCH_THREADS", 2) as u32,
        connections: 1,
        temp_directory: Some(root.join("spill")),
        partition_rows: env_usize("HOGLET_BENCH_PARTITION_ROWS", super::PARTITION_ROWS as usize)
            as u64,
        ..EngineConfig::default()
    };
    config.timeout = StdDuration::from_secs(600);
    let layouts: Vec<(&str, &Path)> = match layout.as_str() {
        "raw" => vec![("raw", raw.as_path())],
        "both" => vec![("raw", raw.as_path()), ("compacted", compacted.as_path())],
        _ => vec![("compacted", compacted.as_path())],
    };
    println!(
        "limits: {} MB, {} threads; runs per query: {runs}",
        config.memory_limit_mb, config.threads
    );
    for (label, lake) in layouts {
        let persons = Arc::new(PersonStore::open(&projections).unwrap());
        let engine = QueryEngine::new(
            Arc::new(DirectorySource::new(lake)),
            persons.clone(),
            config.clone(),
        )
        .unwrap();
        for (name, query) in &queries {
            if !selected(name) {
                continue;
            }
            let mut timings = Vec::new();
            for _ in 0..runs {
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
            let first = timings[0];
            println!(
                "[{label}] {name}: median {:.3}s first {:.3}s runs {:?}",
                median(&mut timings).as_secs_f64(),
                first.as_secs_f64(),
                timings
            );
        }
        let name = "web overview 30d";
        if selected(name) {
            let explorer = Arc::new(
                Explorer::new(Arc::new(DirectorySource::new(lake)), persons.clone()).unwrap(),
            );
            let mut timings = Vec::new();
            for _ in 0..runs {
                let started = Instant::now();
                let overview = explorer
                    .with_connection(|connection| {
                        web::overview(&explorer, connection, PROJECT, &web_query, now)
                    })
                    .unwrap();
                timings.push(started.elapsed());
                std::hint::black_box(overview);
            }
            let first = timings[0];
            println!(
                "[{label}] {name}: median {:.3}s first {:.3}s runs {:?}",
                median(&mut timings).as_secs_f64(),
                first.as_secs_f64(),
                timings
            );
        }
    }
}

/// SQL scratchpad over the kept benchmark data: `HOGLET_LAB_SQL=<file>`
/// holds statements separated by lines containing only `;;`. `{FILES}` is
/// the dataset's `read_parquet([...])` list (`HOGLET_BENCH_LAYOUT`
/// selects raw or compacted); `person_overrides` is loaded. Each statement
/// is timed; `HOGLET_LAB_EXPLAIN=1` prints EXPLAIN ANALYZE too.
#[test]
#[ignore = "scratchpad: needs HOGLET_BENCH_DIR data and HOGLET_LAB_SQL"]
fn sql_lab() {
    let root = std::path::PathBuf::from(std::env::var("HOGLET_BENCH_DIR").unwrap());
    let layout = std::env::var("HOGLET_BENCH_LAYOUT").unwrap_or_else(|_| "compacted".into());
    let lake = root.join(format!("lake_{layout}")).join(PROJECT);
    let mut files = Vec::new();
    let mut days: Vec<_> = std::fs::read_dir(&lake).unwrap().map(|d| d.unwrap().path()).collect();
    days.sort();
    for day in days {
        for file in std::fs::read_dir(day).unwrap() {
            files.push(format!("'{}'", file.unwrap().path().display()));
        }
    }
    let source = format!("read_parquet([{}], union_by_name = true)", files.join(", "));
    let config = duckdb::Config::default()
        .max_memory(&format!("{}MB", env_usize("HOGLET_BENCH_MEM_MB", 512)))
        .unwrap()
        .threads(env_usize("HOGLET_BENCH_THREADS", 2) as i64)
        .unwrap();
    let connection = duckdb::Connection::open_in_memory_with_flags(config).unwrap();
    connection
        .execute_batch(&format!(
            "SET temp_directory = '{}'; SET parquet_metadata_cache = true;
             CREATE TABLE person_overrides (project_id VARCHAR, distinct_id VARCHAR, person_id VARCHAR);",
            root.join("spill").display()
        ))
        .unwrap();
    let projections = rusqlite::Connection::open(root.join("projections.db")).unwrap();
    let mut statement = projections
        .prepare("SELECT distinct_id, person_id FROM distinct_ids")
        .unwrap();
    let mut appender = connection.appender("person_overrides").unwrap();
    let mut rows = statement.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let distinct: String = row.get(0).unwrap();
        let person: String = row.get(1).unwrap();
        appender
            .append_row(duckdb::params![PROJECT, distinct, person])
            .unwrap();
    }
    appender.flush().unwrap();
    let text = std::fs::read_to_string(std::env::var("HOGLET_LAB_SQL").unwrap()).unwrap();
    for statement in text.split("\n;;\n") {
        let sql = statement.replace("{FILES}", &source);
        if sql.trim().is_empty() {
            continue;
        }
        let runs = env_usize("HOGLET_BENCH_RUNS", 1);
        for _ in 0..runs {
            let started = Instant::now();
            let mut prepared = connection.prepare(&sql).unwrap();
            let mut rows = prepared.query([]).unwrap();
            let mut count = 0;
            let mut first = String::new();
            while let Some(row) = rows.next().unwrap() {
                if count == 0 {
                    first = (0..row.as_ref().column_count())
                        .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join(", ");
                }
                count += 1;
            }
            println!(
                "{:.3}s  rows={count} first=[{first}]  :: {}",
                started.elapsed().as_secs_f64(),
                sql.lines().next().unwrap_or("").chars().take(90).collect::<String>()
            );
        }
        if std::env::var_os("HOGLET_LAB_EXPLAIN").is_some() {
            let mut prepared = connection.prepare(&format!("EXPLAIN ANALYZE {sql}")).unwrap();
            let mut rows = prepared.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                let plan: String = row.get(1).unwrap();
                let plan: String = plan
                    .lines()
                    .filter(|line| !line.contains("/home/pal"))
                    .collect::<Vec<_>>()
                    .join("\n");
                println!("{plan}");
            }
        }
    }
}
