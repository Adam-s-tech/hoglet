//! The analytics query engine: DuckDB over the project's Parquet files,
//! persons resolved through identity overrides.
//!
//! Shape of one query:
//!
//! 1. validate the [`InsightQuery`] against explicit limits (400 otherwise);
//! 2. admission: a bounded wait queue in front of a fixed connection pool
//!    (503 `query_busy` when full);
//! 3. sync identity overrides from [`PersonStore`] into the shared
//!    `person_overrides` table (incremental by seq, full reload on epoch
//!    change);
//! 4. resolve the date range, lease exactly the files it needs from the
//!    [`EventSource`] (never a glob);
//! 5. answer from the result cache keyed by (project, canonical query, data
//!    version, identity seq), or run the kind with a watchdog that interrupts
//!    DuckDB at the deadline.
//!
//! Every person count and every per-actor computation uses
//! `person_id = coalesce(override.person_id, distinct_id)`.

mod funnels;
mod identity;
mod lifecycle;
mod paths;
pub mod range;
mod retention;
pub mod sql;
mod sql_query;
mod stickiness;
mod trends;
mod validate;

pub mod formula;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
pub(crate) mod oracle;
#[cfg(test)]
mod proptests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use duckdb::Connection;
use duckdb::arrow::array::{ArrayRef, AsArray, Int64Array, StringArray};
use duckdb::arrow::datatypes::DataType;
use duckdb::arrow::record_batch::RecordBatch;
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::cache::{CacheKey, ResultCache};
use crate::contract::common::{PropertyFilter, PropertySource};
use crate::contract::insight::{
    ActorSelection, ActorsRequest, ActorsResponse, InsightQuery, InsightResult, QueryMeta,
    QueryRequest, QueryResponse,
};
use crate::persons::{PersonStore, PersonStoreError};
use crate::source::{EventFiles, EventSource};

use range::ResolvedRange;
use sql::{EventSchema, Params, Source};

/// Explicit bound on rows any internal result set may return.
pub const MAX_INTERNAL_ROWS: usize = 5_000_000;
/// Explicit bound on rows one streamed statement may deliver. Streamed rows
/// are consumed batch by batch (nothing accumulates here), so the bound only
/// stops runaway scans; consumers bound their own state.
pub const MAX_STREAMED_ROWS: usize = 100_000_000;
/// Explicit bound on actors per page.
pub const MAX_ACTOR_PAGE: u32 = 1_000;
/// Explicit bound on actor offsets.
pub const MAX_ACTOR_OFFSET: u32 = 1_000_000;
/// Cached results that depend on person properties expire: `$set` does not
/// move the identity seq.
const PERSON_PROPERTY_CACHE_TTL: Duration = Duration::from_secs(30);
const MAX_SCHEMA_CACHE: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum QueryError {
    /// 400 `invalid_query`: the request cannot be executed as written.
    Invalid(String),
    /// 400 `query_too_large`: a bound was hit; narrow the query.
    TooLarge(String),
    /// 503 `query_busy`: the bounded queue is full.
    Busy,
    /// 504 `query_timeout`: the deadline interrupted execution.
    Timeout,
    /// 500 `internal_error`.
    Internal(String),
}

impl QueryError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::TooLarge(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) | Self::TooLarge(_) => 400,
            Self::Busy => 503,
            Self::Timeout => 504,
            Self::Internal(_) => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_query",
            Self::TooLarge(_) => "query_too_large",
            Self::Busy => "query_busy",
            Self::Timeout => "query_timeout",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Message safe to show a client.
    pub fn public_message(&self) -> String {
        match self {
            Self::Invalid(message) | Self::TooLarge(message) => message.clone(),
            Self::Busy => "The query queue is full; retry shortly.".to_owned(),
            Self::Timeout => "The query exceeded its execution deadline.".to_owned(),
            Self::Internal(_) => "The query could not be completed.".to_owned(),
        }
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Internal(message) => write!(formatter, "internal query error: {message}"),
            other => formatter.write_str(&other.public_message()),
        }
    }
}

impl std::error::Error for QueryError {}

