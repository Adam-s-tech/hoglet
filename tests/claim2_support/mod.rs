//! Shared harness for the claim 2 evidence: drive a real `hoglet` process
//! (built with the `fault-injection` feature), crash it at a named point,
//! restart it on the same data directory, and reconcile what the clients were
//! told (HTTP 200) against what is actually stored.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Value, json};
use uuid::Uuid;

pub const TOKEN: &str = "phc_claim2";
pub const EVENT: &str = "claim2_event";
pub const VICTIM_EVENT: &str = "claim2_victim_event";

/// Iterations per kill point; raise for nightly or one-off high-count runs.
pub fn seeds() -> u64 {
    std::env::var("HOGLET_FAULT_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2)
}

/// Ports are handed out from a per-process range and probed, so two servers
/// of one test run never share one and a stranger's listener is skipped.
pub fn free_port() -> u16 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = 20_000 + (u64::from(std::process::id()) * 53) % 30_000;
    loop {
        let offset = NEXT.fetch_add(1, Ordering::SeqCst);
        let port = (base + offset) % 40_000 + 20_000;
        if TcpListener::bind(("127.0.0.1", port as u16)).is_ok() {
            return port as u16;
        }
    }
}

// ---------------------------------------------------------------- process

pub struct Server {
    pub child: Child,
    pub port: u16,
    stderr_path: PathBuf,
}

