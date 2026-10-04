//! Latency of the explore read path on a ~1M-event generated dataset.
//!
//! Run: `cargo test --release --test explore_bench -- --ignored --nocapture`

#[path = "support/explore_data.rs"]
mod explore_data;

use std::collections::HashSet;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use explore_data::{Fixture, Spec, generate};
use hoglet::contract::web::{WebDimension, WebQuery};
use hoglet::explore::events::{FeedQuery, feed};
use hoglet::explore::{persons, web};

const PROJECT: &str = "bench";

fn time<T>(label: &str, runs: usize, mut work: impl FnMut() -> T) -> T {
    let mut samples: Vec<StdDuration> = Vec::with_capacity(runs);
    let mut last = None;
    for _ in 0..runs {
        let started = Instant::now();
        last = Some(work());
        samples.push(started.elapsed());
    }
    let first = samples[0];
    samples.sort();
    let median = samples[samples.len() / 2];
    let max = samples[samples.len() - 1];
    println!(
        "{label:<46} first {:>8.1} ms  median {:>8.1} ms  max {:>8.1} ms",
        first.as_secs_f64() * 1e3,
        median.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
    last.expect("at least one run")
}

#[test]
#[ignore = "benchmark; run explicitly in release mode"]
fn explore_latency_on_a_million_events() {
    let start: DateTime<Utc> = DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
        .expect("start")
        .with_timezone(&Utc);
    let started = Instant::now();
    let dataset = generate(&Spec {
        seed: 42,
        humans: 52_000,
        start,
        days: 30,
        max_sessions_per_device: 6,
    });
    println!(
        "generated {} events in {:.1}s",
        dataset.events.len(),
        started.elapsed().as_secs_f64()
    );

    let mut fixture = Fixture::new();
    let started = Instant::now();
    fixture.write_events(PROJECT, &dataset.events, 1);
    // Identity state only needs each distinct id's first event and every
    // merge; skipping the rest keeps setup fast without changing persons.
    let mut seen = HashSet::new();
    let identity = dataset
        .events
        .iter()
        .filter(|event| seen.insert(event.distinct_id.clone()) || event.event == "$identify");
    fixture.project(PROJECT, identity);
    println!(
        "loaded lake + projections in {:.1}s",
        started.elapsed().as_secs_f64()
    );

    let now = start + Duration::days(30) + Duration::hours(1);
    let explorer = fixture.explorer.clone();
    let run = |work: &dyn Fn(&duckdb::Connection) -> usize| {
        explorer
            .with_connection(|connection| Ok(work(connection)))
            .expect("run")
    };

    let newest = time("events feed: newest 100, no filters", 20, || {
        run(&|connection| {
            feed(
                &explorer,
                connection,
                PROJECT,
                &FeedQuery {
                    limit: 100,
                    ..FeedQuery::default()
                },
                now,
            )
            .expect("feed")
            .events
            .len()
        })
    });
    assert_eq!(newest, 100);
    // Live case inside a full day partition (~33k events in today's files).
    let midday = start + Duration::days(29) + Duration::hours(12);
    time("events feed: newest 100, full today partition", 20, || {
        run(&|connection| {
            feed(
                &explorer,
                connection,
                PROJECT,
                &FeedQuery {
                    limit: 100,
                    ..FeedQuery::default()
                },
                midday,
            )
            .expect("feed")
            .events
            .len()
        })
    });
    time("events feed: page 100 from mid-history", 10, || {
        run(&|connection| {
            let query = FeedQuery {
                limit: 100,
                before: Some(start + Duration::days(15)),
                ..FeedQuery::default()
            };
            feed(&explorer, connection, PROJECT, &query, now)
                .expect("feed")
                .events
                .len()
        })
    });
    time("events feed: event=$identify, 100", 10, || {
        run(&|connection| {
            let query = FeedQuery {
                limit: 100,
                event: Some("$identify".into()),
                ..FeedQuery::default()
            };
            feed(&explorer, connection, PROJECT, &query, now)
                .expect("feed")
                .events
                .len()
        })
    });
    time("events feed: property filter plan=pro, 100", 10, || {
        run(&|connection| {
            let query = FeedQuery {
                limit: 100,
                filters: serde_json::from_value(serde_json::json!([
                    {"key": "plan", "value": "pro"}
                ]))
                .expect("filters"),
                ..FeedQuery::default()
            };
            feed(&explorer, connection, PROJECT, &query, now)
                .expect("feed")
                .events
                .len()
        })
    });

    let week = WebQuery {
        date_from: "-7d".into(),
        date_to: None,
        interval: None,
        properties: Vec::new(),
    };
    let month = WebQuery {
        date_from: "-30d".into(),
        ..week.clone()
    };
    let overview = time("web overview: -7d (+ previous 7d)", 10, || {
        run(&|connection| {
            web::overview(&explorer, connection, PROJECT, &week, now)
                .expect("overview")
                .pageviews
                .value as usize
        })
    });
    assert!(overview > 0);
    let pageviews = time("web overview: -30d (+ previous 30d = all data)", 10, || {
        run(&|connection| {
            web::overview(&explorer, connection, PROJECT, &month, now)
                .expect("overview")
                .pageviews
                .value as usize
        })
    });
    println!("  -30d pageviews: {pageviews}");
    for dimension in [
        WebDimension::Page,
        WebDimension::EntryPage,
        WebDimension::ReferringDomain,
    ] {
        time(&format!("web breakdown {dimension:?}: -30d"), 5, || {
            run(&|connection| {
                web::breakdown(&explorer, connection, PROJECT, &month, dimension, 10, now)
                    .expect("breakdown")
                    .rows
                    .len()
            })
        });
    }
    time("persons list: first page of 50", 10, || {
        persons::list(&fixture.persons, PROJECT, None, None, 50)
            .expect("list")
            .persons
            .len()
    });
    time("persons list: search 'person123'", 10, || {
        persons::list(&fixture.persons, PROJECT, Some("person123"), None, 50)
            .expect("list")
            .persons
            .len()
    });
    let someone = dataset
        .person_of
        .values()
        .find(|person| person.starts_with("user-"))
        .expect("an identified person")
        .clone();
    time("person detail (all-time stats)", 10, || {
        run(&|connection| {
            persons::detail(&explorer, connection, PROJECT, &someone)
                .expect("detail")
                .event_count as usize
        })
    });
}
