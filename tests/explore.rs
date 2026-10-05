//! Explore read path (persons, event feed, web analytics, catalog) checked
//! against brute-force Rust reference implementations on generated data.

#[path = "support/explore_data.rs"]
mod explore_data;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use explore_data::*;
use hoglet::capture::event::CapturedEvent;
use hoglet::contract::common::{BREAKDOWN_NONE, Interval, PropertyFilter};
use hoglet::contract::persons::EventListResponse;
use hoglet::contract::web::{WebDimension, WebQuery};
use hoglet::explore::events::{FeedQuery, feed};
use hoglet::explore::{ExploreError, Explorer, persons as explore_persons, web};
use hoglet::persons::PersonStore;
use hoglet::projection_catalog::ProjectionCatalog;
use hoglet::source::{EventFiles, EventSource};
use serde_json::{Value, json};

const PROJECT: &str = "project-a";
const OTHER_PROJECT: &str = "project-b";

/// Brute-force predicate deciding which events a filtered query keeps.
type Keep = Box<dyn Fn(&CapturedEvent) -> bool>;

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("valid timestamp")
        .with_timezone(&Utc)
}

fn spec(seed: u64) -> Spec {
    Spec {
        seed,
        humans: 250,
        start: at("2026-03-01T00:00:00Z"),
        days: 14,
        max_sessions_per_device: 4,
    }
}

/// The base fixture: project A plus a noisy project B sharing distinct ids.
fn base() -> (Fixture, Dataset) {
    let mut fixture = Fixture::new();
    let dataset = generate(&spec(7));
    let other = generate(&spec(8));
    fixture.load(PROJECT, &dataset.events);
    fixture.load(OTHER_PROJECT, &other.events);
    (fixture, dataset)
}

fn filters(value: Value) -> Vec<PropertyFilter> {
    serde_json::from_value(value).expect("filters")
}

fn web_query(from: &str, to: Option<&str>, interval: Option<Interval>, props: Value) -> WebQuery {
    WebQuery {
        date_from: from.to_owned(),
        date_to: to.map(str::to_owned),
        interval,
        properties: filters(props),
    }
}

fn overview(
    fixture: &Fixture,
    query: &WebQuery,
    now: DateTime<Utc>,
) -> hoglet::contract::web::WebOverview {
    let explorer = fixture.explorer.clone();
    fixture
        .explorer
        .with_connection(|connection| web::overview(&explorer, connection, PROJECT, query, now))
        .expect("overview")
}

fn check_metrics(
    label: &str,
    actual: &hoglet::contract::web::WebOverview,
    current: PeriodMetrics,
    previous: Option<PeriodMetrics>,
) {
    let pairs = [
        (
            "visitors",
            &actual.visitors,
            current.visitors,
            previous.map(|p| p.visitors),
        ),
        (
            "pageviews",
            &actual.pageviews,
            current.pageviews,
            previous.map(|p| p.pageviews),
        ),
        (
            "sessions",
            &actual.sessions,
            current.sessions,
            previous.map(|p| p.sessions),
        ),
        (
            "bounce",
            &actual.bounce_rate,
            current.bounce_rate,
            previous.map(|p| p.bounce_rate),
        ),
        (
            "duration",
            &actual.session_duration_s,
            current.duration_s,
            previous.map(|p| p.duration_s),
        ),
    ];
    for (name, metric, expected, expected_previous) in pairs {
        assert!(
            approx(metric.value, expected),
            "{label} {name}: got {} expected {expected}",
            metric.value
        );
        match (metric.previous, expected_previous) {
            (None, None) => {}
            (Some(got), Some(want)) => {
                assert!(
                    approx(got, want),
                    "{label} previous {name}: got {got} expected {want}"
                )
            }
            other => panic!("{label} previous {name}: mismatch {other:?}"),
        }
    }
}

fn person_fn(dataset: &Dataset) -> impl Fn(&str) -> String + '_ {
    |id: &str| {
        dataset
            .person_of
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned())
    }
}