impl From<duckdb::Error> for QueryError {
    fn from(error: duckdb::Error) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<PersonStoreError> for QueryError {
    fn from(error: PersonStoreError) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<duckdb::arrow::error::ArrowError> for QueryError {
    fn from(error: duckdb::arrow::error::ArrowError) -> Self {
        Self::Internal(error.to_string())
    }
}

/// Resource limits. Defaults fit a small VPS.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// DuckDB memory limit shared by all pooled connections.
    pub memory_limit_mb: u32,
    pub threads: u32,
    /// Pooled connections = maximum concurrently executing queries.
    pub connections: usize,
    /// Requests allowed to wait for a connection; more get 503.
    pub max_queued: usize,
    /// Longest a request waits for a connection before 503.
    pub queue_wait: Duration,
    /// Per-query execution deadline.
    pub timeout: Duration,
    /// DuckDB spill directory.
    pub temp_directory: Option<PathBuf>,
    /// Memory limit for one sandboxed SQL query.
    pub sql_memory_limit_mb: u32,
    pub cache_entries: usize,
    /// Rows one ordered per-person partition aims for (funnels, paths,
    /// lifecycle, retention); bounds materialized intermediate results.
    pub partition_rows: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            memory_limit_mb: 512,
            threads: 2,
            connections: 2,
            max_queued: 16,
            queue_wait: Duration::from_secs(10),
            timeout: Duration::from_secs(30),
            temp_directory: None,
            sql_memory_limit_mb: 256,
            cache_entries: 256,
            partition_rows: PARTITION_ROWS,
        }
    }
}

/// A queue slot plus an execution permit. Hold it while the query runs.
pub struct Admission {
    _queue: OwnedSemaphorePermit,
    _execution: OwnedSemaphorePermit,
}

struct Pool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

struct PooledConnection<'a> {
    pool: &'a Pool,
    connection: Option<Connection>,
}

impl std::ops::Deref for PooledConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        // Present from construction until drop.
        self.connection.as_ref().expect("pooled connection present")
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.pool
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(connection);
            self.pool.available.notify_one();
        }
    }
}

/// Interrupts the connection's running statement at the deadline.
struct Watchdog {
    stop: Option<mpsc::Sender<()>>,
    fired: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn start(handle: Arc<duckdb::InterruptHandle>, timeout: Duration) -> Self {
        let (stop, wait) = mpsc::channel::<()>();
        let fired = Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        let thread = std::thread::Builder::new()
            .name("query-watchdog".into())
            .spawn(move || {
                if let Err(mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(timeout) {
                    flag.store(true, Ordering::SeqCst);
                    handle.interrupt();
                }
            })
            .ok();
        Self {
            stop: Some(stop),
            fired,
            thread,
        }
    }

    fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Everything a kind needs to run against one project's files.
pub(crate) struct Ctx<'a> {
    pub conn: &'a Connection,
    pub source: Source<'a>,
    pub project_id: &'a str,
    pub deadline: Instant,
    pub sql_memory_limit_mb: u32,
    pub threads: u32,
    pub partition_rows: u64,
    /// Absolute spill directory (`<data dir>/tmp/query`) for the SQL sandbox.
    pub temp_directory: Option<&'a std::path::Path>,
}

impl Ctx<'_> {
    pub fn check_deadline(&self) -> Result<(), QueryError> {
        if Instant::now() >= self.deadline {
            Err(QueryError::Timeout)
        } else {
            Ok(())
        }
    }

