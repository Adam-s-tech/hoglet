//! Shadow mode: forward a project's captured events to PostHog.
//!
//! A team switching from PostHog points its SDKs at Hoglet and turns on
//! forwarding; PostHog keeps receiving the same events, so both systems can be
//! compared on identical data before the cut-over (`hoglet reconcile`).
//!
//! Forwarding happens strictly after Hoglet's durable acknowledgement, through
//! a bounded queue drained by one background thread. A full queue drops and
//! counts — forwarding may lose events under overload; Hoglet never does, and
//! forwarding never delays a capture response.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capture::event::CapturedEvent;

/// Events waiting to be forwarded, across all projects.
pub const QUEUE_EVENTS: usize = 100_000;
/// Events per forwarded `/batch/` request.
pub const BATCH_EVENTS: usize = 500;
const MAX_ATTEMPTS: u32 = 5;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS project_forwarding (
    project_id TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    host TEXT NOT NULL,
    posthog_token TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);";

/// Per-project forwarding settings, as stored and as edited in the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardingConfig {
    pub enabled: bool,
    /// PostHog ingestion host, e.g. `https://us.i.posthog.com`.
    pub host: String,
    /// Project API key of the PostHog project to forward to.
    pub posthog_token: String,
}

/// What the dashboard shows about forwarding.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ForwardingStatus {
    pub config: Option<ForwardingConfig>,
    pub forwarded: u64,
    pub dropped: u64,
    pub failed: u64,
    pub queued: usize,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct Counters {
    forwarded: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
}

struct Queued {
    project_id: String,
    events: Vec<CapturedEvent>,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Queued>,
    events: usize,
    stopped: bool,
}

pub struct Forwarder {
    connection: Mutex<Connection>,
    configs: Mutex<HashMap<String, ForwardingConfig>>,
    queue: Mutex<Queue>,
    ready: Condvar,
    project_counters: Mutex<HashMap<String, Arc<Counters>>>,
    last_error: Mutex<HashMap<String, String>>,
}