#[test]
fn web_overview_matches_brute_force() {
    let (fixture, dataset) = base();
    let now = at("2026-03-20T00:00:00Z");
    let person = person_fn(&dataset);
    let cases: Vec<(&str, Value, Keep)> = vec![
        ("unfiltered", json!([]), Box::new(|_| true)),
        (
            "browser",
            json!([{"key": "$browser", "value": "Chrome"}]),
            Box::new(|e| text(e, "$browser").as_deref() == Some("Chrome")),
        ),
        (
            "plan json",
            json!([{"key": "plan", "operator": "exact", "value": ["pro"]}]),
            Box::new(|e| text(e, "plan").as_deref() == Some("pro")),
        ),
        (
            "not icontains path",
            json!([{"key": "$pathname", "operator": "not_icontains", "value": "PRIC"}]),
            Box::new(|e| !text(e, "$pathname").is_some_and(|p| p.contains("pric"))),
        ),
    ];
    for (label, props, keep) in cases {
        let query = web_query("2026-03-05", Some("2026-03-10"), Some(Interval::Day), props);
        let actual = overview(&fixture, &query, now);
        let from = micros(at("2026-03-05T00:00:00Z"));
        let to = micros(at("2026-03-11T00:00:00Z"));
        let prev = from - (to - from);
        let (tagged, sessions) = build_sessions(
            &dataset.events,
            &person,
            &[(prev, from), (from, to)],
            &*keep,
        );
        let current = period_metrics(&tagged, &sessions, 1);
        let previous = period_metrics(&tagged, &sessions, 0);
        assert!(
            current.sessions > 50.0,
            "{label}: dataset should be non-trivial"
        );
        check_metrics(label, &actual, current, Some(previous));

        assert_eq!(actual.interval, Interval::Day);
        assert_eq!(actual.days.len(), 6);
        assert_eq!(actual.days[0], "2026-03-05T00:00:00.000000Z");
        for (index, day) in actual.days.iter().enumerate() {
            let start = micros(at(day));
            let end = start + 86_400_000_000;
            let views: Vec<&(usize, String, CapturedEvent)> = tagged
                .iter()
                .filter(|(p, _, e)| {
                    *p == 1
                        && e.event == "$pageview"
                        && micros(e.timestamp) >= start
                        && micros(e.timestamp) < end
                })
                .collect();
            let visitors: HashSet<&String> = views.iter().map(|(_, person, _)| person).collect();
            assert_eq!(
                actual.pageviews_series[index],
                views.len() as u64,
                "{label} day {day}"
            );
            assert_eq!(
                actual.visitors_series[index],
                visitors.len() as u64,
                "{label} day {day}"
            );
        }
        assert_eq!(actual.live_visitors, 0);
    }

    // Hourly, auto interval, no previous mismatch at the period edge.
    let query = web_query(
        "2026-03-06T00:00:00Z",
        Some("2026-03-06T12:00:00Z"),
        None,
        json!([]),
    );
    let actual = overview(&fixture, &query, now);
    assert_eq!(actual.interval, Interval::Hour);
    assert_eq!(actual.days.len(), 12);
    let from = micros(at("2026-03-06T00:00:00Z"));
    let to = micros(at("2026-03-06T12:00:00Z"));
    let (tagged, sessions) = build_sessions(
        &dataset.events,
        &person,
        &[(from - (to - from), from), (from, to)],
        &|_| true,
    );
    check_metrics(
        "hourly",
        &actual,
        period_metrics(&tagged, &sessions, 1),
        Some(period_metrics(&tagged, &sessions, 0)),
    );
    let total: u64 = actual.pageviews_series.iter().sum();
    assert_eq!(total as f64, actual.pageviews.value);

    // `all` has no previous period and starts at the oldest event.
    let actual = overview(
        &fixture,
        &web_query("all", None, Some(Interval::Week), json!([])),
        now,
    );
    let (tagged, sessions) = build_sessions(
        &dataset.events,
        &person,
        &[(i64::MIN, i64::MIN), (i64::MIN + 1, i64::MAX)],
        &|_| true,
    );
    check_metrics("all", &actual, period_metrics(&tagged, &sessions, 1), None);
}

