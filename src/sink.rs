//! Event sink — where accepted events go after the wire edge.
//!
//! The production implementation is [`DurableWalSink`]: `append` returning
//! `Ok` is the durability promise that justifies a 2xx to the client, so no
//! implementation may ack before its write is durable. [`MemorySink`] exists
//! for tests.
//!
//! Two threads, two jobs:
//! - the **writer** drains the bounded request queue, writes every waiting
//!   batch, pays for one fsync per group, then acks the whole group. It seals
//!   the active WAL segment once a second so the publisher can read it.
//! - the **publisher** turns sealed segments into queryable files, compacts
//!   partitions, and reclaims WAL. It never sits in the ack path: a slow
//!   publication delays freshness, never an acknowledgement.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc as sync_mpsc};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

use crate::capture::event::CapturedEvent;
use crate::lake::Lake;
use crate::lake::compactor::Compactor;
use crate::lake::publisher::{PublishError, Publisher};
use crate::pipeline::wal::{CapturedBatch, Recovery, WalConfig, WalError, WriteAheadLog};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkError {
    /// Transient failure → 503, which posthog-js retries.
    Retryable,
    /// The batch itself can never be stored → 400, never retried.
    Fatal,
}

/// A parsed batch together with the durable authorization decisions made at
/// the capture seam. Production sinks must preserve these project bindings.
#[derive(Debug, Clone)]
pub struct AuthorizedEventBatch {
    pub events: Vec<CapturedEvent>,
    pub project_ids_by_token: BTreeMap<String, String>,
    pub historical_migration: bool,
}

#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    async fn append(&self, batch: AuthorizedEventBatch) -> Result<(), SinkError>;
}

/// Explicit bound: no unbounded growth in the ingest path, even in the stub.
pub const MEMORY_SINK_MAX_EVENTS: usize = 1_000_000;

#[derive(Default)]
pub struct MemorySink {
    events: Mutex<Vec<CapturedEvent>>,
}

impl MemorySink {
    pub fn snapshot(&self) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl EventSink for MemorySink {
    async fn append(&self, mut batch: AuthorizedEventBatch) -> Result<(), SinkError> {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() + batch.events.len() > MEMORY_SINK_MAX_EVENTS {
            return Err(SinkError::Retryable);
        }
        events.append(&mut batch.events);
        Ok(())
    }
}

/// Batches waiting for the writer. Full → 503, which SDKs retry.
pub const QUEUE_BATCHES: usize = 4096;
/// Bytes of batches waiting for the writer.
pub const QUEUE_BYTES: usize = 128 * 1024 * 1024;
/// One group commit takes at most this many batches…
pub const GROUP_MAX_BATCHES: usize = 1024;
/// …or this many encoded bytes.
pub const GROUP_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Sealed + active WAL may not exceed this; beyond it publication is too far
/// behind and capture sheds load (503).
pub const MAX_UNPUBLISHED_WAL_BYTES: u64 = 1024 * 1024 * 1024;
/// How often the writer seals the active segment for the publisher.
pub const SEAL_INTERVAL: Duration = Duration::from_millis(1000);
/// How often the publisher wakes without a seal notification (compaction,
/// graveyard sweeps, retries).
pub const PUBLISHER_TICK: Duration = Duration::from_millis(1000);

#[derive(Debug)]
pub enum DurablePipelineError {
    Wal(WalError),
    Publish(PublishError),
    Lake(crate::lake::LakeError),
    Io { path: PathBuf, source: std::io::Error },
    WorkerPanicked,
}

impl fmt::Display for DurablePipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wal(error) => error.fmt(formatter),
            Self::Publish(error) => error.fmt(formatter),
            Self::Lake(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::WorkerPanicked => formatter.write_str("durable pipeline worker panicked"),
        }
    }
}

impl std::error::Error for DurablePipelineError {}

impl From<WalError> for DurablePipelineError {
    fn from(error: WalError) -> Self {
        Self::Wal(error)
    }
}

impl From<PublishError> for DurablePipelineError {
    fn from(error: PublishError) -> Self {
        Self::Publish(error)
    }
}

impl From<crate::lake::LakeError> for DurablePipelineError {
    fn from(error: crate::lake::LakeError) -> Self {
        Self::Lake(error)
    }
}