impl Forwarder {
    pub fn open(control_db: &Path) -> Result<Arc<Self>, rusqlite::Error> {
        let connection = Connection::open_with_flags(
            control_db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(SCHEMA)?;
        let mut configs = HashMap::new();
        {
            let mut statement = connection
                .prepare("SELECT project_id, enabled, host, posthog_token FROM project_forwarding")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ForwardingConfig {
                        enabled: row.get(1)?,
                        host: row.get(2)?,
                        posthog_token: row.get(3)?,
                    },
                ))
            })?;
            for row in rows {
                let (project_id, config) = row?;
                configs.insert(project_id, config);
            }
        }
        let forwarder = Arc::new(Self {
            connection: Mutex::new(connection),
            configs: Mutex::new(configs),
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
            project_counters: Mutex::new(HashMap::new()),
            last_error: Mutex::new(HashMap::new()),
        });
        let worker = forwarder.clone();
        std::thread::Builder::new()
            .name("hoglet-forwarder".to_owned())
            .spawn(move || worker.run())
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(forwarder)
    }

    /// Offer acknowledged events of one project. Never blocks.
    pub fn offer(&self, project_id: &str, events: &[CapturedEvent]) {
        let enabled = self
            .configs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .is_some_and(|config| config.enabled);
        if !enabled || events.is_empty() {
            return;
        }
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.events + events.len() > QUEUE_EVENTS {
            drop(queue);
            self.counters_for(project_id)
                .dropped
                .fetch_add(events.len() as u64, Ordering::Relaxed);
            return;
        }
        queue.events += events.len();
        queue.items.push_back(Queued {
            project_id: project_id.to_owned(),
            events: events.to_vec(),
        });
        drop(queue);
        self.ready.notify_one();
    }

    pub fn config(&self, project_id: &str) -> Option<ForwardingConfig> {
        self.configs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .cloned()
    }

    pub fn set_config(
        &self,
        project_id: &str,
        config: ForwardingConfig,
    ) -> Result<(), rusqlite::Error> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .execute(
                "INSERT INTO project_forwarding (project_id, enabled, host, posthog_token, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(project_id) DO UPDATE SET
                    enabled = excluded.enabled, host = excluded.host,
                    posthog_token = excluded.posthog_token, updated_at = excluded.updated_at",
                params![
                    project_id,
                    config.enabled,
                    config.host,
                    config.posthog_token,
                    chrono::Utc::now().timestamp()
                ],
            )?;
        self.configs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(project_id.to_owned(), config);
        Ok(())
    }

    pub fn status(&self, project_id: &str) -> ForwardingStatus {
        let counters = self.counters_for(project_id);
        let queued = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .iter()
            .filter(|item| item.project_id == project_id)
            .map(|item| item.events.len())
            .sum();
        ForwardingStatus {
            config: self.config(project_id),
            forwarded: counters.forwarded.load(Ordering::Relaxed),
            dropped: counters.dropped.load(Ordering::Relaxed),
            failed: counters.failed.load(Ordering::Relaxed),
            queued,
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(project_id)
                .cloned(),
        }
    }

    pub fn stop(&self) {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopped = true;
        self.ready.notify_all();
    }

    fn counters_for(&self, project_id: &str) -> Arc<Counters> {
        self.project_counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(project_id.to_owned())
            .or_default()
            .clone()
    }

    fn run(&self) {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("hoglet-forward/", env!("CARGO_PKG_VERSION")))
            .build();
        loop {
            let next = {
                let mut queue = self
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while queue.items.is_empty() && !queue.stopped {
                    queue = self
                        .ready
                        .wait(queue)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                if queue.items.is_empty() {
                    return;
                }
                let item = queue.items.pop_front().expect("checked non-empty");
                queue.events -= item.events.len();
                item
            };
            let Some(config) = self.config(&next.project_id).filter(|config| config.enabled) else {
                continue;
            };
            for chunk in next.events.chunks(BATCH_EVENTS) {
                let counters = self.counters_for(&next.project_id);
                match send(&agent, &config, chunk) {
                    Ok(()) => {
                        counters
                            .forwarded
                            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    }
                    Err(error) => {
                        counters
                            .failed
                            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
                        tracing::warn!(project = %next.project_id, %error, "forwarding to PostHog failed");
                        self.last_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(next.project_id.clone(), error);
                    }
                }
            }
        }
    }
}

fn send(agent: &ureq::Agent, config: &ForwardingConfig, events: &[CapturedEvent]) -> Result<(), String> {
    let url = format!("{}/batch/", config.host.trim_end_matches('/'));
    let batch: Vec<_> = events
        .iter()
        .map(|event| {
            json!({
                "uuid": event.uuid,
                "event": event.event,
                "distinct_id": event.distinct_id,
                "timestamp": event.timestamp.to_rfc3339(),
                "properties": event.properties,
            })
        })
        .collect();
    let body = json!({"api_key": config.posthog_token, "batch": batch});
    let mut delay = Duration::from_millis(250);
    for attempt in 1..=MAX_ATTEMPTS {
        match agent.post(&url).send_json(body.clone()) {
            Ok(_) => return Ok(()),
            Err(ureq::Error::Status(code, _)) if code < 500 && code != 429 => {
                return Err(format!("PostHog answered HTTP {code}"));
            }
            Err(error) if attempt == MAX_ATTEMPTS => return Err(error.to_string()),
            Err(_) => {
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
    Err("gave up".to_owned())
}

/// Look up one project's settings without the forwarder (CLI tools).
pub fn read_config(control_db: &Path, project_id: &str) -> Option<ForwardingConfig> {
    let connection = Connection::open_with_flags(control_db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    connection
        .query_row(
            "SELECT enabled, host, posthog_token FROM project_forwarding WHERE project_id = ?1",
            [project_id],
            |row| {
                Ok(ForwardingConfig {
                    enabled: row.get(0)?,
                    host: row.get(1)?,
                    posthog_token: row.get(2)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
}
