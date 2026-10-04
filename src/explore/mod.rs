//! The explore read path: persons, the live event feed, web analytics.
//!
//! [`Explorer`] owns one in-memory DuckDB database with a small bounded pool
//! of connections, an admission gate (bounded concurrency, bounded queue, a
//! deadline that interrupts DuckDB), and an incrementally synced copy of the
//! identity overrides (`distinct_id -> person_id` rows where they differ),
//! which web analytics joins to count persons rather than devices.
//!
//! Event files always come from the [`EventSource`]; the returned
//! [`EventFiles`] guard is held for the whole read.

pub mod dates;
pub mod events;
pub mod filters;
pub mod http;
pub mod persons;
pub mod web;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use tokio::sync::Semaphore;

use crate::persons::{PersonStore, PersonStoreError};
use crate::source::{EventFiles, EventSource};

/// DuckDB memory ceiling for the explore database.
pub const MEMORY_LIMIT: &str = "256MB";
/// Disk DuckDB may spill to when a query exceeds [`MEMORY_LIMIT`].
pub const MAX_SPILL: &str = "4GB";
/// DuckDB worker threads.
pub const MAX_THREADS: usize = 2;
/// Explore queries executing at once.
pub const MAX_CONCURRENT: usize = 2;
/// Explore queries waiting for a slot; beyond this callers get `Busy`.
pub const MAX_QUEUED: usize = 16;
/// Deadline after which a query is interrupted.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
/// Most Parquet files one read may name.
pub const MAX_FILES_PER_READ: usize = 50_000;

#[derive(Debug)]
pub enum ExploreError {
    /// The request is malformed: 400.
    Invalid {
        field: &'static str,
        message: &'static str,
    },
    NotFound,
    /// Admission queue full: 503 + Retry-After.
    Busy,
    /// Deadline exceeded: 504.
    Timeout,
    Database(String),
    Persons(PersonStoreError),
    Internal(String),
}

impl std::fmt::Display for ExploreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { field, message } => write!(formatter, "{field}: {message}"),
            Self::NotFound => formatter.write_str("not found"),
            Self::Busy => formatter.write_str("explore queue full"),
            Self::Timeout => formatter.write_str("explore query timed out"),
            Self::Database(error) => write!(formatter, "explore database error: {error}"),
            Self::Persons(error) => write!(formatter, "{error}"),
            Self::Internal(error) => write!(formatter, "explore internal error: {error}"),
        }
    }
}

impl std::error::Error for ExploreError {}

impl From<duckdb::Error> for ExploreError {
    fn from(error: duckdb::Error) -> Self {
        Self::Database(error.to_string())
    }
}

impl From<PersonStoreError> for ExploreError {
    fn from(error: PersonStoreError) -> Self {
        Self::Persons(error)
    }
}

#[derive(Debug, Default)]
struct OverrideCursor {
    epoch: Option<u64>,
    seq: u64,
}

pub struct Explorer {
    source: Arc<dyn EventSource>,
    persons: Arc<PersonStore>,
    /// Owns the database; pooled connections are clones of it.
    root: Mutex<duckdb::Connection>,
    pool: Mutex<Vec<duckdb::Connection>>,
    overrides: Mutex<OverrideCursor>,
    queue: Arc<Semaphore>,
    running: Arc<Semaphore>,
}

impl Explorer {
    pub fn new(
        source: Arc<dyn EventSource>,
        persons: Arc<PersonStore>,
    ) -> Result<Self, ExploreError> {
        // A static binary cannot load extensions; everything needed is
        // compiled in, so never try to fetch or load one.
        let root = duckdb::Connection::open_in_memory_with_flags(
            duckdb::Config::default().enable_autoload_extension(false)?,
        )?;
        root.execute_batch("SET autoinstall_known_extensions = false;")?;
        // `TimeZone` needs the ICU extension; without it TIMESTAMPTZ is UTC.
        let _ = root.execute_batch("SET GLOBAL TimeZone='UTC';");
        // Large web-analytics ranges may exceed the memory limit; they spill
        // to a bounded scratch directory instead of failing.
        let spill = std::env::temp_dir().join(format!("hoglet-explore-{}", std::process::id()));
        let spill = spill
            .to_string_lossy()
            .replace('\'', "''")
            .replace('\0', "");
        root.execute_batch(&format!(
            "SET temp_directory='{spill}';
             SET max_temp_directory_size='{MAX_SPILL}';
             SET memory_limit='{MEMORY_LIMIT}';
             SET threads={MAX_THREADS};
             SET enable_progress_bar=false;
             CREATE TABLE explore_overrides (
                 project_id VARCHAR NOT NULL,
                 distinct_id VARCHAR NOT NULL,
                 person_id VARCHAR NOT NULL
             );
             CREATE TABLE explore_override_changes (
                 project_id VARCHAR NOT NULL,
                 distinct_id VARCHAR NOT NULL,
                 person_id VARCHAR NOT NULL
             );"
        ))?;
        Ok(Self {
            source,
            persons,
            root: Mutex::new(root),
            pool: Mutex::new(Vec::new()),
            overrides: Mutex::new(OverrideCursor::default()),
            queue: Arc::new(Semaphore::new(MAX_CONCURRENT + MAX_QUEUED)),
            running: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        })
    }

    pub fn source(&self) -> &dyn EventSource {
        self.source.as_ref()
    }

    pub fn persons(&self) -> &PersonStore {
        &self.persons
    }

