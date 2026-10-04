//! Targeted engine regressions: each test pins one thing that was wrong
//! before or one safety property.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::proptests::{Fixture, HOSTILE_KEY, PROJECT, QUOTE_KEY, now, test_config};
use super::{EngineConfig, QueryError};
use crate::capture::event::CapturedEvent;
use crate::contract::common::*;
use crate::contract::insight::*;

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn event(name: &str, distinct_id: &str, time: &str, properties: Value) -> CapturedEvent {
    let Value::Object(properties) = properties else {
        panic!("properties must be an object")
    };
    CapturedEvent {
        uuid: Uuid::new_v4(),
        event: name.into(),
        distinct_id: distinct_id.into(),
        token: "phc_test".into(),
        timestamp: at(time),
        properties,
    }
}

fn series(name: Option<&str>, math: Math) -> EventNode {
    EventNode {
        event: name.map(Into::into),
        custom_name: None,
        properties: Vec::new(),
        math,
        math_property: Some("amount".into()),
    }
}

fn trends(nodes: Vec<EventNode>, from: &str, to: &str) -> TrendsQuery {
    TrendsQuery {
        series: nodes,
        date_range: DateRange {
            date_from: from.into(),
            date_to: Some(to.into()),
        },
        interval: Interval::Day,
        properties: Vec::new(),
        breakdown: None,
        formula: None,
        compare: false,
        display: ChartDisplay::default(),
    }
}

fn data(result: &InsightResult) -> Vec<Vec<f64>> {
    match result {
        InsightResult::Trends { series } => series.iter().map(|s| s.data.clone()).collect(),
        other => panic!("not trends: {other:?}"),
    }
}

fn run(fixture: &Fixture, query: InsightQuery) -> Result<InsightResult, QueryError> {
    fixture
        .engine
        .run_at(
            PROJECT,
            &QueryRequest {
                query,
                refresh: false,
            },
            now(),
        )
        .map(|response| response.result)
}

#[test]
fn identify_merges_reach_person_counts_and_invalidate_the_cache() {
    let mut fixture = Fixture::new(vec![
        event("$pageview", "anon-1", "2026-03-10T10:00:00Z", json!({})),
        event("$pageview", "user-1", "2026-03-10T11:00:00Z", json!({})),
        event("$pageview", "user-1", "2026-03-11T11:00:00Z", json!({})),
    ]);
    let query = InsightQuery::TrendsQuery(trends(
        vec![series(Some("$pageview"), Math::Dau)],
        "2026-03-10",
        "2026-03-11",
    ));
    assert_eq!(
        data(&run(&fixture, query.clone()).unwrap()),
        vec![vec![2.0, 1.0]]
    );

    // Identify on a later day merges anon-1 into user-1: the March 10 count
    // becomes one person, and the cached answer is not served.
    fixture.append(vec![event(
        "$identify",
        "user-1",
        "2026-03-12T09:00:00Z",
        json!({"$anon_distinct_id": "anon-1"}),
    )]);
    assert_eq!(data(&run(&fixture, query).unwrap()), vec![vec![1.0, 1.0]]);
}

