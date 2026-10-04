//! Turns sealed WAL records into queryable event files.
//!
//! One publication = one bounded window of sealed WAL records, applied in WAL
//! order inside a single `projections.db` transaction: identity and catalog
//! projections, one new Parquet file per touched `(project, day)` partition,
//! the file rows, and the new WAL checkpoint commit together. A crash anywhere
//! before the commit leaves only orphan files (removed on the next open) and
//! an unchanged checkpoint, so the same window is published again — exactly
//! once from the reader's point of view. WAL segments are reclaimed only after
//! the commit.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use rusqlite::TransactionBehavior;

use super::{
    Lake, LakeError, LakeFile, NewFile, advance_state, insert_file, read_state, sync_dir,
};
use crate::capture::event::CapturedEvent;
use crate::pipeline::wal::{WalCursor, WalError, WalReader};

/// Upper bound on WAL bytes one publication reads. Bounds memory: the window
/// is materialized as events before files are written.
pub const MAX_PUBLICATION_WAL_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum PublishError {
    Lake(LakeError),
    Wal(WalError),
    Projection(crate::projections::ProjectionError),
    MissingProjectBinding(uuid::Uuid),
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lake(error) => error.fmt(formatter),
            Self::Wal(error) => error.fmt(formatter),
            Self::Projection(error) => error.fmt(formatter),
            Self::MissingProjectBinding(uuid) => {
                write!(formatter, "event {uuid} has no authorized project binding")
            }
        }
    }
}

impl std::error::Error for PublishError {}

impl From<LakeError> for PublishError {
    fn from(error: LakeError) -> Self {
        Self::Lake(error)
    }
}
impl From<WalError> for PublishError {
    fn from(error: WalError) -> Self {
        Self::Wal(error)
    }
}
impl From<crate::projections::ProjectionError> for PublishError {
    fn from(error: crate::projections::ProjectionError) -> Self {
        Self::Projection(error)
    }
}
impl From<rusqlite::Error> for PublishError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Lake(LakeError::Database(error))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub generation: u64,
    pub checkpoint: WalCursor,
    pub events: usize,
    pub files: usize,
    /// Capture time of the oldest event in the window — how far behind
    /// queries were until this publication.
    pub oldest_event_ms: Option<i64>,
}

pub struct Publisher {
    lake: Arc<Lake>,
    wal: WalReader,
}

impl Publisher {
    pub fn new(lake: Arc<Lake>, wal: WalReader) -> Self {
        Self { lake, wal }
    }

    pub fn lake(&self) -> &Arc<Lake> {
        &self.lake
    }

    pub fn wal_reader(&self) -> &WalReader {
        &self.wal
    }

    pub fn checkpoint(&self) -> Result<WalCursor, PublishError> {
        let connection = self.lake.lock_connection()?;
        Ok(read_state(&connection)?.checkpoint)
    }

    /// Publish everything sealed so far, one bounded window at a time.
    pub fn publish_all(&self) -> Result<Vec<Published>, PublishError> {
        let mut published = Vec::new();
        while let Some(result) = self.publish_window()? {
            published.push(result);
        }
        Ok(published)
    }

    /// Publish at most one window. `None` when nothing sealed is pending.
    pub fn publish_window(&self) -> Result<Option<Published>, PublishError> {
        let mut connection = self.lake.lock_connection()?;
        let state = read_state(&connection)?;

        let mut window = self
            .wal
            .read_window(state.checkpoint, MAX_PUBLICATION_WAL_BYTES)?;
        let mut records = Vec::new();
        for record in window.by_ref() {
            records.push(record?);
        }
        let next_checkpoint = window.next_cursor();
        if records.is_empty() {
            drop(connection);
            // A checkpoint can move across empty segment boundaries.
            if next_checkpoint != state.checkpoint {
                self.wal.reclaim_through(state.checkpoint)?;
            }
            return Ok(None);
        }

        let now = Utc::now();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut partitions: BTreeMap<(String, NaiveDate), Vec<CapturedEvent>> = BTreeMap::new();
        let mut events = 0_usize;
        let mut oldest_event_ms: Option<i64> = None;
        // SDK retries of one event inside one window collapse here; across
        // windows the compactor removes them.
        let mut seen: HashSet<(String, uuid::Uuid)> = HashSet::new();
        for record in records {
            let project_ids = record
                .batch
                .project_ids()
                .ok_or(PublishError::MissingProjectBinding(record.batch.events[0].uuid))?;
            if record.batch.received_at_ms > 0 {
                let received = record.batch.received_at_ms;
                oldest_event_ms = Some(oldest_event_ms.map_or(received, |v| v.min(received)));
            }
            for (event, project_id) in record.batch.events.into_iter().zip(project_ids) {
                if !seen.insert((project_id.clone(), event.uuid)) {
                    continue;
                }
                crate::projections::apply_captured_event(
                    &transaction,
                    &project_id,
                    &event,
                    record.span.end,
                )?;
                events += 1;
                partitions
                    .entry((project_id, event.timestamp.date_naive()))
                    .or_default()
                    .push(event);
            }
        }

        let generation = state.generation + 1;
        let mut added: Vec<LakeFile> = Vec::with_capacity(partitions.len());
        let mut written: Vec<PathBuf> = Vec::with_capacity(partitions.len());
        let result = (|| -> Result<(), PublishError> {
            for (index, ((project_id, day), mut partition)) in partitions.into_iter().enumerate() {
                partition.sort_by(|a, b| (&a.event, a.timestamp).cmp(&(&b.event, b.timestamp)));
                let dir = self.lake.partition_dir(&project_id, day)?;
                let path = dir.join(format!("g{generation:012}-{index:04}.parquet"));
                let temporary = path.with_extension("parquet.tmp");
                let bytes = super::parquet::write_file(&partition, &temporary)
                    .map_err(super::io_error(&temporary))?;
                std::fs::rename(&temporary, &path).map_err(super::io_error(&path))?;
                sync_dir(&dir)?;
                written.push(path.clone());
                added.push(insert_file(
                    &transaction,
                    &self.lake,
                    NewFile {
                        project_id: &project_id,
                        day,
                        path: &path,
                        rows: partition.len() as u64,
                        bytes,
                        level: 0,
                    },
                    generation,
                    now.timestamp(),
                )?);
            }
            Ok(())
        })();
        if let Err(error) = result {
            drop(transaction);
            for path in written {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }

        let committed = advance_state(&transaction, &state, Some(next_checkpoint), now.timestamp())?;
        debug_assert_eq!(committed, generation);
        transaction.commit()?;
        drop(connection);

        let files = added.len();
        self.lake.activate(generation, added, &HashSet::new());
        self.wal.reclaim_through(next_checkpoint)?;
        Ok(Some(Published {
            generation,
            checkpoint: next_checkpoint,
            events,
            files,
            oldest_event_ms,
        }))
    }
}