    /// Run `work` on a pooled connection, synchronously. Callers on the async
    /// runtime use [`Explorer::run`].
    pub fn with_connection<T>(
        &self,
        work: impl FnOnce(&duckdb::Connection) -> Result<T, ExploreError>,
    ) -> Result<T, ExploreError> {
        let pooled = self
            .pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let connection = match pooled {
            Some(connection) => connection,
            None => self
                .root
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_clone()?,
        };
        let result = work(&connection);
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.len() < MAX_CONCURRENT + 1 {
            pool.push(connection);
        }
        result
    }

    /// Admit, then run `work` on a blocking thread under [`QUERY_TIMEOUT`].
    pub async fn run<T, F>(self: &Arc<Self>, work: F) -> Result<T, ExploreError>
    where
        T: Send + 'static,
        F: FnOnce(&Explorer, &duckdb::Connection) -> Result<T, ExploreError> + Send + 'static,
    {
        let queued = self
            .queue
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExploreError::Busy)?;
        let running = self
            .running
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ExploreError::Busy)?;
        let interrupt: Arc<Mutex<Option<Arc<duckdb::InterruptHandle>>>> =
            Arc::new(Mutex::new(None));
        let slot = interrupt.clone();
        let explorer = self.clone();
        let mut task = tokio::task::spawn_blocking(move || {
            // Permits are released only when DuckDB actually returns, so a
            // timed-out query still occupies its slot until it stops.
            let _permits = (queued, running);
            explorer.with_connection(|connection| {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(connection.interrupt_handle());
                let result = work(&explorer, connection);
                slot.lock().unwrap_or_else(PoisonError::into_inner).take();
                result
            })
        });
        match tokio::time::timeout(QUERY_TIMEOUT, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(ExploreError::Internal(error.to_string())),
            Err(_) => {
                if let Some(handle) = interrupt
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    handle.interrupt();
                }
                Err(ExploreError::Timeout)
            }
        }
    }

    /// Bring `explore_overrides` up to date with the identity projection.
    pub fn sync_overrides(&self, connection: &duckdb::Connection) -> Result<(), ExploreError> {
        let mut cursor = self
            .overrides
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Each batch is bounded; the loop ends when the feed is drained. A
        // generous iteration bound guards against a misbehaving feed.
        for _ in 0..10_000 {
            let after = if cursor.epoch.is_some() {
                cursor.seq
            } else {
                0
            };
            let batch = self.persons.overrides_since(after)?;
            if cursor.epoch != Some(batch.epoch) {
                connection.execute_batch("DELETE FROM explore_overrides")?;
                let reload = cursor.epoch.is_some() && after > 0;
                cursor.epoch = Some(batch.epoch);
                cursor.seq = 0;
                if reload {
                    continue;
                }
            }
            if !batch.changes.is_empty() {
                apply_override_changes(connection, &batch.changes)?;
            }
            cursor.seq = batch.max_seq;
            if !batch.truncated {
                return Ok(());
            }
        }
        Err(ExploreError::Internal(
            "identity override sync did not converge".to_owned(),
        ))
    }
}

fn apply_override_changes(
    connection: &duckdb::Connection,
    changes: &[crate::persons::OverrideChange],
) -> Result<(), ExploreError> {
    connection.execute_batch("BEGIN TRANSACTION; DELETE FROM explore_override_changes;")?;
    let result = (|| {
        {
            let mut appender = connection.appender("explore_override_changes")?;
            for change in changes {
                appender.append_row([
                    change.project_id.as_str(),
                    change.distinct_id.as_str(),
                    change.person_id.as_str(),
                ])?;
            }
            appender.flush()?;
        }
        connection.execute_batch(
            "DELETE FROM explore_overrides o USING explore_override_changes c
             WHERE o.project_id = c.project_id AND o.distinct_id = c.distinct_id;
             INSERT INTO explore_overrides
             SELECT project_id, distinct_id, person_id FROM explore_override_changes
             WHERE person_id <> distinct_id;
             DELETE FROM explore_override_changes;",
        )
    })();
    match result {
        Ok(()) => {
            connection.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error.into())
        }
    }
}

/// `read_parquet([...], union_by_name=true)` over `files`, or `None` when
/// there are none. Paths come from the event source, never from requests.
pub fn parquet_source(files: &EventFiles) -> Result<Option<String>, ExploreError> {
    if files.is_empty() {
        return Ok(None);
    }
    if files.paths.len() > MAX_FILES_PER_READ {
        return Err(ExploreError::Internal(format!(
            "read names {} files, above the {MAX_FILES_PER_READ} limit",
            files.paths.len()
        )));
    }
    let mut quoted = Vec::with_capacity(files.paths.len());
    for path in &files.paths {
        let text = path
            .to_str()
            .ok_or_else(|| ExploreError::Internal("event file path is not UTF-8".to_owned()))?;
        if text.contains('\0') {
            return Err(ExploreError::Internal(
                "event file path contains NUL".to_owned(),
            ));
        }
        quoted.push(format!("'{}'", text.replace('\'', "''")));
    }
    Ok(Some(format!(
        "read_parquet([{}], union_by_name=true)",
        quoted.join(", ")
    )))
}

/// RFC 3339 with microseconds, `Z` suffix: the API's timestamp form.
pub fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Microseconds since the epoch back to a UTC instant.
pub fn from_micros(micros: i64) -> DateTime<Utc> {
    Utc.timestamp_micros(micros)
        .single()
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}