#[test]
fn weekly_and_monthly_active_use_trailing_windows() {
    let fixture = Fixture::new(vec![
        event("$pageview", "a", "2026-03-01T10:00:00Z", json!({})),
        event("$pageview", "b", "2026-03-05T10:00:00Z", json!({})),
        event("$pageview", "c", "2026-03-09T10:00:00Z", json!({})),
    ]);
    let query = |math| {
        InsightQuery::TrendsQuery(trends(
            vec![series(Some("$pageview"), math)],
            "2026-03-05",
            "2026-03-09",
        ))
    };
    // DAU: only that day. WAU: trailing 7 days, including March 1 (before
    // the range). MAU: everything in the trailing 30 days.
    assert_eq!(
        data(&run(&fixture, query(Math::Dau)).unwrap())[0],
        vec![1.0, 0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        data(&run(&fixture, query(Math::WeeklyActive)).unwrap())[0],
        vec![2.0, 2.0, 2.0, 1.0, 2.0]
    );
    assert_eq!(
        data(&run(&fixture, query(Math::MonthlyActive)).unwrap())[0],
        vec![2.0, 2.0, 2.0, 2.0, 3.0]
    );
}

#[test]
fn unique_sessions_count_sessions_not_people() {
    let fixture = Fixture::new(vec![
        event(
            "$pageview",
            "a",
            "2026-03-10T10:00:00Z",
            json!({"$session_id": "s1"}),
        ),
        event(
            "$pageview",
            "a",
            "2026-03-10T11:00:00Z",
            json!({"$session_id": "s2"}),
        ),
        event(
            "$pageview",
            "a",
            "2026-03-10T12:00:00Z",
            json!({"$session_id": "s3"}),
        ),
        event(
            "$pageview",
            "a",
            "2026-03-10T13:00:00Z",
            json!({"$session_id": "s3"}),
        ),
        event("$pageview", "b", "2026-03-10T13:00:00Z", json!({})),
    ]);
    let result = run(
        &fixture,
        InsightQuery::TrendsQuery(trends(
            vec![series(Some("$pageview"), Math::UniqueSession)],
            "2026-03-10",
            "2026-03-10",
        )),
    )
    .unwrap();
    assert_eq!(data(&result), vec![vec![3.0]]);
}

#[test]
fn funnels_find_a_conversion_after_a_failed_first_attempt() {
    let fixture = Fixture::new(vec![
        event("$pageview", "a", "2026-03-01T10:00:00Z", json!({})),
        event("$pageview", "a", "2026-03-10T10:00:00Z", json!({})),
        event("signup", "a", "2026-03-10T10:05:00Z", json!({})),
    ]);
    let query = InsightQuery::FunnelsQuery(FunnelsQuery {
        series: vec![
            series(Some("$pageview"), Math::Total),
            series(Some("signup"), Math::Total),
        ],
        date_range: DateRange {
            date_from: "2026-02-20".into(),
            date_to: Some("2026-03-14".into()),
        },
        properties: Vec::new(),
        breakdown: None,
        funnel_window: FunnelWindow {
            interval: 1,
            unit: WindowUnit::Day,
        },
        funnel_order: FunnelOrder::Ordered,
        exclusions: Vec::new(),
    });
    let InsightResult::Funnels {
        steps,
        time_to_convert,
        ..
    } = run(&fixture, query).unwrap()
    else {
        panic!()
    };
    assert_eq!(steps[1].count, 1);
    assert_eq!(steps[1].average_conversion_time_s, Some(300.0));
    assert_eq!(time_to_convert.iter().map(|b| b.count).sum::<u64>(), 1);
}

#[test]
fn hostile_names_keys_and_values_are_data_not_sql() {
    let name = "x'); DROP TABLE person_overrides; --";
    let fixture = Fixture::new(vec![
        event(
            name,
            "a",
            "2026-03-10T10:00:00Z",
            json!({HOSTILE_KEY: "v'); DROP", QUOTE_KEY: "q\"'", "ключ": "значение"}),
        ),
        event(
            name,
            "b",
            "2026-03-10T10:00:00Z",
            json!({HOSTILE_KEY: "other"}),
        ),
        event("$pageview", "c", "2026-03-10T10:00:00Z", json!({})),
    ]);
    let mut query = trends(
        vec![series(Some(name), Math::Total)],
        "2026-03-10",
        "2026-03-10",
    );
    query.properties = vec![
        PropertyFilter {
            key: HOSTILE_KEY.into(),
            source: PropertySource::Event,
            operator: PropertyOperator::Exact,
            value: json!("v'); DROP"),
        },
        PropertyFilter {
            key: QUOTE_KEY.into(),
            source: PropertySource::Event,
            operator: PropertyOperator::Icontains,
            value: json!("Q\"'"),
        },
        PropertyFilter {
            key: "ключ".into(),
            source: PropertySource::Event,
            operator: PropertyOperator::Regex,
            value: json!("^зн"),
        },
        PropertyFilter {
            key: "'; SELECT 1; --".into(),
            source: PropertySource::Person,
            operator: PropertyOperator::IsNotSet,
            value: Value::Null,
        },
    ];
    query.breakdown = Some(Breakdown {
        property: "$.a\"b'); DROP TABLE x; --".into(),
        source: PropertySource::Event,
        limit: 5,
    });
    let InsightResult::Trends { series } = run(&fixture, InsightQuery::TrendsQuery(query)).unwrap()
    else {
        panic!()
    };
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].data, vec![1.0]);
    assert_eq!(series[0].breakdown_value.as_deref(), Some(BREAKDOWN_NONE));
    // The override table survived and still works.
    let dau = run(
        &fixture,
        InsightQuery::TrendsQuery(trends(
            vec![series_of(None, Math::Dau)],
            "2026-03-10",
            "2026-03-10",
        )),
    )
    .unwrap();
    assert_eq!(data(&dau), vec![vec![3.0]]);
}