fn expected_breakdown(
    dataset: &Dataset,
    dimension: WebDimension,
    from: i64,
    to: i64,
) -> Vec<(String, u64, u64, Option<f64>)> {
    let person = person_fn(dataset);
    let (tagged, sessions) = build_sessions(
        &dataset.events,
        &person,
        &[(i64::MIN, i64::MIN), (from, to)],
        &|_| true,
    );
    let none = || BREAKDOWN_NONE.to_owned();
    let rate = |members: &[&Session]| {
        let bounced = members
            .iter()
            .filter(|s| s.is_bounce())
            .count();
        100.0 * bounced as f64 / members.len() as f64
    };
    let mut rows: Vec<(String, u64, u64, Option<f64>)> = Vec::new();
    match dimension {
        WebDimension::Page => {
            let mut views: BTreeMap<String, (HashSet<String>, u64)> = BTreeMap::new();
            for (_, person, event) in tagged.iter().filter(|(_, _, e)| e.event == "$pageview") {
                let entry = views
                    .entry(text(event, "$pathname").unwrap_or_else(none))
                    .or_default();
                entry.0.insert(person.clone());
                entry.1 += 1;
            }
            let mut entries: BTreeMap<String, Vec<&Session>> = BTreeMap::new();
            for session in &sessions {
                entries
                    .entry(session.entry_page.clone().unwrap_or_else(none))
                    .or_default()
                    .push(session);
            }
            for (value, (people, count)) in views {
                let bounce = entries.get(&value).map(|members| rate(members));
                rows.push((value, people.len() as u64, count, bounce));
            }
        }
        _ => {
            let key = |session: &Session| -> String {
                let entry = &session.entry;
                match dimension {
                    WebDimension::EntryPage => session.entry_page.clone().unwrap_or_else(none),
                    WebDimension::ExitPage => session.exit_page.clone().unwrap_or_else(none),
                    WebDimension::ReferringDomain => {
                        text(entry, "$referring_domain").unwrap_or_else(|| "$direct".to_owned())
                    }
                    WebDimension::UtmSource => text(entry, "utm_source").unwrap_or_else(none),
                    WebDimension::UtmMedium => text(entry, "utm_medium").unwrap_or_else(none),
                    WebDimension::UtmCampaign => text(entry, "utm_campaign").unwrap_or_else(none),
                    WebDimension::Browser => text(entry, "$browser").unwrap_or_else(none),
                    WebDimension::Os => text(entry, "$os").unwrap_or_else(none),
                    WebDimension::DeviceType => text(entry, "$device_type").unwrap_or_else(none),
                    WebDimension::Country => {
                        text(entry, "$geoip_country_code").unwrap_or_else(none)
                    }
                    WebDimension::Page => unreachable!(),
                }
            };
            let mut groups: BTreeMap<String, Vec<&Session>> = BTreeMap::new();
            for session in &sessions {
                groups.entry(key(session)).or_default().push(session);
            }
            let page_like = matches!(dimension, WebDimension::EntryPage | WebDimension::ExitPage);
            for (value, members) in groups {
                let people: HashSet<&String> = members.iter().map(|s| &s.person).collect();
                let views = if page_like {
                    members.len() as u64
                } else {
                    members.iter().map(|s| s.pageviews).sum()
                };
                rows.push((
                    value,
                    people.len() as u64,
                    views,
                    page_like.then(|| rate(&members)),
                ));
            }
        }
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    rows.truncate(100);
    rows
}

#[test]
fn web_breakdowns_match_brute_force() {
    let (fixture, dataset) = base();
    let now = at("2026-03-20T00:00:00Z");
    let query = web_query("2026-03-02", Some("2026-03-12"), None, json!([]));
    let from = micros(at("2026-03-02T00:00:00Z"));
    let to = micros(at("2026-03-13T00:00:00Z"));
    for dimension in [
        WebDimension::Page,
        WebDimension::EntryPage,
        WebDimension::ExitPage,
        WebDimension::ReferringDomain,
        WebDimension::UtmSource,
        WebDimension::UtmMedium,
        WebDimension::UtmCampaign,
        WebDimension::Browser,
        WebDimension::Os,
        WebDimension::DeviceType,
        WebDimension::Country,
    ] {
        let explorer = fixture.explorer.clone();
        let actual = fixture
            .explorer
            .with_connection(|connection| {
                web::breakdown(&explorer, connection, PROJECT, &query, dimension, 100, now)
            })
            .expect("breakdown");
        let expected = expected_breakdown(&dataset, dimension, from, to);
        assert!(!expected.is_empty());
        assert_eq!(actual.dimension, dimension);
        let got: Vec<(String, u64, u64)> = actual
            .rows
            .iter()
            .map(|row| (row.value.clone(), row.visitors, row.views))
            .collect();
        let want: Vec<(String, u64, u64)> = expected
            .iter()
            .map(|row| (row.0.clone(), row.1, row.2))
            .collect();
        assert_eq!(got, want, "{dimension:?}");
        for (row, expected) in actual.rows.iter().zip(&expected) {
            match (row.bounce_rate, expected.3) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert!(approx(a, b), "{dimension:?} {}: {a} vs {b}", row.value)
                }
                other => panic!("{dimension:?} {}: bounce {other:?}", row.value),
            }
        }
    }
    // Referring domain is session-entry attributed: internal navigation
    // never shows up as a referrer.
    let explorer = fixture.explorer.clone();
    let referrers = fixture
        .explorer
        .with_connection(|connection| {
            web::breakdown(
                &explorer,
                connection,
                PROJECT,
                &query,
                WebDimension::ReferringDomain,
                3,
                now,
            )
        })
        .expect("breakdown");
    assert_eq!(referrers.rows.len(), 3);
    assert!(referrers.rows.iter().all(|row| row.value != "example.com"));
}

/// A small dataset whose every number is computed by hand.
fn hand_events() -> Vec<CapturedEvent> {
    let day = "2026-04-01T";
    let t = |clock: &str| at(&format!("{day}{clock}:00Z"));
    vec![
        event(
            "$pageview",
            "anon-a",
            t("10:00"),
            json!({"$session_id": "S1", "$pathname": "/", "$referring_domain": "google.com"}),
        ),
        event(
            "$pageview",
            "anon-a",
            t("10:05"),
            json!({"$session_id": "S1", "$pathname": "/pricing", "$referring_domain": "example.com"}),
        ),
        event(
            "$identify",
            "user-1",
            t("10:06"),
            json!({"$session_id": "S1", "$anon_distinct_id": "anon-a", "$set": {"email": "one@example.com", "name": "One"}}),
        ),
        event(
            "$pageview",
            "anon-b",
            t("12:00"),
            json!({"$session_id": "S2", "$pathname": "/docs"}),
        ),
        event(
            "$identify",
            "user-1",
            t("12:01"),
            json!({"$session_id": "S2", "$anon_distinct_id": "anon-b"}),
        ),
        event(
            "$pageview",
            "anon-c",
            t("13:00"),
            json!({"$session_id": "S3", "$pathname": "/", "$referring_domain": "twitter.com"}),
        ),
        event(
            "$pageview",
            "user-2",
            t("14:00"),
            json!({"$pathname": "/blog"}),
        ),
        event(
            "$pageview",
            "user-2",
            t("14:10"),
            json!({"$pathname": "/about"}),
        ),
        event("$pageview", "user-2", t("15:00"), json!({"$pathname": "/"})),
        event("purchase", "user-1", t("16:00"), json!({"amount": 30})),
    ]
}