/// Freshness and backlog, shared by both threads and read by `/status`.
#[derive(Debug, Default)]
pub struct PipelineStats {
    /// Bytes in sealed, unreclaimed WAL segments (publisher-owned).
    sealed_bytes: AtomicU64,
    /// Bytes in the active segment (writer-owned).
    active_bytes: AtomicU64,
    /// Receive time (ms) of the first batch in the active segment; 0 if empty.
    active_oldest_ms: AtomicI64,
    /// `(segment, receive time of its first batch)` for sealed segments the
    /// publisher has not fully published yet, oldest first.
    sealed_oldest: Mutex<std::collections::VecDeque<(u64, i64)>>,
    acked_events: AtomicU64,
    published_events: AtomicU64,
}

impl PipelineStats {
    /// Seconds between the oldest acknowledged-but-unqueryable batch and now;
    /// 0 when everything acknowledged is queryable.
    pub fn lag_seconds(&self) -> f64 {
        let sealed = self
            .sealed_oldest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .front()
            .map(|(_, ms)| *ms);
        let active = Some(self.active_oldest_ms.load(Ordering::Relaxed)).filter(|ms| *ms > 0);
        let oldest = match (sealed, active) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return 0.0,
        };
        let now = chrono::Utc::now().timestamp_millis();
        ((now - oldest).max(0) as f64) / 1000.0
    }

    pub fn acked_events(&self) -> u64 {
        self.acked_events.load(Ordering::Relaxed)
    }

    pub fn published_events(&self) -> u64 {
        self.published_events.load(Ordering::Relaxed)
    }

    pub fn unpublished_bytes(&self) -> u64 {
        self.sealed_bytes
            .load(Ordering::Relaxed)
            .saturating_add(self.active_bytes.load(Ordering::Relaxed))
    }

    fn note_acked(&self, received_ms: i64, events: u64) {
        self.acked_events.fetch_add(events, Ordering::Relaxed);
        let _ = self.active_oldest_ms.compare_exchange(
            0,
            received_ms.max(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    fn note_sealed(&self, segment: u64) {
        let oldest = self.active_oldest_ms.swap(0, Ordering::Relaxed);
        if oldest > 0 {
            self.sealed_oldest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back((segment, oldest));
        }
    }

    /// Everything before `checkpoint_segment` is published.
    fn note_published(&self, events: u64, checkpoint_segment: u64) {
        self.published_events.fetch_add(events, Ordering::Relaxed);
        let mut sealed = self
            .sealed_oldest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while sealed
            .front()
            .is_some_and(|(segment, _)| *segment < checkpoint_segment)
        {
            sealed.pop_front();
        }
    }
}

enum WriterRequest {
    Append {
        batch: CapturedBatch,
        encoded_bytes: u64,
        _queue_bytes: OwnedSemaphorePermit,
        ack: oneshot::Sender<Result<(), SinkError>>,
    },
    /// Seal, then hand the erasure to the publisher so it covers every
    /// event acknowledged before it.
    Erase {
        project_id: String,
        person_id: String,
        ack: oneshot::Sender<Result<ErasureReport, DurablePipelineError>>,
    },
    Shutdown {
        ack: oneshot::Sender<Result<(), DurablePipelineError>>,
    },
}

enum PublisherSignal {
    Sealed,
    Erase {
        project_id: String,
        person_id: String,
        ack: oneshot::Sender<Result<ErasureReport, DurablePipelineError>>,
    },
    Shutdown {
        ack: std::sync::mpsc::SyncSender<Result<(), DurablePipelineError>>,
    },
}

/// What a person erasure removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ErasureReport {
    pub distinct_ids: usize,
    pub events: u64,
}

/// The production capture adapter.
pub struct DurableWalSink {
    sender: sync_mpsc::SyncSender<WriterRequest>,
    queue_bytes: Arc<Semaphore>,
}

pub struct DurableWalRuntime {
    sender: sync_mpsc::SyncSender<WriterRequest>,
    writer: Option<std::thread::JoinHandle<()>>,
    publisher: Option<std::thread::JoinHandle<()>>,
    stats: Arc<PipelineStats>,
}

/// Configuration of the durable pipeline.
pub struct PipelineConfig {
    pub wal_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub retention_days: Option<u32>,
}

impl DurableWalSink {
    /// Recover the WAL, publish everything it holds (so acknowledged events
    /// are queryable before the server reports ready), then start both
    /// threads.
    pub fn open(
        config: PipelineConfig,
        lake: Arc<Lake>,
    ) -> Result<(Arc<Self>, DurableWalRuntime, Recovery), DurablePipelineError> {
        let (mut wal, recovery) = WriteAheadLog::open(&config.wal_dir, WalConfig::default())?;
        wal.seal()?;
        let publisher = Publisher::new(lake.clone(), wal.reader());
        let recovered = publisher.publish_all()?;
        if !recovered.is_empty() {
            tracing::info!(
                windows = recovered.len(),
                events = recovered.iter().map(|p| p.events).sum::<usize>(),
                "published recovered WAL records"
            );
        }
        let compactor = Compactor::new(lake, &config.tmp_dir, config.retention_days)?;
        let stats = Arc::new(PipelineStats::default());
        stats.sealed_bytes.store(
            wal.reader().sealed_bytes().unwrap_or(0),
            Ordering::Relaxed,
        );

        let (signal_tx, signal_rx) = sync_mpsc::sync_channel(64);
        let publisher_stats = stats.clone();
        let publisher_thread = std::thread::Builder::new()
            .name("hoglet-publisher".to_owned())
            .spawn(move || publisher_loop(publisher, compactor, signal_rx, publisher_stats))
            .map_err(|source| DurablePipelineError::Io {
                path: PathBuf::from("<hoglet-publisher-thread>"),
                source,
            })?;

        let (sender, receiver) = sync_mpsc::sync_channel(QUEUE_BATCHES);
        let writer_stats = stats.clone();
        let writer_thread = std::thread::Builder::new()
            .name("hoglet-wal-writer".to_owned())
            .spawn(move || writer_loop(wal, receiver, signal_tx, writer_stats))
            .map_err(|source| DurablePipelineError::Io {
                path: PathBuf::from("<hoglet-wal-writer-thread>"),
                source,
            })?;

        Ok((
            Arc::new(Self {
                sender: sender.clone(),
                queue_bytes: Arc::new(Semaphore::new(QUEUE_BYTES)),
            }),
            DurableWalRuntime {
                sender,
                writer: Some(writer_thread),
                publisher: Some(publisher_thread),
                stats,
            },
            recovery,
        ))
    }
}

#[async_trait::async_trait]
impl EventSink for DurableWalSink {
    async fn append(&self, batch: AuthorizedEventBatch) -> Result<(), SinkError> {
        let batch = CapturedBatch::authorized(
            batch.events,
            batch.project_ids_by_token,
            batch.historical_migration,
        )
        .map_err(|_| SinkError::Fatal)?;
        // Cheap upper bound for queue accounting; the WAL encodes for real.
        let encoded_bytes: usize = batch
            .events
            .iter()
            .map(|event| {
                256 + event.event.len()
                    + event.distinct_id.len()
                    + event.properties.len() * 64
            })
            .sum();
        let permits = u32::try_from(encoded_bytes.min(QUEUE_BYTES)).map_err(|_| SinkError::Fatal)?;
        let queue_bytes = self
            .queue_bytes
            .clone()
            .try_acquire_many_owned(permits)
            .map_err(|_| SinkError::Retryable)?;
        let (ack, receive) = oneshot::channel();
        self.sender
            .try_send(WriterRequest::Append {
                batch,
                encoded_bytes: encoded_bytes as u64,
                _queue_bytes: queue_bytes,
                ack,
            })
            .map_err(|_| SinkError::Retryable)?;
        receive.await.map_err(|_| SinkError::Retryable)?
    }
}

impl DurableWalRuntime {
    pub fn stats(&self) -> Arc<PipelineStats> {
        self.stats.clone()
    }

    /// A cloneable handle for erasure requests.
    pub fn eraser(&self) -> Eraser {
        Eraser {
            writer: self.sender.clone(),
        }
    }

    /// Stop accepting, flush and fsync, publish everything, then stop.
    pub async fn shutdown(mut self) -> Result<(), DurablePipelineError> {
        let (ack, receive) = oneshot::channel();
        let sender = self.sender.clone();
        let sent =
            tokio::task::spawn_blocking(move || sender.send(WriterRequest::Shutdown { ack })).await;
        let result = if matches!(sent, Ok(Ok(()))) {
            receive
                .await
                .unwrap_or(Err(DurablePipelineError::WorkerPanicked))
        } else {
            Err(DurablePipelineError::WorkerPanicked)
        };
        for handle in [self.writer.take(), self.publisher.take()].into_iter().flatten() {
            if !matches!(
                tokio::task::spawn_blocking(move || handle.join()).await,
                Ok(Ok(()))
            ) {
                return Err(DurablePipelineError::WorkerPanicked);
            }
        }
        result
    }
}

/// Requests physical erasure of a person, serialized with publication and
/// compaction on the publisher thread.
#[derive(Clone)]
pub struct Eraser {
    writer: sync_mpsc::SyncSender<WriterRequest>,
}

impl Eraser {
    /// Publish everything acknowledged so far, then remove the person, their
    /// distinct ids, and every stored event of those distinct ids.
    pub async fn erase_person(
        &self,
        project_id: &str,
        person_id: &str,
    ) -> Result<ErasureReport, DurablePipelineError> {
        let (ack, receive) = oneshot::channel();
        self.writer
            .try_send(WriterRequest::Erase {
                project_id: project_id.to_owned(),
                person_id: person_id.to_owned(),
                ack,
            })
            .map_err(|_| DurablePipelineError::WorkerPanicked)?;
        receive
            .await
            .unwrap_or(Err(DurablePipelineError::WorkerPanicked))
    }
}

struct Pending {
    ack: oneshot::Sender<Result<(), SinkError>>,
    result: Result<(), SinkError>,
    events: u64,
    received_ms: i64,
    _queue_bytes: OwnedSemaphorePermit,
}

fn writer_loop(
    mut wal: WriteAheadLog,
    receiver: sync_mpsc::Receiver<WriterRequest>,
    publisher: sync_mpsc::SyncSender<PublisherSignal>,
    stats: Arc<PipelineStats>,
) {
    let mut last_seal = Instant::now();
    let mut shutdown_ack = None;
    loop {
        let wait = SEAL_INTERVAL.saturating_sub(last_seal.elapsed());
        let first = match receiver.recv_timeout(wait) {
            Ok(request) => Some(request),
            Err(sync_mpsc::RecvTimeoutError::Timeout) => None,
            Err(sync_mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let mut group: Vec<Pending> = Vec::new();
        let mut erasures = Vec::new();
        let mut group_bytes = 0_u64;
        let mut next = first;
        while let Some(request) = next.take() {
            match request {
                WriterRequest::Append {
                    batch,
                    encoded_bytes,
                    _queue_bytes,
                    ack,
                } => {
                    let unpublished = stats
                        .sealed_bytes
                        .load(Ordering::Relaxed)
                        .saturating_add(wal.active_bytes());
                    let result = if unpublished.saturating_add(encoded_bytes)
                        > MAX_UNPUBLISHED_WAL_BYTES
                    {
                        Err(SinkError::Retryable)
                    } else {
                        wal.write(&batch).map(|_| ()).map_err(map_wal_error)
                    };
                    group_bytes = group_bytes.saturating_add(encoded_bytes);
                    group.push(Pending {
                        ack,
                        result,
                        events: batch.events.len() as u64,
                        received_ms: batch.received_at_ms,
                        _queue_bytes,
                    });
                }
                WriterRequest::Erase {
                    project_id,
                    person_id,
                    ack,
                } => {
                    erasures.push((project_id, person_id, ack));
                }
                WriterRequest::Shutdown { ack } => {
                    shutdown_ack = Some(ack);
                    break;
                }
            }
            if group.len() >= GROUP_MAX_BATCHES || group_bytes >= GROUP_MAX_BYTES {
                break;
            }
            next = receiver.try_recv().ok();
        }

        if !group.is_empty() {
            // One fsync makes the whole group durable; nobody is acked before.
            let synced = wal.sync().map_err(map_wal_error);
            for pending in group {
                let result = pending.result.and(synced);
                if result.is_ok() {
                    stats.note_acked(pending.received_ms, pending.events);
                }
                let _ = pending.ack.send(result);
            }
            stats
                .active_bytes
                .store(wal.active_bytes(), Ordering::Relaxed);
        }

        if shutdown_ack.is_some() {
            break;
        }
        if !erasures.is_empty() || last_seal.elapsed() >= SEAL_INTERVAL {
            last_seal = Instant::now();
            if wal.active_bytes() > 0 {
                let before = wal.active_bytes();
                match wal.seal() {
                    Ok(next) => {
                        stats.sealed_bytes.fetch_add(before, Ordering::Relaxed);
                        stats.active_bytes.store(0, Ordering::Relaxed);
                        stats.note_sealed(next.segment.saturating_sub(1));
                        let _ = publisher.try_send(PublisherSignal::Sealed);
                    }
                    Err(error) => tracing::error!(%error, "sealing the WAL segment failed"),
                }
            }
        }
        for (project_id, person_id, ack) in erasures {
            if let Err(sync_mpsc::TrySendError::Full(PublisherSignal::Erase { ack, .. })
            | sync_mpsc::TrySendError::Disconnected(PublisherSignal::Erase { ack, .. })) =
                publisher.try_send(PublisherSignal::Erase {
                    project_id,
                    person_id,
                    ack,
                })
            {
                let _ = ack.send(Err(DurablePipelineError::WorkerPanicked));
            }
        }
    }

    // Drain: seal what was written, then let the publisher finish.
    let sealed = wal
        .seal()
        .map(|next| stats.note_sealed(next.segment.saturating_sub(1)))
        .map_err(DurablePipelineError::from);
    let (done_tx, done_rx) = sync_mpsc::sync_channel(1);
    let published = if publisher
        .send(PublisherSignal::Shutdown { ack: done_tx })
        .is_ok()
    {
        done_rx
            .recv()
            .unwrap_or(Err(DurablePipelineError::WorkerPanicked))
    } else {
        Err(DurablePipelineError::WorkerPanicked)
    };
    if let Some(ack) = shutdown_ack {
        let _ = ack.send(sealed.and(published));
    }
}

fn publisher_loop(
    publisher: Publisher,
    compactor: Compactor,
    signals: sync_mpsc::Receiver<PublisherSignal>,
    stats: Arc<PipelineStats>,
) {
    let mut last_maintenance = Instant::now();
    loop {
        let shutdown = match signals.recv_timeout(PUBLISHER_TICK) {
            Ok(PublisherSignal::Sealed) | Err(sync_mpsc::RecvTimeoutError::Timeout) => None,
            Ok(PublisherSignal::Erase {
                project_id,
                person_id,
                ack,
            }) => {
                let result = publish_and_account(&publisher, &stats)
                    .and_then(|()| erase(&publisher, &compactor, &project_id, &person_id));
                let _ = ack.send(result);
                continue;
            }
            Ok(PublisherSignal::Shutdown { ack }) => Some(ack),
            Err(sync_mpsc::RecvTimeoutError::Disconnected) => return,
        };

        let published = publish_and_account(&publisher, &stats);
        if let Some(ack) = shutdown {
            let _ = ack.send(published);
            return;
        }
        if let Err(error) = published {
            tracing::error!(%error, "event publication failed; the WAL keeps the events and will retry");
            continue;
        }

        // Maintenance between publications: bounded steps, freshness first.
        if last_maintenance.elapsed() >= PUBLISHER_TICK {
            last_maintenance = Instant::now();
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                match compactor.step() {
                    Ok(Some(done)) => tracing::debug!(
                        project = %done.project_id,
                        day = %done.day,
                        files = done.input_files,
                        rows = done.output_rows,
                        "compacted partition"
                    ),
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(%error, "compaction failed; it will be retried");
                        break;
                    }
                }
            }
            publisher.lake().sweep_graveyard();
        }
    }
}

fn erase(
    publisher: &Publisher,
    compactor: &Compactor,
    project_id: &str,
    person_id: &str,
) -> Result<ErasureReport, DurablePipelineError> {
    let distinct_ids = {
        let mut connection = publisher.lake().lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| DurablePipelineError::Lake(error.into()))?;
        let ids = crate::projections::erase_person(&transaction, project_id, person_id)
            .map_err(|error| DurablePipelineError::Publish(error.into()))?;
        transaction
            .commit()
            .map_err(|error| DurablePipelineError::Lake(error.into()))?;
        ids
    };
    let events = compactor.erase(project_id, &distinct_ids)?;
    tracing::info!(
        distinct_ids = distinct_ids.len(),
        events,
        "erased a person and their events"
    );
    Ok(ErasureReport {
        distinct_ids: distinct_ids.len(),
        events,
    })
}