fn series_of(name: Option<&str>, math: Math) -> EventNode {
    series(name, math)
}

#[test]
fn empty_projects_return_correctly_shaped_empty_results() {
    let fixture = Fixture::new(Vec::new());
    let range = DateRange {
        date_from: "-7d".into(),
        date_to: None,
    };
    let result = run(
        &fixture,
        InsightQuery::TrendsQuery(TrendsQuery {
            date_range: range.clone(),
            ..trends(
                vec![series(None, Math::Total), series(None, Math::Dau)],
                "-7d",
                "",
            )
        }),
    )
    .unwrap();
    assert_eq!(data(&result), vec![vec![0.0; 8], vec![0.0; 8]]);

    let result = run(
        &fixture,
        InsightQuery::FunnelsQuery(FunnelsQuery {
            series: vec![
                series(Some("a"), Math::Total),
                series(Some("b"), Math::Total),
            ],
            date_range: range.clone(),
            properties: Vec::new(),
            breakdown: None,
            funnel_window: FunnelWindow::default(),
            funnel_order: FunnelOrder::Ordered,
            exclusions: Vec::new(),
        }),
    )
    .unwrap();
    let InsightResult::Funnels { steps, .. } = result else {
        panic!()
    };
    assert_eq!(
        steps.iter().map(|s| s.count).collect::<Vec<_>>(),
        vec![0, 0]
    );

    let result = run(
        &fixture,
        InsightQuery::RetentionQuery(RetentionQuery {
            target: series(None, Math::Total),
            returning: series(None, Math::Total),
            period: RetentionPeriod::Day,
            total_intervals: 4,
            retention_type: RetentionType::RetentionFirstTime,
            properties: Vec::new(),
        }),
    )
    .unwrap();
    let InsightResult::Retention { cohorts, .. } = result else {
        panic!()
    };
    assert_eq!(
        cohorts.iter().map(|c| c.values.len()).collect::<Vec<_>>(),
        vec![4, 3, 2, 1]
    );

    let result = run(
        &fixture,
        InsightQuery::LifecycleQuery(LifecycleQuery {
            series: series(None, Math::Total),
            date_range: DateRange {
                date_from: "all".into(),
                date_to: None,
            },
            interval: Interval::Day,
            properties: Vec::new(),
        }),
    )
    .unwrap();
    let InsightResult::Lifecycle { new, dormant, .. } = result else {
        panic!()
    };
    assert_eq!(new, vec![0]);
    assert_eq!(dormant, vec![0]);

    let result = run(
        &fixture,
        InsightQuery::SqlQuery(SqlQuery {
            query: "SELECT count(*) AS n, count(DISTINCT session_id) AS s FROM events".into(),
        }),
    )
    .unwrap();
    let InsightResult::Sql { rows, .. } = result else {
        panic!()
    };
    assert_eq!(rows, vec![vec![json!(0), json!(0)]]);
}

fn sql(fixture: &Fixture, text: &str) -> Result<InsightResult, QueryError> {
    run(
        fixture,
        InsightQuery::SqlQuery(SqlQuery { query: text.into() }),
    )
}