#[test]
fn hand_computed_sessions_merges_bounce_entry_exit() {
    let mut fixture = Fixture::new();
    fixture.load(PROJECT, &hand_events());
    let now = at("2026-04-10T00:00:00Z");
    let query = web_query(
        "2026-04-01",
        Some("2026-04-01"),
        Some(Interval::Day),
        json!([]),
    );
    let actual = overview(&fixture, &query, now);
    // user-1 (anon-a + anon-b merged), anon-c, user-2. Without merges: 4.
    assert_eq!(actual.visitors.value, 3.0);
    assert_eq!(actual.pageviews.value, 7.0);
    // S1, S2, S3, user-2 14:00-14:10, user-2 15:00. The sessionless
    // purchase has no pageview, so it is not a web session.
    assert_eq!(actual.sessions.value, 5.0);
    // S3 and user-2's 15:00 visit bounced; S2 has an $identify.
    assert!(approx(actual.bounce_rate.value, 40.0));
    // (360 + 60 + 0 + 600 + 0) / 5.
    assert!(approx(actual.session_duration_s.value, 204.0));
    assert_eq!(actual.visitors.previous, Some(0.0));
    assert_eq!(actual.days, vec!["2026-04-01T00:00:00.000000Z".to_owned()]);
    assert_eq!(actual.visitors_series, vec![3]);
    assert_eq!(actual.pageviews_series, vec![7]);

    let breakdown = |dimension| {
        let explorer = fixture.explorer.clone();
        fixture
            .explorer
            .with_connection(|connection| {
                web::breakdown(&explorer, connection, PROJECT, &query, dimension, 10, now)
            })
            .expect("breakdown")
            .rows
            .into_iter()
            .map(|row| {
                (
                    row.value,
                    row.visitors,
                    row.views,
                    row.bounce_rate.map(|r| (r * 100.0).round() / 100.0),
                )
            })
            .collect::<Vec<_>>()
    };
    let s = |text: &str| text.to_owned();
    assert_eq!(
        breakdown(WebDimension::EntryPage),
        vec![
            (s("/"), 3, 3, Some(66.67)),
            (s("/blog"), 1, 1, Some(0.0)),
            (s("/docs"), 1, 1, Some(0.0)),
        ]
    );
    assert_eq!(
        breakdown(WebDimension::ExitPage),
        vec![
            (s("/"), 2, 2, Some(100.0)),
            (s("/about"), 1, 1, Some(0.0)),
            (s("/docs"), 1, 1, Some(0.0)),
            (s("/pricing"), 1, 1, Some(0.0)),
        ]
    );
    assert_eq!(
        breakdown(WebDimension::Page),
        vec![
            (s("/"), 3, 3, Some(66.67)),
            (s("/about"), 1, 1, None),
            (s("/blog"), 1, 1, Some(0.0)),
            (s("/docs"), 1, 1, Some(0.0)),
            (s("/pricing"), 1, 1, None),
        ]
    );
    assert_eq!(
        breakdown(WebDimension::ReferringDomain),
        vec![
            (s("$direct"), 2, 4, None),
            (s("google.com"), 1, 2, None),
            (s("twitter.com"), 1, 1, None),
        ]
    );

    // Person detail: all distinct ids, all-time stats, lookup by merged id.
    let explorer = fixture.explorer.clone();
    let detail = fixture
        .explorer
        .with_connection(|connection| {
            explore_persons::detail(&explorer, connection, PROJECT, "anon-a")
        })
        .expect("detail");
    assert_eq!(detail.person.id, "user-1");
    assert_eq!(detail.person.display_name, "one@example.com");
    assert!(detail.person.is_identified);
    assert_eq!(detail.distinct_ids, vec!["anon-a", "user-1", "anon-b"]);
    assert_eq!(detail.event_count, 6);
    assert_eq!(
        detail.first_seen.as_deref(),
        Some("2026-04-01T10:00:00.000000Z")
    );
    assert_eq!(
        detail.last_seen.as_deref(),
        Some("2026-04-01T16:00:00.000000Z")
    );
    assert_eq!(detail.person.last_seen, detail.last_seen);
    // S1, S2, and the sessionless purchase.
    assert_eq!(detail.session_count, 3);
    let missing = fixture.explorer.with_connection(|connection| {
        explore_persons::detail(&explorer, connection, PROJECT, "nobody")
    });
    assert!(matches!(missing, Err(ExploreError::NotFound)));
    let other_project = fixture.explorer.with_connection(|connection| {
        explore_persons::detail(&explorer, connection, OTHER_PROJECT, "user-1")
    });
    assert!(matches!(other_project, Err(ExploreError::NotFound)));
}

#[test]
fn live_visitors_counts_persons_with_a_pageview_in_the_last_five_minutes() {
    let mut fixture = Fixture::new();
    let now = at("2026-05-01T00:02:00Z");
    let events = vec![
        event(
            "$pageview",
            "x-old",
            now - Duration::minutes(6),
            json!({"$session_id": "a"}),
        ),
        event(
            "$pageview",
            "x-1",
            now - Duration::minutes(4),
            json!({"$session_id": "b"}),
        ),
        event(
            "$pageview",
            "x-2",
            now - Duration::minutes(3),
            json!({"$session_id": "c"}),
        ),
        event(
            "$identify",
            "x-1",
            now - Duration::minutes(2),
            json!({"$anon_distinct_id": "x-2"}),
        ),
        event(
            "$pageview",
            "x-1",
            now - Duration::minutes(1),
            json!({"$session_id": "b"}),
        ),
        event(
            "$autocapture",
            "x-3",
            now - Duration::minutes(1),
            json!({"$session_id": "d"}),
        ),
    ];
    fixture.load(PROJECT, &events);
    let actual = overview(&fixture, &web_query("-24h", None, None, json!([])), now);
    // x-1 and x-2 are one person; x-old is too old; x-3 viewed no page.
    assert_eq!(actual.live_visitors, 1);
    assert_eq!(actual.visitors.value, 2.0);
}

