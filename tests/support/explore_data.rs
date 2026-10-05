//! Realistic event datasets, a lake + projection fixture, and brute-force
//! reference implementations of the explore metrics.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use hoglet::capture::event::CapturedEvent;
use hoglet::explore::Explorer;
use hoglet::persons::PersonStore;
use hoglet::pipeline::wal::WalCursor;
use hoglet::source::DirectorySource;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use uuid::Uuid;

pub const PATHS: &[&str] = &["/", "/pricing", "/docs", "/blog", "/signup", "/about"];
const BROWSERS: &[&str] = &["Chrome", "Firefox", "Safari"];
const OSES: &[&str] = &["Windows", "Mac OS X", "iOS", "Android"];
const DEVICES: &[&str] = &["Desktop", "Mobile"];
const COUNTRIES: &[&str] = &["US", "DE", "IN", "GB"];
const REFERRERS: &[&str] = &["google.com", "news.ycombinator.com", "twitter.com"];
const UTM_SOURCES: &[&str] = &["newsletter", "google", "twitter"];
const UTM_MEDIUMS: &[&str] = &["email", "cpc", "social"];
const UTM_CAMPAIGNS: &[&str] = &["launch", "spring", "retarget"];

pub struct Spec {
    pub seed: u64,
    pub humans: usize,
    pub start: DateTime<Utc>,
    pub days: i64,
    pub max_sessions_per_device: usize,
}

pub struct Dataset {
    /// Sorted by timestamp, timestamps strictly increasing.
    pub events: Vec<CapturedEvent>,
    /// Ground truth: distinct id -> the person it must resolve to.
    pub person_of: HashMap<String, String>,
}

fn pick<'a>(rng: &mut StdRng, items: &[&'a str]) -> &'a str {
    items[rng.gen_range(0..items.len())]
}