impl Server {
    /// Spawn on `data_dir`; `env` arms faults. Retries on a port race.
    pub fn spawn(data_dir: &Path, env: &[(&str, String)]) -> Server {
        for attempt in 0..8 {
            let port = free_port();
            let stderr_path = data_dir.join(format!("server-{port}-{attempt}.log"));
            let stderr = std::fs::File::create(&stderr_path).expect("server log");
            let mut command = Command::new(env!("CARGO_BIN_EXE_hoglet"));
            command
                .env("HOGLET_DATA", data_dir)
                .env("HOGLET_ADDR", format!("127.0.0.1:{port}"))
                .env("HOGLET_MAX_EVENTS_PER_SEC", "1000000")
                .env("RUST_LOG", "hoglet=warn")
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr));
            for (name, value) in env {
                command.env(name, value);
            }
            let child = command.spawn().expect("spawn hoglet");
            let mut server = Server {
                child,
                port,
                stderr_path,
            };
            match server.wait_until_up(Duration::from_secs(60)) {
                Up::Yes => return server,
                Up::Exited if server.log().contains("in use") => continue,
                Up::Exited => panic!(
                    "hoglet exited during startup ({:?}):\n{}",
                    server.child.try_wait(),
                    server.log()
                ),
                Up::Timeout => {
                    let _ = server.child.kill();
                    panic!("hoglet did not start in time:\n{}", server.log());
                }
            }
        }
        panic!("no free port after 8 attempts");
    }

    /// Spawn expecting the process to refuse to start; returns its log.
    pub fn spawn_expecting_failure(data_dir: &Path) -> String {
        let port = free_port();
        let stderr_path = data_dir.join(format!("server-fail-{port}.log"));
        let stderr = std::fs::File::create(&stderr_path).expect("server log");
        let mut child = Command::new(env!("CARGO_BIN_EXE_hoglet"))
            .env("HOGLET_DATA", data_dir)
            .env("HOGLET_ADDR", format!("127.0.0.1:{port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn hoglet");
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().expect("poll child") {
                assert!(!status.success(), "server started on a corrupt WAL");
                return std::fs::read_to_string(&stderr_path).unwrap_or_default();
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("server accepted connections on a corrupt WAL");
            }
            assert!(Instant::now() < deadline, "server neither started nor failed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn wait_until_up(&mut self, within: Duration) -> Up {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if self.child.try_wait().expect("poll child").is_some() {
                return Up::Exited;
            }
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return Up::Yes;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Up::Timeout
    }

    /// Did the process die from its own injected SIGKILL?
    pub fn killed_by_fault(&mut self) -> Option<String> {
        let status = self.child.try_wait().ok().flatten()?;
        (status.signal() == Some(9))
            .then(|| self.log())
            .filter(|log| log.contains("fault injection: aborting at"))
    }

    pub fn wait_for_death(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if self.child.try_wait().expect("poll child").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    pub fn sigkill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// SIGTERM: graceful shutdown (drain, publish the WAL).
    pub fn stop_gracefully(&mut self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        if !self.wait_for_death(Duration::from_secs(60)) {
            self.sigkill();
            panic!("graceful shutdown timed out:\n{}", self.log());
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum Up {
    Yes,
    Exited,
    Timeout,
}

// ------------------------------------------------------------------- HTTP

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 2xx: the client was told the events are durable.
    Acked,
    /// 5xx: retryable by SDKs.
    Retryable(u16),
    /// 4xx: SDKs never retry; a test that provokes one is wrong.
    Rejected(u16),
    /// Connection refused / reset / timed out.
    Unknown,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
}

pub fn http(
    method: &str,
    port: u16,
    path: &str,
    body: Option<&Value>,
    cookie: Option<&str>,
) -> (Outcome, Value) {
    let url = format!("http://127.0.0.1:{port}{path}");
    let mut request = agent().request(method, &url);
    if let Some(cookie) = cookie {
        request = request.set("Cookie", cookie);
    }
    let result = match body {
        Some(body) => request.send_json(body.clone()),
        None => request.call(),
    };
    match result {
        Ok(response) => {
            let json = response
                .into_string()
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            (Outcome::Acked, json)
        }
        Err(ureq::Error::Status(code, response)) => {
            let json = response
                .into_string()
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            if code >= 500 {
                (Outcome::Retryable(code), json)
            } else {
                (Outcome::Rejected(code), json)
            }
        }
        Err(ureq::Error::Transport(_)) => (Outcome::Unknown, Value::Null),
    }
}

pub struct Project {
    pub id: String,
    pub cookie: String,
}

/// First-run setup through the real HTTP API; adopts `TOKEN` as the project's
/// capture token.
pub fn setup(port: u16) -> Project {
    let url = format!("http://127.0.0.1:{port}/api/auth/setup");
    let response = agent()
        .post(&url)
        .send_json(json!({
            "email": "owner@example.com",
            "password": "correct horse battery staple",
            "organization_name": "Claim 2",
            "project_name": "Durability",
            "existing_project_token": TOKEN
        }))
        .expect("setup");
    let cookie = response
        .header("set-cookie")
        .and_then(|value| value.split(';').next())
        .expect("session cookie")
        .to_owned();
    let body: Value = serde_json::from_str(&response.into_string().expect("body")).expect("json");
    let id = body["organizations"][0]["projects"][0]["id"]
        .as_str()
        .expect("project id")
        .to_owned();
    Project { id, cookie }
}

// ----------------------------------------------------------------- ledger

#[derive(Debug, Clone)]
pub struct Sent {
    pub event: String,
    pub distinct_id: String,
    pub micros: i64,
}

/// What the clients sent and what the server promised.
#[derive(Default)]
pub struct Ledger {
    pub sent: Mutex<HashMap<Uuid, Sent>>,
    pub acked: Mutex<HashSet<Uuid>>,
    pub retryable: AtomicU64,
}

impl Ledger {
    pub fn acked_count(&self) -> usize {
        self.acked.lock().unwrap().len()
    }
}

/// Timestamps are unique per event so the feed's keyset paging is exact.
static CLOCK: AtomicU64 = AtomicU64::new(0);

pub struct EventPlan {
    pub uuid: Uuid,
    pub event: &'static str,
    pub distinct_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub fn plan_event(
    event: &'static str,
    distinct_id: String,
    days_ago: i64,
) -> EventPlan {
    let day = (chrono::Utc::now() - chrono::Duration::days(days_ago)).date_naive();
    let midnight = day.and_hms_opt(1, 0, 0).expect("time").and_utc();
    let tick = CLOCK.fetch_add(1, Ordering::SeqCst) as i64;
    EventPlan {
        uuid: Uuid::now_v7(),
        event,
        distinct_id,
        // Unique microsecond offsets, at most 11 hours into the day.
        timestamp: midnight + chrono::Duration::microseconds(1 + tick % 40_000_000_000),
    }
}

/// POST one batch; every event is recorded as sent first, acked only on 200.
pub fn post_batch(port: u16, ledger: &Ledger, plans: &[EventPlan]) -> Outcome {
    {
        let mut sent = ledger.sent.lock().unwrap();
        for plan in plans {
            sent.insert(
                plan.uuid,
                Sent {
                    event: plan.event.to_owned(),
                    distinct_id: plan.distinct_id.clone(),
                    micros: plan.timestamp.timestamp_micros(),
                },
            );
        }
    }
    let batch: Vec<Value> = plans
        .iter()
        .map(|plan| {
            json!({
                "event": plan.event,
                "distinct_id": plan.distinct_id,
                "uuid": plan.uuid.to_string(),
                "timestamp": plan.timestamp.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                "properties": {"claim2_payload": plan.uuid.simple().to_string()}
            })
        })
        .collect();
    let (outcome, _) = http(
        "POST",
        port,
        "/batch/",
        Some(&json!({"api_key": TOKEN, "batch": batch})),
        None,
    );
    match outcome {
        Outcome::Acked => {
            let mut acked = ledger.acked.lock().unwrap();
            acked.extend(plans.iter().map(|plan| plan.uuid));
        }
        Outcome::Retryable(_) => {
            ledger.retryable.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
    outcome
}

/// Writers hammering the server until `stop` is set or the server stops
/// answering. Returns the join handles.
pub struct Writers {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Writers {
    pub fn start(
        port: u16,
        ledger: &Arc<Ledger>,
        seed: u64,
        count: usize,
        max_batch: usize,
        pace_ms: u64,
        id_prefix: &str,
    ) -> Writers {
        let stop = Arc::new(AtomicBool::new(false));
        let handles = (0..count)
            .map(|writer| {
                let stop = stop.clone();
                let ledger = ledger.clone();
                let prefix = id_prefix.to_owned();
                std::thread::spawn(move || {
                    let mut rng = StdRng::seed_from_u64(seed.wrapping_mul(1_000).wrapping_add(writer as u64));
                    let mut unknown_streak = 0;
                    while !stop.load(Ordering::Relaxed) {
                        let size = rng.gen_range(1..=max_batch);
                        let plans: Vec<EventPlan> = (0..size)
                            .map(|_| {
                                // Mostly past days so partitions are compactable.
                                let days_ago = if rng.gen_bool(0.75) { rng.gen_range(2..=3) } else { 0 };
                                plan_event(
                                    EVENT,
                                    format!("{prefix}w{writer}-{}", rng.gen_range(0..16)),
                                    days_ago,
                                )
                            })
                            .collect();
                        match post_batch(port, &ledger, &plans) {
                            Outcome::Unknown => {
                                unknown_streak += 1;
                                if unknown_streak > 3 {
                                    break;
                                }
                            }
                            Outcome::Rejected(code) => panic!("capture answered {code}; the harness sent a bad batch"),
                            _ => unknown_streak = 0,
                        }
                        if pace_ms > 0 {
                            std::thread::sleep(Duration::from_millis(rng.gen_range(0..=pace_ms)));
                        }
                    }
                })
            })
            .collect();
        Writers { stop, handles }
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles {
            handle.join().expect("writer thread");
        }
    }

    pub fn finished(&self) -> bool {
        self.handles.iter().all(|handle| handle.is_finished())
    }
}

// ------------------------------------------------------------ polling

pub fn wait_until(what: &str, within: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    loop {
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn status_stored_events(port: u16, project: &Project) -> Option<u64> {
    let (outcome, body) = http(
        "GET",
        port,
        &format!("/api/projects/{}/status", project.id),
        None,
        Some(&project.cookie),
    );
    (outcome == Outcome::Acked).then(|| body["stored_events"].as_u64()).flatten()
}

// ------------------------------------------------------------- the lake

#[derive(Debug)]
pub struct CatalogView {
    pub live: Vec<(PathBuf, u64)>,
    pub known_paths: BTreeSet<PathBuf>,
    pub generation: u64,
    pub checkpoint: (u64, u64),
}

fn open_projections(data_dir: &Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_with_flags(
        data_dir.join("projections.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("projections.db");
    connection.busy_timeout(Duration::from_secs(10)).expect("busy");
    connection
}

pub fn catalog(data_dir: &Path) -> CatalogView {
    let connection = open_projections(data_dir);
    let root = std::fs::canonicalize(data_dir.join("events")).unwrap_or_else(|_| data_dir.join("events"));
    let mut live = Vec::new();
    let mut known = BTreeSet::new();
    let mut statement = connection
        .prepare("SELECT relative_path, rows, retired_generation IS NULL FROM lake_files")
        .expect("catalog query");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, bool>(2)?))
        })
        .expect("catalog rows");
    for row in rows {
        let (relative, rows, is_live) = row.expect("row");
        let path = root.join(relative);
        known.insert(path.clone());
        if is_live {
            live.push((path, rows as u64));
        }
    }
    let (generation, segment, offset): (i64, i64, i64) = connection
        .query_row(
            "SELECT generation, wal_segment, wal_offset FROM projection_state",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("projection state");
    CatalogView {
        live,
        known_paths: known,
        generation: generation as u64,
        checkpoint: (segment as u64, offset as u64),
    }
}

/// Parquet and temporary files on disk under `events/`.
pub fn lake_files_on_disk(data_dir: &Path) -> BTreeSet<PathBuf> {
    let root = std::fs::canonicalize(data_dir.join("events")).unwrap_or_else(|_| data_dir.join("events"));
    let mut found = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.insert(path);
            }
        }
    }
    found
}

#[derive(Debug, Clone)]
pub struct StoredRow {
    pub uuid: String,
    pub event: String,
    pub distinct_id: String,
    pub micros: i64,
    pub payload: Option<String>,
}

/// Every row of every live file, read straight from Parquet with DuckDB.
/// A running server may compact (and unlink) files between the catalog read
/// and the Parquet read; retry against a fresh catalog.
pub fn stored_rows(data_dir: &Path) -> Result<Vec<StoredRow>, String> {
    let mut last = String::new();
    for _ in 0..40 {
        match stored_rows_once(data_dir) {
            Ok(rows) => return Ok(rows),
            Err(error) => last = error,
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(last)
}

fn stored_rows_once(data_dir: &Path) -> Result<Vec<StoredRow>, String> {
    let view = catalog(data_dir);
    if view.live.is_empty() {
        return Ok(Vec::new());
    }
    let files = view
        .live
        .iter()
        .map(|(path, _)| format!("'{}'", path.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let duck = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
    let mut statement = duck
        .prepare(&format!(
            "SELECT uuid, event, distinct_id, epoch_us(timestamp)::BIGINT,
                    json_extract_string(properties, '$.claim2_payload')
             FROM read_parquet([{files}], union_by_name = true)"
        ))
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(StoredRow {
                uuid: row.get(0)?,
                event: row.get(1)?,
                distinct_id: row.get(2)?,
                micros: row.get(3)?,
                payload: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

pub fn parquet_row_count(path: &Path) -> Result<u64, String> {
    let duck = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
    duck.query_row(
        &format!("SELECT count(*) FROM read_parquet('{}')", path.display()),
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count as u64)
    .map_err(|e| e.to_string())
}

/// Distinct ids with a person, and the person count.
pub fn identity_view(data_dir: &Path) -> (BTreeSet<String>, u64, u64) {
    let connection = open_projections(data_dir);
    let mut statement = connection
        .prepare("SELECT distinct_id FROM distinct_ids")
        .expect("distinct ids");
    let ids: BTreeSet<String> = statement
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("ids");
    let persons: i64 = connection
        .query_row("SELECT count(*) FROM persons", [], |row| row.get(0))
        .expect("persons");
    let orphan_persons: i64 = connection
        .query_row(
            "SELECT count(*) FROM persons p WHERE NOT EXISTS
               (SELECT 1 FROM distinct_ids d WHERE d.project_id = p.project_id AND d.person_id = p.id)",
            [],
            |row| row.get(0),
        )
        .expect("orphans");
    (ids, persons as u64, orphan_persons as u64)
}

// -------------------------------------------------------------- verdicts

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub acked: usize,
    pub sent: usize,
    pub stored: usize,
    /// Durable but never acknowledged: allowed to exist, never required.
    pub unacked_but_stored: usize,
    pub lake_files: usize,
    pub aborted_at: String,
}

/// Reconcile the clients' ledger with the stored events. `erased` is the set
/// of distinct ids whose events must be gone.
pub fn reconcile(
    data_dir: &Path,
    ledger: &Ledger,
    erased: &BTreeSet<String>,
) -> Result<Report, String> {
    let rows = stored_rows(data_dir)?;
    let sent = ledger.sent.lock().unwrap();
    let acked = ledger.acked.lock().unwrap();
    let mut seen: HashMap<&str, &StoredRow> = HashMap::new();
    for row in &rows {
        if seen.insert(row.uuid.as_str(), row).is_some() {
            return Err(format!("event {} is stored twice", row.uuid));
        }
        let uuid: Uuid = row.uuid.parse().map_err(|_| format!("stored garbage uuid {}", row.uuid))?;
        let Some(expected) = sent.get(&uuid) else {
            return Err(format!("event {uuid} is stored but was never sent (phantom data)"));
        };
        if expected.event != row.event
            || expected.distinct_id != row.distinct_id
            || expected.micros != row.micros
            || row.payload.as_deref() != Some(uuid.simple().to_string().as_str())
        {
            return Err(format!("event {uuid} is stored altered: sent {expected:?}, stored {row:?}"));
        }
        if erased.contains(&row.distinct_id) {
            return Err(format!("erased person's event {uuid} (distinct id {}) survived", row.distinct_id));
        }
    }
    let lost: Vec<&Uuid> = acked
        .iter()
        .filter(|uuid| {
            let id = &sent[*uuid].distinct_id;
            !erased.contains(id) && !seen.contains_key(uuid.to_string().as_str())
        })
        .collect();
    if !lost.is_empty() {
        return Err(format!(
            "{} of {} acknowledged events were lost, e.g. {}",
            lost.len(),
            acked.len(),
            lost[0]
        ));
    }
    let unacked_but_stored = rows
        .iter()
        .filter(|row| row.uuid.parse::<Uuid>().is_ok_and(|uuid| !acked.contains(&uuid)))
        .count();
    Ok(Report {
        acked: acked.len(),
        sent: sent.len(),
        stored: rows.len(),
        unacked_but_stored,
        lake_files: catalog(data_dir).live.len(),
        aborted_at: String::new(),
    })
}

/// No orphan files, no missing files, row counts match, identity matches the
/// events. Polled: a live server may still be sweeping retired files.
pub fn check_consistent(data_dir: &Path, erased: &BTreeSet<String>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match consistency_problem(data_dir, erased) {
            None => return Ok(()),
            Some(problem) if Instant::now() >= deadline => return Err(problem),
            Some(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn consistency_problem(data_dir: &Path, erased: &BTreeSet<String>) -> Option<String> {
    let view = catalog(data_dir);
    let on_disk = lake_files_on_disk(data_dir);
    if let Some(orphan) = on_disk.difference(&view.known_paths).next() {
        return Some(format!("orphan file left on disk: {}", orphan.display()));
    }
    for (path, rows) in &view.live {
        if !path.is_file() {
            return Some(format!("catalogued file is missing: {}", path.display()));
        }
        match parquet_row_count(path) {
            Ok(actual) if actual == *rows => {}
            Ok(actual) => {
                return Some(format!("catalog says {rows} rows, parquet has {actual}: {}", path.display()));
            }
            Err(error) => return Some(format!("unreadable parquet {}: {error}", path.display())),
        }
    }
    let rows = match stored_rows(data_dir) {
        Ok(rows) => rows,
        Err(error) => return Some(format!("cannot read lake: {error}")),
    };
    let (ids, persons, orphan_persons) = identity_view(data_dir);
    let event_ids: BTreeSet<String> = rows.iter().map(|row| row.distinct_id.clone()).collect();
    if let Some(missing) = event_ids.difference(&ids).next() {
        return Some(format!("stored events for {missing} have no person (identity and lake diverged)"));
    }
    if let Some(extra) = ids.iter().find(|id| erased.contains(*id)) {
        return Some(format!("erased distinct id {extra} still has a person"));
    }
    if orphan_persons != 0 {
        return Some(format!("{orphan_persons} persons have no distinct id"));
    }
    if persons > ids.len() as u64 {
        return Some(format!("{persons} persons for {} distinct ids", ids.len()));
    }
    None
}

/// All event uuids visible through `GET /events`, paged to exhaustion.
pub fn feed_uuids(port: u16, project: &Project, event: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut before: Option<String> = None;
    for _ in 0..10_000 {
        let mut path = format!("/api/projects/{}/events?event={event}&limit=200", project.id);
        if let Some(before) = &before {
            path.push_str(&format!("&before={}", before.replace('+', "%2B").replace(':', "%3A")));
        }
        let (outcome, body) = http("GET", port, &path, None, Some(&project.cookie));
        assert_eq!(outcome, Outcome::Acked, "events feed failed: {body}");
        for row in body["events"].as_array().expect("events array") {
            found.insert(row["uuid"].as_str().expect("uuid").to_owned());
        }
        match body["next_before"].as_str() {
            Some(next) => before = Some(next.to_owned()),
            None => break,
        }
    }
    found
}

/// Wait until every acknowledged (non-erased) event is queryable.
pub fn wait_until_queryable(port: u16, project: &Project, ledger: &Ledger, erased: &BTreeSet<String>) {
    let want: BTreeSet<String> = {
        let sent = ledger.sent.lock().unwrap();
        ledger
            .acked
            .lock()
            .unwrap()
            .iter()
            .filter(|uuid| !erased.contains(&sent[*uuid].distinct_id))
            .map(Uuid::to_string)
            .collect()
    };
    wait_until("acknowledged events to be queryable through the API", Duration::from_secs(60), || {
        feed_uuids(port, project, EVENT).is_superset(&want)
    });
}

pub fn count_by_event(rows: &[StoredRow]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(row.event.clone()).or_insert(0) += 1;
    }
    counts
}