fn publish_and_account(
    publisher: &Publisher,
    stats: &PipelineStats,
) -> Result<(), DurablePipelineError> {
    let started = Instant::now();
    let published = publisher.publish_all()?;
    let events: usize = published.iter().map(|p| p.events).sum();
    if events > 0 {
        let elapsed = started.elapsed();
        let files: usize = published.iter().map(|p| p.files).sum();
        if elapsed > Duration::from_secs(1) {
            tracing::info!(events, files, elapsed_ms = elapsed.as_millis() as u64, "slow publication");
        } else {
            tracing::debug!(events, files, elapsed_ms = elapsed.as_millis() as u64, "published");
        }
    }
    let checkpoint = publisher.checkpoint()?;
    stats.sealed_bytes.store(
        publisher.wal_reader().sealed_bytes().unwrap_or(0),
        Ordering::Relaxed,
    );
    stats.note_published(events as u64, checkpoint.segment);
    Ok(())
}

fn map_wal_error(error: WalError) -> SinkError {
    match error {
        WalError::EmptyBatch | WalError::InvalidProjectBinding | WalError::RecordTooLarge { .. } => {
            SinkError::Fatal
        }
        _ => SinkError::Retryable,
    }
}

/// For callers that only need the WAL directory path rules.
pub fn wal_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("wal")
}