pub fn event(
    name: &str,
    distinct_id: &str,
    timestamp: DateTime<Utc>,
    properties: Value,
) -> CapturedEvent {
    let properties = match properties {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    CapturedEvent {
        uuid: Uuid::new_v4(),
        event: name.to_owned(),
        distinct_id: distinct_id.to_owned(),
        token: "phc_test".to_owned(),
        timestamp,
        properties,
    }
}

pub fn generate(spec: &Spec) -> Dataset {
    let mut rng = StdRng::seed_from_u64(spec.seed);
    let mut events = Vec::new();
    let mut person_of = HashMap::new();
    let span_seconds = spec.days * 86_400;
    for human in 0..spec.humans {
        let identifies = rng.gen_bool(0.6);
        let user_id = format!("user-{human}");
        let devices = if identifies { rng.gen_range(1..=3) } else { 1 };
        let plan = if rng.gen_bool(0.5) { "free" } else { "pro" };
        for device in 0..devices {
            let anon = format!("anon-{human}-{device}");
            person_of.insert(
                anon.clone(),
                if identifies {
                    user_id.clone()
                } else {
                    anon.clone()
                },
            );
            let browser = pick(&mut rng, BROWSERS);
            let os = pick(&mut rng, OSES);
            let device_type = pick(&mut rng, DEVICES);
            let country = if rng.gen_bool(0.1) {
                None
            } else {
                Some(pick(&mut rng, COUNTRIES))
            };
            let sessions = rng.gen_range(1..=spec.max_sessions_per_device);
            let login_session = identifies.then(|| rng.gen_range(0..sessions));
            let mut starts: Vec<i64> = (0..sessions)
                .map(|_| rng.gen_range(0..span_seconds.max(1)))
                .collect();
            starts.sort_unstable();
            let mut current_id = anon.clone();
            for (index, start) in starts.into_iter().enumerate() {
                let mut at = spec.start + Duration::seconds(start);
                let session_id = rng.gen_bool(0.85).then(|| Uuid::new_v4().to_string());
                let pageviews = if rng.gen_bool(0.4) {
                    1
                } else {
                    rng.gen_range(2..=5)
                };
                let direct = rng.gen_bool(0.3);
                let referrer = pick(&mut rng, REFERRERS);
                let tagged = rng.gen_bool(0.5);
                let utm = (
                    pick(&mut rng, UTM_SOURCES),
                    pick(&mut rng, UTM_MEDIUMS),
                    pick(&mut rng, UTM_CAMPAIGNS),
                );
                for view in 0..pageviews {
                    let path = pick(&mut rng, PATHS);
                    let mut properties = json!({
                        "$pathname": path,
                        "$current_url": format!("https://example.com{path}"),
                        "$host": "example.com",
                        "$browser": browser,
                        "$os": os,
                        "$device_type": device_type,
                        "$lib": "web",
                        "plan": plan,
                    });
                    let props = properties.as_object_mut().expect("object");
                    if let Some(country) = country {
                        props.insert("$geoip_country_code".into(), json!(country));
                    }
                    if let Some(session_id) = &session_id {
                        props.insert("$session_id".into(), json!(session_id));
                    }
                    if view == 0 {
                        props.insert(
                            "$referring_domain".into(),
                            json!(if direct { "$direct" } else { referrer }),
                        );
                        if tagged {
                            props.insert("utm_source".into(), json!(utm.0));
                            props.insert("utm_medium".into(), json!(utm.1));
                            props.insert("utm_campaign".into(), json!(utm.2));
                        }
                    } else {
                        // Multi-page app: internal navigations refer to self.
                        props.insert("$referring_domain".into(), json!("example.com"));
                    }
                    events.push(event("$pageview", &current_id, at, properties));
                    at += Duration::seconds(rng.gen_range(5..600));
                    if view == 0 && login_session == Some(index) && current_id == anon {
                        let mut identify = json!({
                            "$anon_distinct_id": anon,
                            "$set": {
                                "email": format!("person{human}@example.com"),
                                "name": format!("Person {human}"),
                                "plan": plan,
                            },
                        });
                        if let Some(session_id) = &session_id {
                            identify
                                .as_object_mut()
                                .expect("object")
                                .insert("$session_id".into(), json!(session_id));
                        }
                        events.push(event("$identify", &user_id, at, identify));
                        current_id = user_id.clone();
                        at += Duration::seconds(rng.gen_range(5..60));
                    }
                    if rng.gen_bool(0.25) {
                        let mut click = json!({"$event_type": "click", "$pathname": path});
                        if let Some(session_id) = &session_id {
                            click
                                .as_object_mut()
                                .expect("object")
                                .insert("$session_id".into(), json!(session_id));
                        }
                        events.push(event("$autocapture", &current_id, at, click));
                        at += Duration::seconds(rng.gen_range(5..300));
                    }
                }
            }
        }
        if identifies {
            person_of.insert(user_id.clone(), user_id.clone());
            for _ in 0..rng.gen_range(0..=2) {
                let at = spec.start + Duration::seconds(rng.gen_range(0..span_seconds.max(1)));
                events.push(event(
                    "purchase",
                    &user_id,
                    at,
                    json!({"amount": rng.gen_range(5..500), "currency": "USD"}),
                ));
            }
        }
    }
    make_strictly_increasing(&mut events);
    Dataset { events, person_of }
}

pub fn make_strictly_increasing(events: &mut [CapturedEvent]) {
    events.sort_by_key(|event| event.timestamp);
    for index in 1..events.len() {
        if events[index].timestamp <= events[index - 1].timestamp {
            events[index].timestamp = events[index - 1].timestamp + Duration::microseconds(1);
        }
    }
}

/// A bootstrapped projections database, a directory lake, and an explorer.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub source: Arc<DirectorySource>,
    pub persons: Arc<PersonStore>,
    pub explorer: Arc<Explorer>,
    pub projections: PathBuf,
    pub control: PathBuf,
    applied: u64,
    files: u64,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let control = dir.path().join("control.db");
        let projections = dir.path().join("projections.db");
        hoglet::storage_bootstrap::bootstrap_storage(&control, &projections)
            .expect("storage bootstrap");
        let connection = Connection::open(&projections).expect("projections open");
        hoglet::projections::initialize_schema(&connection).expect("projection schema");
        drop(connection);
        let source = Arc::new(DirectorySource::new(dir.path().join("events")));
        let persons = Arc::new(PersonStore::open(&projections).expect("person store"));
        let explorer = Arc::new(Explorer::new(source.clone(), persons.clone()).expect("explorer"));
        Self {
            dir,
            source,
            persons,
            explorer,
            projections,
            control,
            applied: 0,
            files: 0,
        }
    }

    /// Write `events` as Parquet, `files_per_day` files per UTC day.
    pub fn write_events(
        &mut self,
        project_id: &str,
        events: &[CapturedEvent],
        files_per_day: usize,
    ) {
        let mut by_day: BTreeMap<NaiveDate, Vec<CapturedEvent>> = BTreeMap::new();
        for event in events {
            by_day
                .entry(event.timestamp.date_naive())
                .or_default()
                .push(event.clone());
        }
        for (day, day_events) in by_day {
            let directory = self.source.partition_dir(project_id, day);
            std::fs::create_dir_all(&directory).expect("partition dir");
            let chunk = day_events.len().div_ceil(files_per_day.max(1)).max(1);
            for part in day_events.chunks(chunk) {
                self.files += 1;
                let path = directory.join(format!("part-{:06}.parquet", self.files));
                hoglet::lake::parquet::write_file(part, &path).expect("parquet write");
            }
        }
    }

    /// Apply events to the projections in order (WAL order = slice order).
    pub fn project<'a>(
        &mut self,
        project_id: &str,
        events: impl IntoIterator<Item = &'a CapturedEvent>,
    ) {
        let mut connection = Connection::open(&self.projections).expect("projections open");
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=OFF;")
            .expect("pragmas");
        let transaction = connection.transaction().expect("transaction");
        for event in events {
            self.applied += 1;
            hoglet::projections::apply_captured_event(
                &transaction,
                project_id,
                event,
                WalCursor::new(1, self.applied),
            )
            .expect("projection apply");
        }
        transaction.commit().expect("commit");
    }

    /// Write and project, the common case.
    pub fn load(&mut self, project_id: &str, events: &[CapturedEvent]) {
        self.write_events(project_id, events, 2);
        self.project(project_id, events.iter());
    }
}