    /// Run a statement and map every row, bounded by [`MAX_INTERNAL_ROWS`].
    pub fn rows<T>(
        &self,
        sql: &str,
        params: &Params,
        mut map: impl FnMut(&duckdb::Row<'_>) -> duckdb::Result<T>,
    ) -> Result<Vec<T>, QueryError> {
        #[cfg(test)]
        self.profile(sql, params);
        let mut statement = self.conn.prepare(sql)?;
        let mut rows = statement.query(duckdb::params_from_iter(params.values()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            if out.len() == MAX_INTERNAL_ROWS {
                return Err(QueryError::too_large(
                    "the query produced too many intermediate rows; narrow the date range or filters",
                ));
            }
            out.push(map(row)?);
        }
        Ok(out)
    }

    /// Test hook: with `HOGLET_QUERY_PROFILE` set, print the statement and
    /// DuckDB's `EXPLAIN ANALYZE` of it (runs it once more, so only for
    /// profiling sessions).
    #[cfg(test)]
    fn profile(&self, sql: &str, params: &Params) {
        if std::env::var_os("HOGLET_QUERY_PROFILE").is_none() {
            return;
        }
        println!("---- SQL ----\n{sql}");
        let plan = (|| -> duckdb::Result<String> {
            let mut statement = self.conn.prepare(&format!("EXPLAIN ANALYZE {sql}"))?;
            let mut rows = statement.query(duckdb::params_from_iter(params.values()))?;
            let mut out = String::new();
            while let Some(row) = rows.next()? {
                out.push_str(&row.get::<_, String>(1)?);
            }
            Ok(out)
        })();
        println!("---- PLAN ----\n{}", plan.unwrap_or_else(|e| e.to_string()));
    }

    /// Stream a statement's result as Arrow batches.
    pub fn arrow(
        &self,
        sql: &str,
        params: &Params,
        mut each: impl FnMut(&RecordBatch) -> Result<(), QueryError>,
    ) -> Result<(), QueryError> {
        #[cfg(test)]
        self.profile(sql, params);
        let mut statement = self.conn.prepare(sql)?;
        let batches = statement.query_arrow(duckdb::params_from_iter(params.values()))?;
        let mut rows = 0_usize;
        for batch in batches {
            rows += batch.num_rows();
            if rows > MAX_STREAMED_ROWS {
                return Err(QueryError::too_large(
                    "the query produced too many intermediate rows; narrow the date range or filters",
                ));
            }
            self.check_deadline()?;
            each(&batch)?;
        }
        Ok(())
    }

    /// Like [`Ctx::arrow`], but DuckDB streams the result instead of
    /// materializing it first: a large ordered result never sits in memory
    /// next to the consumer's own state. `types` are the result columns'
    /// Arrow types (`Utf8` for VARCHAR, `Int64` for BIGINT).
    pub fn arrow_streaming(
        &self,
        sql: &str,
        params: &Params,
        types: &[DataType],
        mut each: impl FnMut(&RecordBatch) -> Result<(), QueryError>,
    ) -> Result<(), QueryError> {
        #[cfg(test)]
        self.profile(sql, params);
        let schema = Arc::new(duckdb::arrow::datatypes::Schema::new(
            types
                .iter()
                .enumerate()
                .map(|(index, kind)| {
                    duckdb::arrow::datatypes::Field::new(format!("c{index}"), kind.clone(), true)
                })
                .collect::<Vec<_>>(),
        ));
        let mut statement = self.conn.prepare(sql)?;
        let batches = statement.stream_arrow(duckdb::params_from_iter(params.values()), schema)?;
        let mut rows = 0_usize;
        for batch in batches {
            rows += batch.num_rows();
            if rows > MAX_STREAMED_ROWS {
                return Err(QueryError::too_large(
                    "the query produced too many intermediate rows; narrow the date range or filters",
                ));
            }
            self.check_deadline()?;
            each(&batch)?;
        }
        Ok(())
    }

    /// How many person-hash partitions per-person scans use so one ordered,
    /// materialized partition stays near [`PARTITION_ROWS`] rows (from the
    /// files' row counts — Parquet metadata, no scan).
    pub fn person_partitions(&self) -> Result<u64, QueryError> {
        if self.source.files.is_empty() {
            return Ok(1);
        }
        let mut list = Vec::with_capacity(self.source.files.len());
        for path in self.source.files {
            list.push(sql::string_literal(&path.to_string_lossy())?);
        }
        let rows: i64 = self.conn.query_row(
            &format!(
                "SELECT count(*) FROM read_parquet([{}], union_by_name = true)",
                list.join(", ")
            ),
            [],
            |row| row.get(0),
        )?;
        Ok((rows.max(0) as u64)
            .div_ceil(self.partition_rows)
            .clamp(1, MAX_PARTITIONS))
    }

    /// Replace a connection-local temp table with one VARCHAR column.
    pub fn temp_text_table(
        &self,
        name: &str,
        column: &str,
        rows: &[String],
    ) -> Result<(), QueryError> {
        self.conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {name} ({column} VARCHAR)"
        ))?;
        let mut appender = self.conn.appender_to_catalog_and_db(name, "temp", "main")?;
        for row in rows {
            appender.append_row(duckdb::params![row])?;
        }
        appender.flush()?;
        Ok(())
    }

    /// Replace a connection-local temp table of BIGINT columns.
    pub fn temp_i64_table(
        &self,
        name: &str,
        columns: &[&str],
        rows: &[Vec<i64>],
    ) -> Result<(), QueryError> {
        let definition: Vec<String> = columns
            .iter()
            .map(|column| format!("{column} BIGINT"))
            .collect();
        self.conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {name} ({})",
            definition.join(", ")
        ))?;
        let mut appender = self.conn.appender_to_catalog_and_db(name, "temp", "main")?;
        for row in rows {
            appender.append_row(duckdb::appender_params_from_iter(row.iter()))?;
        }
        appender.flush()?;
        Ok(())
    }
}

/// Rows one per-person partition aims for.
pub const PARTITION_ROWS: u64 = 2_000_000;
const MAX_PARTITIONS: u64 = 1_024;

/// SQL selecting one person-hash partition (server integers only).
pub(crate) fn partition_clause(partition: u64, partitions: u64) -> String {
    if partitions <= 1 {
        "TRUE".to_owned()
    } else {
        format!("hash(person_id) % {partitions} = {partition}")
    }
}

/// Arrow column helpers.
pub(crate) fn string_column(batch: &RecordBatch, index: usize) -> Result<StringArray, QueryError> {
    let column = batch.column(index);
    let cast: ArrayRef = match column.data_type() {
        DataType::Utf8 => column.clone(),
        _ => duckdb::arrow::compute::cast(column, &DataType::Utf8)?,
    };
    Ok(cast.as_string::<i32>().clone())
}