#[test]
fn identity_overrides_sync_incrementally() {
    let mut fixture = Fixture::new();
    let day = at("2026-06-01T09:00:00Z");
    let first = vec![
        event("$pageview", "p-1", day, json!({"$session_id": "s1"})),
        event(
            "$pageview",
            "p-2",
            day + Duration::minutes(1),
            json!({"$session_id": "s2"}),
        ),
        event(
            "$pageview",
            "p-3",
            day + Duration::minutes(2),
            json!({"$session_id": "s3"}),
        ),
    ];
    fixture.load(PROJECT, &first);
    let now = at("2026-06-02T00:00:00Z");
    let query = web_query("2026-06-01", Some("2026-06-01"), None, json!([]));
    assert_eq!(overview(&fixture, &query, now).visitors.value, 3.0);

    let merge = vec![event(
        "$identify",
        "p-1",
        day + Duration::minutes(5),
        json!({"$anon_distinct_id": "p-2"}),
    )];
    fixture.load(PROJECT, &merge);
    assert_eq!(overview(&fixture, &query, now).visitors.value, 2.0);

    let dangerous = vec![event(
        "$merge_dangerously",
        "p-1",
        day + Duration::minutes(6),
        json!({"alias": "p-3"}),
    )];
    fixture.load(PROJECT, &dangerous);
    assert_eq!(overview(&fixture, &query, now).visitors.value, 1.0);
}

fn page_through(
    fixture: &Fixture,
    query: &FeedQuery,
    now: DateTime<Utc>,
) -> Vec<hoglet::contract::persons::EventRow> {
    let mut out = Vec::new();
    let mut query = query.clone();
    for _ in 0..10_000 {
        let explorer = fixture.explorer.clone();
        let page: EventListResponse = fixture
            .explorer
            .with_connection(|connection| feed(&explorer, connection, PROJECT, &query, now))
            .expect("feed");
        assert!(page.events.len() <= query.limit);
        out.extend(page.events);
        match page.next_before {
            Some(before) => query.before = Some(at(&before)),
            None => return out,
        }
    }
    panic!("feed did not terminate");
}

fn expected_feed<'a>(events: impl Iterator<Item = &'a CapturedEvent>) -> Vec<String> {
    let mut rows: Vec<&CapturedEvent> = events.collect();
    rows.sort_by(|a, b| {
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| b.uuid.to_string().cmp(&a.uuid.to_string()))
    });
    rows.into_iter().map(|e| e.uuid.to_string()).collect()
}