// ---------------------------------------------------------------------------
// Brute-force reference implementations.
// ---------------------------------------------------------------------------

pub fn micros(at: DateTime<Utc>) -> i64 {
    at.timestamp_micros()
}

pub fn text(event: &CapturedEvent, key: &str) -> Option<String> {
    match event.properties.get(key)? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub period: usize,
    pub person: String,
    pub pageviews: u64,
    pub events: u64,
    pub autocaptures: u64,
    pub duration_us: i64,
    pub entry_page: Option<String>,
    pub exit_page: Option<String>,
    /// First pageview of the session.
    pub entry: CapturedEvent,
}

/// Tag every in-range, matching event with (period, person) and group
/// into pageview sessions, exactly per the documented definitions.
pub fn build_sessions(
    events: &[CapturedEvent],
    person: &dyn Fn(&str) -> String,
    periods: &[(i64, i64)],
    keep: &dyn Fn(&CapturedEvent) -> bool,
) -> (Vec<(usize, String, CapturedEvent)>, Vec<Session>) {
    let mut tagged = Vec::new();
    for event in events {
        let ts = micros(event.timestamp);
        let Some(period) = periods
            .iter()
            .position(|(from, to)| ts >= *from && ts < *to)
        else {
            continue;
        };
        if !keep(event) {
            continue;
        }
        tagged.push((period, person(&event.distinct_id), event.clone()));
    }
    tagged.sort_by_key(|(_, _, event)| (event.timestamp, event.uuid.to_string()));
    let mut groups: BTreeMap<(usize, String), Vec<(String, CapturedEvent)>> = BTreeMap::new();
    let mut last_seen: HashMap<(usize, String), (i64, u64)> = HashMap::new();
    for (period, person, event) in &tagged {
        let sid = match text(event, "$session_id") {
            Some(session) => format!("s{session}"),
            None => {
                let ts = micros(event.timestamp);
                let key = (*period, person.clone());
                let counter = match last_seen.get(&key) {
                    Some((previous, counter)) if ts - previous <= 30 * 60 * 1_000_000 => *counter,
                    Some((_, counter)) => counter + 1,
                    None => 1,
                };
                last_seen.insert(key, (ts, counter));
                format!("a{person}\u{1f}{counter}")
            }
        };
        groups
            .entry((*period, sid))
            .or_default()
            .push((person.clone(), event.clone()));
    }
    let mut out = Vec::new();
    for ((period, _), members) in groups {
        let pageviews: Vec<&(String, CapturedEvent)> = members
            .iter()
            .filter(|(_, event)| event.event == "$pageview")
            .collect();
        let Some(first) = pageviews.first() else {
            continue;
        };
        let last = pageviews.last().expect("non-empty");
        let min = members
            .iter()
            .map(|(_, e)| micros(e.timestamp))
            .min()
            .unwrap_or(0);
        let max = members
            .iter()
            .map(|(_, e)| micros(e.timestamp))
            .max()
            .unwrap_or(0);
        out.push(Session {
            period,
            person: first.0.clone(),
            pageviews: pageviews.len() as u64,
            events: members.len() as u64,
            autocaptures: members
                .iter()
                .filter(|(_, event)| event.event == "$autocapture")
                .count() as u64,
            duration_us: max - min,
            entry_page: text(&first.1, "$pathname"),
            exit_page: text(&last.1, "$pathname"),
            entry: first.1.clone(),
        });
    }
    (tagged, out)
}

impl Session {
    /// PostHog's bounce: one pageview, no autocapture, under 10 seconds.
    pub fn is_bounce(&self) -> bool {
        self.pageviews == 1 && self.autocaptures == 0 && self.duration_us < 10_000_000
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PeriodMetrics {
    pub visitors: f64,
    pub pageviews: f64,
    pub sessions: f64,
    pub bounce_rate: f64,
    pub duration_s: f64,
}

pub fn period_metrics(
    tagged: &[(usize, String, CapturedEvent)],
    sessions: &[Session],
    period: usize,
) -> PeriodMetrics {
    let views: Vec<&(usize, String, CapturedEvent)> = tagged
        .iter()
        .filter(|(p, _, event)| *p == period && event.event == "$pageview")
        .collect();
    let visitors: HashSet<&String> = views.iter().map(|(_, person, _)| person).collect();
    let mine: Vec<&Session> = sessions.iter().filter(|s| s.period == period).collect();
    let bounces = mine
        .iter()
        .filter(|s| s.is_bounce())
        .count();
    let count = mine.len() as f64;
    PeriodMetrics {
        visitors: visitors.len() as f64,
        pageviews: views.len() as f64,
        sessions: count,
        bounce_rate: if count > 0.0 {
            100.0 * bounces as f64 / count
        } else {
            0.0
        },
        duration_s: if count > 0.0 {
            mine.iter().map(|s| s.duration_us as f64).sum::<f64>() / count / 1e6
        } else {
            0.0
        },
    }
}

pub fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * (1.0 + a.abs().max(b.abs()))
}