/// The static binary has no ICU: ordinary time functions must still work on
/// `events.timestamp`, and `now_utc()` must exist.
#[test]
fn sql_time_functions_work_without_icu() {
    let fixture = Fixture::new(vec![
        event("a", "u1", "2026-03-10T10:30:00Z", json!({})),
        event("a", "u1", "2026-03-11T23:59:59Z", json!({})),
    ]);
    let InsightResult::Sql { rows, .. } = sql(
        &fixture,
        "SELECT CAST(date_trunc('day', timestamp) AS VARCHAR), extract(hour FROM timestamp), \
                CAST(timestamp::DATE AS VARCHAR), \
                timestamp > now_utc() - INTERVAL 7 DAY, now_utc() > TIMESTAMP '2020-01-01' \
         FROM events ORDER BY timestamp",
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(rows[0][0], json!("2026-03-10 00:00:00"));
    assert_eq!(rows[0][1], json!(10));
    assert_eq!(rows[1][2], json!("2026-03-11"));
    assert_eq!(rows[0][3], json!(false), "March 2026 is not within 7 days of now");
    assert_eq!(rows[0][4], json!(true));
}

#[test]
fn sql_is_a_read_only_single_select_sandbox() {
    let fixture = Fixture::new(vec![
        event(
            "$pageview",
            "a",
            "2026-03-10T10:00:00Z",
            json!({"$browser": "Chrome"}),
        ),
        event(
            "$identify",
            "u",
            "2026-03-10T11:00:00Z",
            json!({"$anon_distinct_id": "a"}),
        ),
    ]);
    let InsightResult::Sql { columns, types, rows, truncated } = sql(
        &fixture,
        "WITH x AS (SELECT * FROM events) SELECT person_id, browser, timestamp FROM x ORDER BY timestamp;",
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(columns, vec!["person_id", "browser", "timestamp"]);
    assert_eq!(types[0], "VARCHAR");
    assert!(types[2].starts_with("TIMESTAMP"));
    assert_eq!(rows[0][0], json!("u"), "person ids are resolved");
    assert_eq!(rows[0][1], json!("Chrome"));
    assert!(!truncated);

    let other = fixture
        .dir
        .path()
        .join("lake")
        .join("22222222-2222-4222-8222-222222222222");
    let other_file = std::fs::read_dir(
        std::fs::read_dir(&other)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap()
    .next()
    .unwrap()
    .unwrap()
    .path();
    for statement in [
        "DROP TABLE person_overrides",
        "SELECT 1; SELECT 2",
        "COPY (SELECT 1) TO '/tmp/hoglet_sql_escape.csv'",
        "SELECT * FROM read_csv('/etc/passwd')",
        &format!(
            "SELECT count(*) FROM read_parquet('{}')",
            other_file.display()
        ),
        "ATTACH '/tmp/hoglet_attach.db'",
        "INSTALL httpfs",
        "SET threads = 64",
        "SELECT * FROM no_such_table",
        "PRAGMA database_list",
        "CREATE TABLE t AS SELECT 1",
    ] {
        let error = sql(&fixture, statement).unwrap_err();
        assert!(
            matches!(error, QueryError::Invalid(_)),
            "{statement} → {error:?}"
        );
    }
    assert!(!std::path::Path::new("/tmp/hoglet_sql_escape.csv").exists());

    let InsightResult::Sql {
        rows, truncated, ..
    } = sql(&fixture, "SELECT * FROM range(20000)").unwrap()
    else {
        panic!()
    };
    assert_eq!(rows.len(), super::sql_query::MAX_SQL_ROWS);
    assert!(truncated);
}

#[test]
fn sql_and_scans_stop_at_the_deadline() {
    let fixture = Fixture::with_config(
        vec![event("$pageview", "a", "2026-03-10T10:00:00Z", json!({}))],
        EngineConfig {
            timeout: Duration::from_millis(300),
            ..test_config()
        },
    );
    let error = sql(&fixture, "SELECT count(*) FROM range(100000000000) a").unwrap_err();
    assert_eq!(error, QueryError::Timeout);

    let fixture = Fixture::with_config(
        vec![event("$pageview", "a", "2026-03-10T10:00:00Z", json!({}))],
        EngineConfig {
            timeout: Duration::ZERO,
            ..test_config()
        },
    );
    let error = run(
        &fixture,
        InsightQuery::TrendsQuery(trends(vec![series(None, Math::Total)], "-7d", "dStart")),
    )
    .unwrap_err();
    assert_eq!(error, QueryError::Timeout);
}

#[test]
fn invalid_queries_are_rejected_with_reasons() {
    let fixture = Fixture::new(Vec::new());
    let many = (0..21).map(|_| series(None, Math::Total)).collect();
    let mut cases = vec![
        InsightQuery::TrendsQuery(trends(many, "-7d", "dStart")),
        InsightQuery::TrendsQuery(TrendsQuery {
            formula: Some("A + C".into()),
            ..trends(vec![series(None, Math::Total)], "-7d", "dStart")
        }),
        InsightQuery::TrendsQuery(TrendsQuery {
            breakdown: Some(Breakdown {
                property: "plan".into(),
                source: PropertySource::Event,
                limit: 301,
            }),
            ..trends(vec![series(None, Math::Total)], "-7d", "dStart")
        }),
        InsightQuery::TrendsQuery(trends(vec![series(None, Math::Total)], "-7x", "dStart")),
        InsightQuery::TrendsQuery(TrendsQuery {
            interval: Interval::Hour,
            ..trends(vec![series(None, Math::Total)], "-3y", "dStart")
        }),
        InsightQuery::SqlQuery(SqlQuery {
            query: "x".repeat(super::validate::MAX_SQL_BYTES + 1),
        }),
    ];
    let mut regex = trends(vec![series(None, Math::Total)], "-7d", "dStart");
    regex.properties = vec![PropertyFilter {
        key: "plan".into(),
        source: PropertySource::Event,
        operator: PropertyOperator::Regex,
        value: json!("(unclosed"),
    }];
    cases.push(InsightQuery::TrendsQuery(regex));
    let mut bad_value = trends(vec![series(None, Math::Total)], "-7d", "dStart");
    bad_value.properties = vec![PropertyFilter {
        key: "amount".into(),
        source: PropertySource::Event,
        operator: PropertyOperator::Gt,
        value: json!("lots"),
    }];
    cases.push(InsightQuery::TrendsQuery(bad_value));
    for query in cases {
        let error = run(&fixture, query.clone()).unwrap_err();
        assert!(
            matches!(error, QueryError::Invalid(_)),
            "{} → {error:?}",
            serde_json::to_string(&query)
                .unwrap()
                .chars()
                .take(200)
                .collect::<String>()
        );
        assert_eq!(error.status(), 400);
    }
}

#[tokio::test]
async fn admission_is_bounded() {
    let fixture = Fixture::with_config(
        Vec::new(),
        EngineConfig {
            connections: 1,
            max_queued: 0,
            queue_wait: Duration::from_millis(50),
            ..test_config()
        },
    );
    let held = fixture.engine.admit().await.unwrap();
    let error = fixture.engine.admit().await.err().unwrap();
    assert_eq!(error, QueryError::Busy);
    assert_eq!(error.status(), 503);
    drop(held);
    assert!(fixture.engine.admit().await.is_ok());
}

#[test]
fn actor_pages_load_person_rows() {
    let mut events = Vec::new();
    for index in 0..5 {
        let mut properties = Map::new();
        properties.insert(
            "$set".into(),
            json!({"email": format!("p{index}@example.com")}),
        );
        events.push(CapturedEvent {
            uuid: Uuid::new_v4(),
            event: "$pageview".into(),
            distinct_id: format!("person-{index}"),
            token: "t".into(),
            timestamp: Utc.with_ymd_and_hms(2026, 3, 10, 10, 0, index).unwrap(),
            properties,
        });
    }
    let fixture = Fixture::new(events);
    let query = InsightQuery::TrendsQuery(trends(
        vec![series(Some("$pageview"), Math::Dau)],
        "2026-03-10",
        "2026-03-10",
    ));
    let request = |offset| ActorsRequest {
        query: query.clone(),
        selection: ActorSelection::TrendsPoint {
            series_index: 0,
            day: "2026-03-10T00:00:00Z".into(),
            breakdown_value: None,
        },
        offset,
        limit: 2,
    };
    let first = fixture
        .engine
        .actors_at(PROJECT, &request(0), now())
        .unwrap();
    assert_eq!(first.persons.len(), 2);
    assert!(first.has_more);
    assert_eq!(first.persons[0].id, "person-0");
    assert_eq!(first.persons[0].display_name, "p0@example.com");
    assert_eq!(first.persons[0].distinct_ids, vec!["person-0"]);
    let last = fixture
        .engine
        .actors_at(PROJECT, &request(4), now())
        .unwrap();
    assert_eq!(last.persons.len(), 1);
    assert!(!last.has_more);

    let wrong = ActorsRequest {
        selection: ActorSelection::StickinessBar {
            series_index: 0,
            intervals: 1,
        },
        ..request(0)
    };
    assert!(matches!(
        fixture.engine.actors_at(PROJECT, &wrong, now()),
        Err(QueryError::Invalid(_))
    ));
}

#[test]
fn refresh_bypasses_the_cache() {
    let fixture = Fixture::new(vec![event(
        "$pageview",
        "a",
        "2026-03-10T10:00:00Z",
        json!({}),
    )]);
    let request = |refresh| QueryRequest {
        query: InsightQuery::TrendsQuery(trends(vec![series(None, Math::Total)], "-7d", "dStart")),
        refresh,
    };
    assert!(
        !fixture
            .engine
            .run_at(PROJECT, &request(false), now())
            .unwrap()
            .meta
            .cached
    );
    assert!(
        fixture
            .engine
            .run_at(PROJECT, &request(false), now())
            .unwrap()
            .meta
            .cached
    );
    assert!(
        !fixture
            .engine
            .run_at(PROJECT, &request(true), now())
            .unwrap()
            .meta
            .cached
    );
}

#[test]
fn paths_link_actors_reconcile_with_the_link_count() {
    let page = |path: &str, who: &str, time: &str| {
        event("$pageview", who, time, json!({"$pathname": path}))
    };
    let fixture = Fixture::new(vec![
        // a and b: /pricing -> /signup; c: /pricing -> /docs; d: /docs only.
        page("/pricing", "a", "2026-03-10T10:00:00Z"),
        page("/signup", "a", "2026-03-10T10:01:00Z"),
        page("/pricing", "b", "2026-03-10T11:00:00Z"),
        page("/signup", "b", "2026-03-10T11:02:00Z"),
        page("/pricing", "c", "2026-03-10T12:00:00Z"),
        page("/docs", "c", "2026-03-10T12:01:00Z"),
        page("/docs", "d", "2026-03-10T13:00:00Z"),
    ]);
    let query = InsightQuery::PathsQuery(PathsQuery {
        paths_type: PathsType::Pageviews,
        start_point: None,
        end_point: None,
        step_limit: 5,
        edge_limit: 50,
        date_range: DateRange {
            date_from: "2026-03-10".into(),
            date_to: Some("2026-03-10".into()),
        },
        properties: Vec::new(),
    });
    let InsightResult::Paths { links } = run(&fixture, query.clone()).unwrap() else {
        panic!()
    };
    let link = links
        .iter()
        .find(|link| link.source == "1_/pricing" && link.target == "2_/signup")
        .expect("the strongest link is listed");
    assert_eq!(link.value, 2);

    let people = |source: &str, target: &str| {
        fixture
            .engine
            .actors_at(
                PROJECT,
                &ActorsRequest {
                    query: query.clone(),
                    selection: ActorSelection::PathsLink {
                        source: source.into(),
                        target: target.into(),
                    },
                    offset: 0,
                    limit: 100,
                },
                now(),
            )
            .unwrap()
            .persons
            .into_iter()
            .map(|person| person.id)
            .collect::<Vec<_>>()
    };
    let mut signup = people("1_/pricing", "2_/signup");
    signup.sort();
    assert_eq!(signup, vec!["a", "b"], "the persons behind a link reconcile with its count");
    assert_eq!(people("1_/pricing", "2_/docs"), vec!["c"]);
    assert!(people("1_/docs", "2_/pricing").is_empty());

    let bad = fixture.engine.actors_at(
        PROJECT,
        &ActorsRequest {
            query: query.clone(),
            selection: ActorSelection::PathsLink {
                source: "1_/pricing".into(),
                target: "3_/signup".into(),
            },
            offset: 0,
            limit: 10,
        },
        now(),
    );
    assert!(matches!(bad, Err(QueryError::Invalid(_))));
}

#[test]
fn sql_hides_host_introspection_and_caps_cells() {
    let fixture = Fixture::new(vec![event(
        "$pageview",
        "a",
        "2026-03-10T10:00:00Z",
        json!({}),
    )]);
    for hostile in [
        "SELECT current_setting('allowed_paths')",
        "SELECT current_setting('temp_directory')",
        "SELECT getenv('HOME')",
        "SELECT * FROM duckdb_settings()",
        "SELECT * FROM duckdb_views()",
        "SELECT * FROM duckdb_temporary_files()",
        "SELECT * FROM glob('/*')",
        "SELECT * FROM events WHERE (SELECT count(*) FROM duckdb_databases()) > 0",
    ] {
        assert!(sql(&fixture, hostile).is_err(), "{hostile} was allowed");
    }
    let InsightResult::Sql {
        rows, truncated, ..
    } = sql(&fixture, "SELECT repeat('x', 5000000) AS c").unwrap()
    else {
        panic!()
    };
    let cell = rows[0][0].as_str().unwrap();
    assert!(cell.len() < 70 * 1024 && cell.ends_with("[truncated]"));
    assert!(!truncated);
    // Many large cells hit the response budget instead of growing it.
    let InsightResult::Sql {
        rows, truncated, ..
    } = sql(&fixture, "SELECT repeat('y', 60000) AS c FROM range(1000)").unwrap()
    else {
        panic!()
    };
    assert!(truncated && rows.len() < 200, "{} rows", rows.len());
}