pub(crate) fn i64_column(batch: &RecordBatch, index: usize) -> Result<Int64Array, QueryError> {
    let column = batch.column(index);
    let cast: ArrayRef = match column.data_type() {
        DataType::Int64 => column.clone(),
        _ => duckdb::arrow::compute::cast(column, &DataType::Int64)?,
    };
    Ok(cast
        .as_primitive::<duckdb::arrow::datatypes::Int64Type>()
        .clone())
}

/// What a kind resolved before touching data.
pub(crate) enum Prepared {
    Trends(trends::Plan),
    Funnels(ResolvedRange),
    Retention(retention::Plan),
    Lifecycle(lifecycle::Plan),
    Stickiness(ResolvedRange),
    Paths(ResolvedRange),
    Sql,
}

impl Prepared {
    /// `[from, to)` of events read; `None` = from the first event / to the
    /// last.
    fn load_span(&self) -> (Option<i64>, Option<i64>) {
        match self {
            Self::Trends(plan) => (Some(plan.load_from()), Some(plan.range.to)),
            Self::Funnels(range) | Self::Stickiness(range) | Self::Paths(range) => {
                (Some(range.from), Some(range.to))
            }
            Self::Retention(plan) => (plan.load_from(), Some(plan.end)),
            Self::Lifecycle(plan) => (None, Some(plan.range.to)),
            Self::Sql => (None, None),
        }
    }

    fn meta_range(&self, now: DateTime<Utc>) -> (String, String) {
        match self {
            Self::Trends(plan) => (plan.range.date_from(), plan.range.date_to()),
            Self::Funnels(range) | Self::Stickiness(range) | Self::Paths(range) => {
                (range.date_from(), range.date_to())
            }
            Self::Retention(plan) => (
                range::rfc3339(plan.starts[0]),
                range::rfc3339_micros(plan.end - 1),
            ),
            Self::Lifecycle(plan) => (plan.range.date_from(), plan.range.date_to()),
            Self::Sql => (String::new(), range::rfc3339(range::from_datetime(now))),
        }
    }

    fn fingerprint(&self) -> String {
        match self {
            Self::Trends(plan) => {
                format!("{:?}", (plan.range, plan.previous.as_ref().map(|p| p.0)))
            }
            Self::Funnels(range) | Self::Stickiness(range) | Self::Paths(range) => {
                format!("{range:?}")
            }
            Self::Retention(plan) => format!("{:?}", (&plan.starts, plan.end)),
            Self::Lifecycle(plan) => format!("{:?}", plan.range),
            Self::Sql => String::new(),
        }
    }
}

/// Mode of one engine session.
enum Work<'a> {
    Result,
    Actors(&'a ActorSelection),
}

enum Output {
    Result(InsightResult),
    Actors(Vec<String>),
}

pub struct QueryEngine {
    source: Arc<dyn EventSource>,
    persons: Arc<PersonStore>,
    config: EngineConfig,
    pool: Pool,
    identity: Mutex<identity::IdentitySync>,
    cache: ResultCache<Arc<InsightResult>>,
    queue: Arc<Semaphore>,
    execution: Arc<Semaphore>,
    schemas: Mutex<HashMap<[u8; 32], EventSchema>>,
}