#[test]
fn feed_pages_are_complete_stable_and_resolved() {
    let mut fixture = Fixture::new();
    let mut dataset = generate(&spec(11));
    // Ties: five events in one microsecond, and history 200 days back.
    let tie = at("2026-03-07T12:00:00.123456Z");
    for index in 0..5 {
        dataset
            .events
            .push(event("tie", &format!("tie-{index}"), tie, json!({})));
    }
    for index in 0..3 {
        dataset.events.push(event(
            "ancient",
            "anon-0-0",
            at("2025-08-01T00:00:00Z") + Duration::hours(index),
            json!({}),
        ));
    }
    dataset.events.sort_by_key(|e| e.timestamp);
    fixture.load(PROJECT, &dataset.events);
    let now = at("2026-03-15T00:00:00Z");

    let all = page_through(
        &fixture,
        &FeedQuery {
            limit: 37,
            ..FeedQuery::default()
        },
        now,
    );
    let uuids: Vec<String> = all.iter().map(|row| row.uuid.clone()).collect();
    assert_eq!(uuids, expected_feed(dataset.events.iter()));
    for row in &all {
        let expected = dataset
            .person_of
            .get(&row.distinct_id)
            .cloned()
            .unwrap_or_else(|| row.distinct_id.clone());
        assert_eq!(row.person_id, expected, "person of {}", row.distinct_id);
        assert!(row.properties.is_object());
    }

    let cases: Vec<(FeedQuery, Keep)> = vec![
        (
            FeedQuery {
                event: Some("$identify".into()),
                limit: 50,
                ..FeedQuery::default()
            },
            Box::new(|e| e.event == "$identify"),
        ),
        (
            FeedQuery {
                distinct_ids: Some(vec!["anon-3-0".into()]),
                limit: 5,
                ..FeedQuery::default()
            },
            Box::new(|e| e.distinct_id == "anon-3-0"),
        ),
        (
            FeedQuery {
                filters: filters(json!([{"key": "$browser", "value": ["Chrome", "Safari"]}])),
                limit: 200,
                ..FeedQuery::default()
            },
            Box::new(|e| matches!(text(e, "$browser").as_deref(), Some("Chrome" | "Safari"))),
        ),
        (
            FeedQuery {
                filters: filters(json!([
                    {"key": "plan", "operator": "icontains", "value": "PR"},
                    {"key": "$session_id", "operator": "is_not_set"}
                ])),
                limit: 100,
                ..FeedQuery::default()
            },
            Box::new(|e| {
                text(e, "plan").as_deref() == Some("pro") && text(e, "$session_id").is_none()
            }),
        ),
        (
            FeedQuery {
                filters: filters(json!([{"key": "amount", "operator": "gt", "value": 100}])),
                limit: 100,
                ..FeedQuery::default()
            },
            Box::new(|e| {
                e.properties
                    .get("amount")
                    .and_then(Value::as_f64)
                    .is_some_and(|a| a > 100.0)
            }),
        ),
        (
            FeedQuery {
                filters: filters(
                    json!([{"key": "$pathname", "operator": "regex", "value": "^/(docs|blog)$"}]),
                ),
                limit: 100,
                ..FeedQuery::default()
            },
            Box::new(|e| matches!(text(e, "$pathname").as_deref(), Some("/docs" | "/blog"))),
        ),
    ];
    for (query, keep) in cases {
        let got: Vec<String> = page_through(&fixture, &query, now)
            .into_iter()
            .map(|row| row.uuid)
            .collect();
        let want = expected_feed(dataset.events.iter().filter(|e| keep(e)));
        assert!(!want.is_empty(), "{query:?} should match something");
        assert_eq!(got, want, "{query:?}");
    }

    // A person's events: all of their distinct ids, newest first.
    let human = dataset
        .person_of
        .iter()
        .filter(|(id, person)| *id != *person)
        .map(|(_, person)| person.clone())
        .next()
        .expect("a merged person");
    let explorer = fixture.explorer.clone();
    let page = fixture
        .explorer
        .with_connection(|connection| {
            explore_persons::person_events(&explorer, connection, PROJECT, &human, None, 200, now)
        })
        .expect("person events");
    let want = expected_feed(
        dataset
            .events
            .iter()
            .filter(|e| dataset.person_of.get(&e.distinct_id) == Some(&human)),
    );
    let got: Vec<String> = page.events.iter().map(|row| row.uuid.clone()).collect();
    assert_eq!(got, want[..want.len().min(200)].to_vec());
    assert!(page.events.iter().all(|row| row.person_id == human));

    // Bad regexes are client errors, not server errors.
    let bad = FeedQuery {
        filters: filters(json!([{"key": "$pathname", "operator": "regex", "value": "("}])),
        limit: 10,
        ..FeedQuery::default()
    };
    let result = fixture
        .explorer
        .with_connection(|connection| feed(&explorer, connection, PROJECT, &bad, now));
    assert!(matches!(result, Err(ExploreError::Invalid { .. })));
}

/// Records which day ranges were read (excluding existence probes).
struct CountingSource {
    inner: Arc<hoglet::source::DirectorySource>,
    reads: Mutex<Vec<(NaiveDate, NaiveDate, usize)>>,
}

impl EventSource for CountingSource {
    fn files(&self, project_id: &str, from: NaiveDate, to_exclusive: NaiveDate) -> EventFiles {
        let files = self.inner.files(project_id, from, to_exclusive);
        self.reads
            .lock()
            .expect("lock")
            .push((from, to_exclusive, files.paths.len()));
        files
    }
}

#[test]
fn feed_reads_only_recent_windows_when_recent_events_exist() {
    let mut fixture = Fixture::new();
    let now = at("2026-07-10T12:00:00Z");
    let mut events = Vec::new();
    for index in 0..150 {
        events.push(event(
            "recent",
            "r",
            now - Duration::minutes(index),
            json!({}),
        ));
    }
    for day in 1..300 {
        events.push(event("old", "o", now - Duration::days(day), json!({})));
    }
    make_strictly_increasing(&mut events);
    fixture.load(PROJECT, &events);
    let counting = Arc::new(CountingSource {
        inner: fixture.source.clone(),
        reads: Mutex::new(Vec::new()),
    });
    let explorer =
        Arc::new(Explorer::new(counting.clone(), fixture.persons.clone()).expect("explorer"));
    let page = explorer
        .with_connection(|connection| {
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
        })
        .expect("feed");
    assert_eq!(page.events.len(), 100);
    assert!(page.events.iter().all(|row| row.event == "recent"));
    let reads = counting.reads.lock().expect("lock").clone();
    let scanned: Vec<_> = reads
        .iter()
        .filter(|(from, _, _)| *from != NaiveDate::MIN)
        .collect();
    assert_eq!(scanned.len(), 1, "one window should suffice: {scanned:?}");
    assert!(scanned[0].1 - scanned[0].0 <= chrono::TimeDelta::days(2));

    // Paging into history widens windows instead of reading day by day.
    let all = {
        let mut out = 0;
        let mut query = FeedQuery {
            limit: 200,
            ..FeedQuery::default()
        };
        loop {
            let page = explorer
                .with_connection(|connection| feed(&explorer, connection, PROJECT, &query, now))
                .expect("feed");
            out += page.events.len();
            match page.next_before {
                Some(before) => query.before = Some(at(&before)),
                None => break out,
            }
        }
    };
    assert_eq!(all, events.len());
    let windows = counting
        .reads
        .lock()
        .expect("lock")
        .iter()
        .filter(|(from, _, _)| *from != NaiveDate::MIN)
        .count();
    assert!(
        windows < 40,
        "walk-back should be logarithmic, got {windows} windows"
    );
}