impl QueryEngine {
    pub fn new(
        source: Arc<dyn EventSource>,
        persons: Arc<PersonStore>,
        config: EngineConfig,
    ) -> Result<Self, QueryError> {
        if config.connections == 0 || config.threads == 0 || config.memory_limit_mb == 0 {
            return Err(QueryError::internal("engine limits must be positive"));
        }
        let duck_config = duckdb::Config::default()
            .max_memory(&format!("{}MB", config.memory_limit_mb))?
            .threads(i64::from(config.threads))?
            .enable_autoload_extension(false)?;
        let root = Connection::open_in_memory_with_flags(duck_config)?;
        let mut setup = String::from(
            "SET autoinstall_known_extensions = false; \
             SET parquet_metadata_cache = true; \
             CREATE TABLE person_overrides (project_id VARCHAR NOT NULL, \
                 distinct_id VARCHAR NOT NULL, person_id VARCHAR NOT NULL);",
        );
        // Absolute, so a relative data dir never becomes a spill directory
        // relative to the working directory of whichever thread runs a query.
        let mut config = config;
        if let Some(directory) = &config.temp_directory {
            let absolute = std::path::absolute(directory)
                .map_err(|error| QueryError::internal(format!("temp directory: {error}")))?;
            std::fs::create_dir_all(&absolute)
                .map_err(|error| QueryError::internal(format!("temp directory: {error}")))?;
            setup.push_str(&format!(
                "SET temp_directory = {};",
                sql::string_literal(&absolute.to_string_lossy())?
            ));
            config.temp_directory = Some(absolute);
        }
        root.execute_batch(&setup)?;
        let mut idle = Vec::with_capacity(config.connections);
        for _ in 0..config.connections {
            let connection = root.try_clone()?;
            set_utc(&connection);
            idle.push(connection);
        }
        let sync_connection = root.try_clone()?;
        set_utc(&sync_connection);
        Ok(Self {
            source,
            persons,
            pool: Pool {
                idle: Mutex::new(idle),
                available: Condvar::new(),
            },
            identity: Mutex::new(identity::IdentitySync::new(sync_connection)),
            cache: ResultCache::new(config.cache_entries),
            queue: Arc::new(Semaphore::new(config.max_queued + config.connections)),
            execution: Arc::new(Semaphore::new(config.connections)),
            schemas: Mutex::new(HashMap::new()),
            config,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Bounded admission: a full queue or a wait past `queue_wait` is
    /// `Busy`.
    pub async fn admit(&self) -> Result<Admission, QueryError> {
        let queue = self
            .queue
            .clone()
            .try_acquire_owned()
            .map_err(|_| QueryError::Busy)?;
        let execution = tokio::time::timeout(
            self.config.queue_wait,
            self.execution.clone().acquire_owned(),
        )
        .await
        .map_err(|_| QueryError::Busy)?
        .map_err(|_| QueryError::Busy)?;
        Ok(Admission {
            _queue: queue,
            _execution: execution,
        })
    }

    pub fn run(
        &self,
        project_id: &str,
        request: &QueryRequest,
    ) -> Result<QueryResponse, QueryError> {
        self.run_at(project_id, request, Utc::now())
    }

    pub fn run_at(
        &self,
        project_id: &str,
        request: &QueryRequest,
        now: DateTime<Utc>,
    ) -> Result<QueryResponse, QueryError> {
        let started = Instant::now();
        let (output, meta) = self.session(
            project_id,
            &request.query,
            now,
            request.refresh,
            Work::Result,
        )?;
        match output {
            Output::Result(result) => {
                let mut meta = meta;
                meta.elapsed_ms = started.elapsed().as_millis() as u64;
                Ok(QueryResponse { result, meta })
            }
            Output::Actors(_) => Err(QueryError::internal("unexpected actors output")),
        }
    }

    pub fn actors(
        &self,
        project_id: &str,
        request: &ActorsRequest,
    ) -> Result<ActorsResponse, QueryError> {
        self.actors_at(project_id, request, Utc::now())
    }

    pub fn actors_at(
        &self,
        project_id: &str,
        request: &ActorsRequest,
        now: DateTime<Utc>,
    ) -> Result<ActorsResponse, QueryError> {
        if request.limit == 0 || request.limit > MAX_ACTOR_PAGE {
            return Err(QueryError::invalid(format!(
                "limit must be between 1 and {MAX_ACTOR_PAGE}"
            )));
        }
        if request.offset > MAX_ACTOR_OFFSET {
            return Err(QueryError::invalid(format!(
                "offset must be at most {MAX_ACTOR_OFFSET}"
            )));
        }
        let ids = self.actor_ids_at(project_id, &request.query, &request.selection, now)?;
        let start = (request.offset as usize).min(ids.len());
        let end = start.saturating_add(request.limit as usize).min(ids.len());
        let persons = identity::person_summaries(&self.persons, project_id, &ids[start..end])?;
        Ok(ActorsResponse {
            persons,
            has_more: end < ids.len(),
        })
    }

    /// Every person behind one number, sorted by person id.
    pub fn actor_ids_at(
        &self,
        project_id: &str,
        query: &InsightQuery,
        selection: &ActorSelection,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, QueryError> {
        match self
            .session(project_id, query, now, true, Work::Actors(selection))?
            .0
        {
            Output::Actors(ids) => Ok(ids),
            Output::Result(_) => Err(QueryError::internal("unexpected result output")),
        }
    }

    fn connection(&self) -> Result<PooledConnection<'_>, QueryError> {
        let deadline = Instant::now() + self.config.queue_wait;
        let mut idle = self
            .pool
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(connection) = idle.pop() {
                return Ok(PooledConnection {
                    pool: &self.pool,
                    connection: Some(connection),
                });
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(QueryError::Busy);
            }
            idle = self
                .pool
                .available
                .wait_timeout(idle, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    fn files(&self, project_id: &str, span: (Option<i64>, Option<i64>)) -> EventFiles {
        let from = span.0.map(range::day_of).unwrap_or(NaiveDate::MIN);
        let to = span
            .1
            .map(|to| range::day_of(to - 1).succ_opt().unwrap_or(NaiveDate::MAX))
            .unwrap_or(NaiveDate::MAX);
        self.source.files(project_id, from, to)
    }

    fn schema(&self, conn: &Connection, files: &EventFiles) -> Result<EventSchema, QueryError> {
        if files.is_empty() {
            return Ok(EventSchema::complete());
        }
        let mut hasher = Sha256::new();
        hasher.update(files.data_version.to_le_bytes());
        for path in &files.paths {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update([0]);
        }
        let key: [u8; 32] = hasher.finalize().into();
        if let Some(schema) = self
            .schemas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return Ok(schema.clone());
        }
        let mut list = Vec::with_capacity(files.paths.len());
        for path in &files.paths {
            list.push(sql::string_literal(&path.to_string_lossy())?);
        }
        let mut statement = conn.prepare(&format!(
            "DESCRIBE SELECT * FROM read_parquet([{}], union_by_name = true)",
            list.join(", ")
        ))?;
        let columns: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        let schema = EventSchema::from_columns(&columns);
        let mut schemas = self
            .schemas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if schemas.len() >= MAX_SCHEMA_CACHE {
            schemas.clear();
        }
        schemas.insert(key, schema.clone());
        Ok(schema)
    }

    fn earliest(
        &self,
        conn: &Connection,
        files: &EventFiles,
        schema: &EventSchema,
        project_id: &str,
    ) -> Result<Option<i64>, QueryError> {
        let source = Source {
            files: &files.paths,
            schema,
            project_id,
            person_keys: &[],
        };
        let mut params = Params::new();
        let relation = source.relation(&mut params, None, None, false)?;
        let sql = format!("SELECT min(ts) FROM ({relation})");
        let mut statement = conn.prepare(&sql)?;
        Ok(
            statement.query_row(duckdb::params_from_iter(params.values()), |row| {
                row.get::<_, Option<i64>>(0)
            })?,
        )
    }

    fn session(
        &self,
        project_id: &str,
        query: &InsightQuery,
        now: DateTime<Utc>,
        refresh: bool,
        work: Work<'_>,
    ) -> Result<(Output, QueryMeta), QueryError> {
        validate::query(query)?;
        if let Work::Actors(selection) = &work {
            validate::selection(query, selection)?;
        }
        let identity_version = {
            let mut sync = self
                .identity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sync.sync(&self.persons)?
        };
        let conn = self.connection()?;
        let watchdog = Watchdog::start(conn.interrupt_handle(), self.config.timeout);
        let deadline = Instant::now() + self.config.timeout;
        let result = self.session_inner(
            &conn,
            project_id,
            query,
            now,
            refresh,
            work,
            identity_version,
            deadline,
        );
        let fired = watchdog.fired();
        drop(watchdog);
        match result {
            Err(_) if fired => Err(QueryError::Timeout),
            Err(QueryError::Internal(message)) if message.contains("INTERRUPT") => {
                Err(QueryError::Timeout)
            }
            Err(QueryError::Internal(message)) if message.contains("Out of Memory") => {
                Err(QueryError::too_large(
                    "the query exceeded the engine's memory limit; narrow the date range or filters",
                ))
            }
            other => other,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn session_inner(
        &self,
        conn: &Connection,
        project_id: &str,
        query: &InsightQuery,
        now: DateTime<Utc>,
        refresh: bool,
        work: Work<'_>,
        identity_version: (u64, u64),
        deadline: Instant,
    ) -> Result<(Output, QueryMeta), QueryError> {
        let all_files = if uses_all(query) {
            Some(self.files(project_id, (None, None)))
        } else {
            None
        };
        let earliest = || -> Result<Option<i64>, QueryError> {
            match &all_files {
                Some(files) => {
                    let schema = self.schema(conn, files)?;
                    self.earliest(conn, files, &schema, project_id)
                }
                None => Ok(None),
            }
        };
        let prepared = prepare(query, now, earliest)?;
        let files = match all_files {
            Some(files) => files,
            None => self.files(project_id, prepared.load_span()),
        };
        let (date_from, date_to) = prepared.meta_range(now);
        let meta = QueryMeta {
            kind: query.kind().to_owned(),
            elapsed_ms: 0,
            cached: false,
            data_version: files.data_version,
            date_from,
            date_to,
            timezone: "UTC".to_owned(),
        };
        let person_keys = person_keys(query);
        let cacheable = matches!(work, Work::Result) && !matches!(query, InsightQuery::SqlQuery(_));
        let key =
            cacheable.then(|| cache_key(project_id, query, &prepared, &files, identity_version));
        if let Some(key) = &key
            && !refresh
            && let Some(result) = self.cache.get(key)
        {
            let mut meta = meta;
            meta.cached = true;
            return Ok((Output::Result(InsightResult::clone(&result)), meta));
        }

        validate_regexes(conn, query)?;
        let schema = self.schema(conn, &files)?;
        for (index, key) in person_keys.iter().enumerate() {
            identity::load_person_property(conn, &self.persons, project_id, key, index)?;
        }
        let ctx = Ctx {
            conn,
            source: Source {
                files: &files.paths,
                schema: &schema,
                project_id,
                person_keys: &person_keys,
            },
            project_id,
            deadline,
            sql_memory_limit_mb: self.config.sql_memory_limit_mb,
            threads: self.config.threads,
            partition_rows: self.config.partition_rows.max(1),
            temp_directory: self.config.temp_directory.as_deref(),
        };
        let output = match work {
            Work::Result => Output::Result(run_kind(&ctx, query, &prepared)?),
            Work::Actors(selection) => {
                let mut ids = actors_kind(&ctx, query, &prepared, selection)?;
                ids.sort();
                ids.dedup();
                Output::Actors(ids)
            }
        };
        if let (Some(key), Output::Result(result)) = (key, &output)
            && let Ok(bytes) = serde_json::to_vec(result)
        {
            let ttl = (!person_keys.is_empty()).then_some(PERSON_PROPERTY_CACHE_TTL);
            self.cache
                .put(key, Arc::new(result.clone()), bytes.len(), ttl);
        }
        Ok((output, meta))
    }
}

/// Pin the session time zone to UTC. Without the ICU extension DuckDB has no
/// `TimeZone` setting and `TIMESTAMPTZ` is UTC already, so failure is fine.
/// The engine never relies on it: buckets come from epoch microseconds.
pub(crate) fn set_utc(connection: &Connection) {
    let _ = connection.execute_batch("SET TimeZone = 'UTC';");
}

/// The engine's request validation, for callers that store queries.
pub fn validate_query(query: &InsightQuery) -> Result<(), QueryError> {
    validate::query(query)
}

fn uses_all(query: &InsightQuery) -> bool {
    match query {
        InsightQuery::TrendsQuery(q) => range::date_from_is_all(&q.date_range),
        InsightQuery::FunnelsQuery(q) => range::date_from_is_all(&q.date_range),
        InsightQuery::LifecycleQuery(q) => range::date_from_is_all(&q.date_range),
        InsightQuery::StickinessQuery(q) => range::date_from_is_all(&q.date_range),
        InsightQuery::PathsQuery(q) => range::date_from_is_all(&q.date_range),
        InsightQuery::RetentionQuery(_) | InsightQuery::SqlQuery(_) => false,
    }
}

fn prepare(
    query: &InsightQuery,
    now: DateTime<Utc>,
    earliest: impl FnOnce() -> Result<Option<i64>, QueryError>,
) -> Result<Prepared, QueryError> {
    Ok(match query {
        InsightQuery::TrendsQuery(q) => Prepared::Trends(trends::plan(q, now, earliest)?),
        InsightQuery::FunnelsQuery(q) => Prepared::Funnels(range::resolve(
            &q.date_range,
            crate::contract::common::Interval::Day,
            now,
            earliest,
        )?),
        InsightQuery::RetentionQuery(q) => Prepared::Retention(retention::plan(q, now)?),
        InsightQuery::LifecycleQuery(q) => Prepared::Lifecycle(lifecycle::plan(q, now, earliest)?),
        InsightQuery::StickinessQuery(q) => {
            let range = range::resolve(&q.date_range, q.interval, now, earliest)?;
            range.buckets()?;
            Prepared::Stickiness(range)
        }
        InsightQuery::PathsQuery(q) => Prepared::Paths(range::resolve(
            &q.date_range,
            crate::contract::common::Interval::Day,
            now,
            earliest,
        )?),
        InsightQuery::SqlQuery(_) => Prepared::Sql,
    })
}

fn run_kind(
    ctx: &Ctx<'_>,
    query: &InsightQuery,
    prepared: &Prepared,
) -> Result<InsightResult, QueryError> {
    match (query, prepared) {
        (InsightQuery::TrendsQuery(q), Prepared::Trends(plan)) => trends::run(ctx, q, plan),
        (InsightQuery::FunnelsQuery(q), Prepared::Funnels(range)) => funnels::run(ctx, q, range),
        (InsightQuery::RetentionQuery(q), Prepared::Retention(plan)) => {
            retention::run(ctx, q, plan)
        }
        (InsightQuery::LifecycleQuery(q), Prepared::Lifecycle(plan)) => {
            lifecycle::run(ctx, q, plan)
        }
        (InsightQuery::StickinessQuery(q), Prepared::Stickiness(range)) => {
            stickiness::run(ctx, q, range)
        }
        (InsightQuery::PathsQuery(q), Prepared::Paths(range)) => paths::run(ctx, q, range),
        (InsightQuery::SqlQuery(q), Prepared::Sql) => sql_query::run(ctx, q),
        _ => Err(QueryError::internal("query and plan disagree")),
    }
}

fn actors_kind(
    ctx: &Ctx<'_>,
    query: &InsightQuery,
    prepared: &Prepared,
    selection: &ActorSelection,
) -> Result<Vec<String>, QueryError> {
    match (query, prepared, selection) {
        (
            InsightQuery::TrendsQuery(q),
            Prepared::Trends(plan),
            ActorSelection::TrendsPoint {
                series_index,
                day,
                breakdown_value,
            },
        ) => trends::actors(ctx, q, plan, *series_index, day, breakdown_value.as_deref()),
        (
            InsightQuery::FunnelsQuery(q),
            Prepared::Funnels(range),
            ActorSelection::FunnelStep {
                step,
                converted,
                breakdown_value,
            },
        ) => funnels::actors(ctx, q, range, *step, *converted, breakdown_value.as_deref()),
        (
            InsightQuery::RetentionQuery(q),
            Prepared::Retention(plan),
            ActorSelection::RetentionCell {
                cohort_date,
                interval,
            },
        ) => retention::actors(ctx, q, plan, cohort_date, *interval),
        (
            InsightQuery::LifecycleQuery(q),
            Prepared::Lifecycle(plan),
            ActorSelection::LifecycleCell { status, day },
        ) => lifecycle::actors(ctx, q, plan, *status, day),
        (
            InsightQuery::StickinessQuery(q),
            Prepared::Stickiness(range),
            ActorSelection::StickinessBar {
                series_index,
                intervals,
            },
        ) => stickiness::actors(ctx, q, range, *series_index, *intervals),
        (
            InsightQuery::PathsQuery(q),
            Prepared::Paths(range),
            ActorSelection::PathsLink { source, target },
        ) => paths::actors(ctx, q, range, source, target),
        _ => Err(QueryError::invalid(
            "the actor selection does not match the query kind",
        )),
    }
}

/// Every property filter of a query.
fn all_filters(query: &InsightQuery) -> Vec<&PropertyFilter> {
    let mut lists: Vec<&[PropertyFilter]> = Vec::new();
    match query {
        InsightQuery::TrendsQuery(q) => {
            lists.push(&q.properties);
            lists.extend(q.series.iter().map(|s| s.properties.as_slice()));
        }
        InsightQuery::FunnelsQuery(q) => {
            lists.push(&q.properties);
            lists.extend(q.series.iter().map(|s| s.properties.as_slice()));
        }
        InsightQuery::RetentionQuery(q) => {
            lists.push(&q.properties);
            lists.push(&q.target.properties);
            lists.push(&q.returning.properties);
        }
        InsightQuery::LifecycleQuery(q) => {
            lists.push(&q.properties);
            lists.push(&q.series.properties);
        }
        InsightQuery::StickinessQuery(q) => {
            lists.push(&q.properties);
            lists.extend(q.series.iter().map(|s| s.properties.as_slice()));
        }
        InsightQuery::PathsQuery(q) => lists.push(&q.properties),
        InsightQuery::SqlQuery(_) => {}
    }
    lists.into_iter().flatten().collect()
}

/// Every person-property key the query reads, in a stable order.
fn person_keys(query: &InsightQuery) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for filter in all_filters(query) {
        if filter.source == PropertySource::Person && !keys.contains(&filter.key) {
            keys.push(filter.key.clone());
        }
    }
    let breakdown = match query {
        InsightQuery::TrendsQuery(q) => q.breakdown.as_ref(),
        InsightQuery::FunnelsQuery(q) => q.breakdown.as_ref(),
        _ => None,
    };
    if let Some(breakdown) = breakdown
        && breakdown.source == PropertySource::Person
        && !keys.contains(&breakdown.property)
    {
        keys.push(breakdown.property.clone());
    }
    keys
}

/// Reject invalid regular expressions as a 400 before they reach a scan.
fn validate_regexes(conn: &Connection, query: &InsightQuery) -> Result<(), QueryError> {
    use crate::contract::common::PropertyOperator;
    for filter in all_filters(query) {
        if matches!(
            filter.operator,
            PropertyOperator::Regex | PropertyOperator::NotRegex
        ) {
            let pattern = sql::filter_scalar(&filter.value)?;
            conn.query_row(
                "SELECT regexp_matches('', $1::VARCHAR)",
                [&pattern],
                |row| row.get::<_, Option<bool>>(0),
            )
            .map_err(|_| {
                QueryError::invalid(format!("invalid regular expression for `{}`", filter.key))
            })?;
        }
    }
    Ok(())
}

fn cache_key(
    project_id: &str,
    query: &InsightQuery,
    prepared: &Prepared,
    files: &EventFiles,
    identity: (u64, u64),
) -> CacheKey {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(query).unwrap_or_default());
    hasher.update([0]);
    hasher.update(prepared.fingerprint().as_bytes());
    hasher.update([0]);
    hasher.update((files.paths.len() as u64).to_le_bytes());
    CacheKey {
        project_id: project_id.to_owned(),
        query_hash: hasher.finalize().into(),
        data_version: files.data_version,
        identity_epoch: identity.0,
        identity_seq: identity.1,
    }
}