fn all_person_ids(fixture: &Fixture, project_id: &str) -> Vec<(String, String)> {
    let connection = rusqlite::Connection::open(&fixture.projections).expect("open");
    let mut statement = connection
        .prepare("SELECT created_at, id FROM persons WHERE project_id = ?1 ORDER BY created_at DESC, id DESC")
        .expect("prepare");
    statement
        .query_map([project_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn list_all(
    store: &PersonStore,
    search: Option<&str>,
    limit: usize,
) -> Vec<hoglet::contract::persons::PersonSummary> {
    let mut out = Vec::new();
    let mut cursor = None;
    for _ in 0..10_000 {
        let page =
            explore_persons::list(store, PROJECT, search, cursor.as_ref(), limit).expect("list");
        assert!(page.persons.len() <= limit);
        out.extend(page.persons);
        match page.next_cursor {
            Some(next) => cursor = Some(explore_persons::decode_cursor(&next).expect("cursor")),
            None => return out,
        }
    }
    panic!("persons paging did not terminate");
}

#[test]
fn persons_list_pages_newest_first_and_searches() {
    let (fixture, dataset) = base();
    let expected = all_person_ids(&fixture, PROJECT);
    let listed = list_all(&fixture.persons, None, 23);
    let got: Vec<(String, String)> = listed
        .iter()
        .map(|p| (p.created_at.clone(), p.id.clone()))
        .collect();
    assert_eq!(got, expected);
    // Ground truth: one person per non-merged identity.
    let truth: HashSet<&String> = dataset.person_of.values().collect();
    assert_eq!(listed.len(), truth.len());
    for person in &listed {
        assert!(person.distinct_ids.len() <= 10);
        assert!(!person.distinct_ids.is_empty());
        if person.id.starts_with("user-") {
            let human = person.id.trim_start_matches("user-");
            assert_eq!(person.display_name, format!("person{human}@example.com"));
            assert!(person.is_identified);
        } else {
            assert_eq!(person.display_name, person.distinct_ids[0]);
        }
        let newest = dataset
            .events
            .iter()
            .filter(|event| dataset.person_of.get(&event.distinct_id) == Some(&person.id))
            .map(|event| event.timestamp)
            .max();
        let listed_last = person
            .last_seen
            .as_deref()
            .map(|text| chrono::DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&chrono::Utc));
        assert_eq!(listed_last, newest, "last_seen of {}", person.id);
    }
    // Exact limit boundary: a full last page still terminates.
    assert_eq!(
        list_all(&fixture.persons, None, expected.len()).len(),
        expected.len()
    );
    assert_eq!(list_all(&fixture.persons, None, 100).len(), expected.len());

    // Prefix search on distinct ids.
    let by_prefix: HashSet<String> = list_all(&fixture.persons, Some("user-1"), 7)
        .into_iter()
        .map(|p| p.id)
        .collect();
    let want: HashSet<String> = dataset
        .person_of
        .iter()
        .filter(|(id, _)| id.starts_with("user-1"))
        .map(|(_, person)| person.clone())
        .collect();
    assert_eq!(by_prefix, want);
    // Prefix through a merged anonymous id finds the merged person.
    let anon = dataset
        .person_of
        .iter()
        .find(|(id, person)| id.starts_with("anon-") && person.starts_with("user-"))
        .expect("merged anon");
    let found: Vec<String> = list_all(&fixture.persons, Some(anon.0), 10)
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert!(found.contains(anon.1));

    // Case-insensitive substring on email and name.
    let by_email: Vec<String> = list_all(&fixture.persons, Some("PERSON5@EXAMPLE"), 10)
        .into_iter()
        .map(|p| p.id)
        .collect();
    let want_email: Vec<String> = if truth.contains(&"user-5".to_owned()) {
        vec!["user-5".to_owned()]
    } else {
        Vec::new()
    };
    assert_eq!(by_email, want_email);
    let by_name: HashSet<String> = list_all(&fixture.persons, Some("person 7"), 50)
        .into_iter()
        .map(|p| p.id)
        .collect();
    let want_name: HashSet<String> = truth
        .iter()
        .filter(|id| {
            id.starts_with("user-")
                && format!("person {}", id.trim_start_matches("user-")).contains("person 7")
        })
        .map(|id| (*id).clone())
        .collect();
    assert_eq!(by_name, want_name);
    assert!(list_all(&fixture.persons, Some("zzz-nobody"), 10).is_empty());
    // Over-long searches are truncated, not rejected.
    assert!(list_all(&fixture.persons, Some(&"x".repeat(10_000)), 10).is_empty());
}

#[test]
fn person_detail_stats_match_brute_force() {
    let (fixture, dataset) = base();
    let mut groups: HashMap<String, Vec<&CapturedEvent>> = HashMap::new();
    for event in &dataset.events {
        let person = dataset
            .person_of
            .get(&event.distinct_id)
            .cloned()
            .unwrap_or_else(|| event.distinct_id.clone());
        groups.entry(person).or_default().push(event);
    }
    let mut checked = 0;
    for (person, events) in groups.iter().take(60) {
        let explorer = fixture.explorer.clone();
        let detail = fixture
            .explorer
            .with_connection(|connection| {
                explore_persons::detail(&explorer, connection, PROJECT, person)
            })
            .expect("detail");
        assert_eq!(&detail.person.id, person);
        let ids: HashSet<&String> = events.iter().map(|e| &e.distinct_id).collect();
        let listed: HashSet<&String> = detail.distinct_ids.iter().collect();
        assert!(ids.is_subset(&listed), "{person}: {ids:?} vs {listed:?}");
        assert_eq!(detail.event_count, events.len() as u64);
        let first = events.iter().map(|e| e.timestamp).min().expect("first");
        let last = events.iter().map(|e| e.timestamp).max().expect("last");
        assert_eq!(detail.first_seen.as_deref().map(at), Some(first));
        assert_eq!(detail.last_seen.as_deref().map(at), Some(last));
        let mut session_ids = HashSet::new();
        let mut sessionless = 0;
        let mut previous: Option<i64> = None;
        for event in events {
            match text(event, "$session_id") {
                Some(session) => {
                    session_ids.insert(session);
                }
                None => {
                    let ts = micros(event.timestamp);
                    if previous.is_none_or(|p| ts - p > 30 * 60 * 1_000_000) {
                        sessionless += 1;
                    }
                    previous = Some(ts);
                }
            }
        }
        assert_eq!(
            detail.session_count,
            (session_ids.len() + sessionless) as u64,
            "{person}"
        );
        checked += 1;
    }
    assert_eq!(checked, 60);
}

#[test]
fn catalog_matches_brute_force() {
    let (fixture, dataset) = base();
    let catalog = ProjectionCatalog::open(&fixture.projections).expect("catalog");

    let mut names: HashMap<&str, u64> = HashMap::new();
    let mut keys: HashMap<&str, u64> = HashMap::new();
    let mut browsers: HashMap<String, u64> = HashMap::new();
    for event in &dataset.events {
        *names.entry(&event.event).or_default() += 1;
        for key in event.properties.keys() {
            *keys.entry(key).or_default() += 1;
        }
        if let Some(browser) = text(event, "$browser") {
            *browsers.entry(browser).or_default() += 1;
        }
    }
    let events = catalog.event_names(PROJECT, "", 200).expect("events");
    assert_eq!(events.len(), names.len());
    for row in &events {
        assert_eq!(row.count, names[row.name.as_str()], "{}", row.name);
        assert!(row.last_seen.is_some());
    }
    assert!(events.windows(2).all(|pair| pair[0].count >= pair[1].count));
    let searched = catalog.event_names(PROJECT, "PAGE", 200).expect("events");
    assert_eq!(
        searched.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["$pageview"]
    );

    let properties = catalog
        .property_keys(PROJECT, hoglet::contract::common::PropertySource::Event, "")
        .expect("keys");
    assert_eq!(properties.len(), keys.len());
    for row in &properties {
        assert_eq!(row.count, keys[row.key.as_str()], "{}", row.key);
        assert_eq!(row.source, "event");
    }
    let amount = properties
        .iter()
        .find(|p| p.key == "amount")
        .expect("amount");
    assert_eq!(amount.property_type, "number");
    let set = properties.iter().find(|p| p.key == "$set").expect("$set");
    assert_eq!(set.property_type, "object");

    let values = catalog
        .property_values(
            PROJECT,
            hoglet::contract::common::PropertySource::Event,
            "$browser",
            "",
            100,
        )
        .expect("values");
    assert_eq!(values.len(), browsers.len());
    for row in &values {
        assert_eq!(row.count, browsers[&row.value]);
    }
    let fire = catalog
        .property_values(
            PROJECT,
            hoglet::contract::common::PropertySource::Event,
            "$browser",
            "FIRE",
            100,
        )
        .expect("values");
    assert_eq!(
        fire.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(),
        vec!["Firefox"]
    );

    // Person keys and values come from person properties.
    let identified: HashSet<&String> = dataset
        .person_of
        .values()
        .filter(|person| person.starts_with("user-"))
        .collect();
    let person_keys = catalog
        .property_keys(
            PROJECT,
            hoglet::contract::common::PropertySource::Person,
            "",
        )
        .expect("person keys");
    let mut person_key_counts: BTreeMap<&str, u64> = BTreeMap::new();
    for key in &person_keys {
        assert_eq!(key.source, "person");
        assert_eq!(key.property_type, "string");
        person_key_counts.insert(&key.key, key.count);
    }
    let n = identified.len() as u64;
    assert_eq!(
        person_key_counts,
        BTreeMap::from([("email", n), ("name", n), ("plan", n)])
    );
    let plans = catalog
        .property_values(
            PROJECT,
            hoglet::contract::common::PropertySource::Person,
            "plan",
            "",
            10,
        )
        .expect("plan values");
    assert_eq!(plans.iter().map(|v| v.count).sum::<u64>(), n);
    assert!(plans.iter().all(|v| v.value == "free" || v.value == "pro"));
    let emails = catalog
        .property_values(
            PROJECT,
            hoglet::contract::common::PropertySource::Person,
            "email",
            "person1",
            100,
        )
        .expect("emails");
    assert!(
        emails
            .iter()
            .all(|v| v.value.starts_with("person1") && v.count == 1)
    );
    assert!(
        catalog
            .property_values(
                PROJECT,
                hoglet::contract::common::PropertySource::Event,
                "",
                "",
                10
            )
            .is_err()
    );
}
