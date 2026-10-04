//! Keeps partitions few-filed and duplicate-free.
//!
//! Publication writes one small file per touched partition per window. The
//! compactor merges a partition's small files into one, sorted by
//! `(event, timestamp)` so DuckDB's row-group statistics prune event filters,
//! and drops SDK-retry duplicates (same `uuid`) on the way — the lake's
//! equivalent of a ReplacingMergeTree merge. It also enforces retention.
//!
//! Each step does bounded work on one partition and commits a new generation;
//! replaced files are retired, not deleted, until no lease can read them.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use rusqlite::{TransactionBehavior, params};

use super::{
    Lake, LakeError, LakeFile, NewFile, Partition, advance_state, insert_file, read_state,
    sync_dir,
};

/// Today's partition is merged once it has this many small files; older
/// partitions as soon as they have two.
pub const ACTIVE_PARTITION_MERGE_THRESHOLD: usize = 12;
/// A file at least this large is "big" and only merged with others when the
/// partition has too many big files.
pub const SMALL_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Bound on one merge's input.
pub const MAX_MERGE_FILES: usize = 64;
pub const MAX_MERGE_BYTES: u64 = 1024 * 1024 * 1024;
/// DuckDB memory for compaction; it spills to `tmp/` beyond this.
pub const COMPACTION_MEMORY: &str = "192MB";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compacted {
    pub project_id: String,
    pub day: NaiveDate,
    pub input_files: usize,
    pub input_rows: u64,
    pub output_rows: u64,
}

pub struct Compactor {
    lake: Arc<Lake>,
    duck: duckdb::Connection,
    retention_days: Option<u32>,
}

impl Compactor {
    pub fn new(lake: Arc<Lake>, tmp_dir: &Path, retention_days: Option<u32>) -> Result<Self, LakeError> {
        std::fs::create_dir_all(tmp_dir).map_err(super::io_error(tmp_dir))?;
        // No extension may be fetched or loaded (a static binary cannot).
        let duck = duckdb::Connection::open_in_memory_with_flags(
            duckdb::Config::default()
                .enable_autoload_extension(false)
                .map_err(duck_error)?,
        )
        .map_err(duck_error)?;
        duck.execute_batch(&format!(
            "SET autoinstall_known_extensions = false;
             SET memory_limit='{COMPACTION_MEMORY}'; SET threads=1;
             SET preserve_insertion_order=false; SET temp_directory='{}';",
            tmp_dir.display().to_string().replace('\'', "''")
        ))
        .map_err(duck_error)?;
        Ok(Self {
            lake,
            duck,
            retention_days,
        })
    }

    /// Do one bounded unit of work. `Ok(None)` when nothing needs doing.
    pub fn step(&self) -> Result<Option<Compacted>, LakeError> {
        if let Some(days) = self.retention_days
            && self.enforce_retention(days)? > 0
        {
            return Ok(None);
        }
        let today = Utc::now().date_naive();
        let Some((partition, inputs)) = self
            .lake
            .partitions()
            .into_iter()
            .filter_map(|partition| {
                let inputs = merge_inputs(&partition, today);
                (inputs.len() >= 2).then_some((partition, inputs))
            })
            .max_by_key(|(_, inputs)| inputs.len())
        else {
            return Ok(None);
        };
        self.merge(&partition, inputs).map(Some)
    }

    fn merge(&self, partition: &Partition, inputs: Vec<Arc<LakeFile>>) -> Result<Compacted, LakeError> {
        let dir = self.lake.partition_dir(&partition.project_id, partition.day)?;
        let generation = self.lake.generation() + 1;
        let path = dir.join(format!("c{generation:012}.parquet"));
        let temporary = path.with_extension("parquet.tmp");
        let sources = inputs
            .iter()
            .map(|file| sql_string(&file.path))
            .collect::<Vec<_>>()
            .join(", ");
        // `uuid` duplicates are SDK retries of one event; keep the first copy.
        let copied = self
            .duck
            .execute(
                &merge_sql(&sources, &sql_string(&temporary)),
                [],
            )
            .map_err(duck_error)?;
        crate::fault::hit("compact.after_tmp");
        let file = std::fs::File::open(&temporary).map_err(super::io_error(&temporary))?;
        file.sync_all().map_err(super::io_error(&temporary))?;
        let bytes = file.metadata().map_err(super::io_error(&temporary))?.len();
        std::fs::rename(&temporary, &path).map_err(super::io_error(&path))?;
        sync_dir(&dir)?;
        crate::fault::hit("compact.after_rename");

        let input_rows = inputs.iter().map(|file| file.rows).sum();
        let retired: HashSet<i64> = inputs.iter().map(|file| file.id).collect();
        let now = Utc::now().timestamp();
        let committed = (|| -> Result<LakeFile, LakeError> {
            let mut connection = self.lake.lock_connection()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let state = read_state(&transaction)?;
            if state.generation + 1 != generation {
                return Err(LakeError::Corrupt(
                    "lake generation moved during compaction".to_owned(),
                ));
            }
            let added = insert_file(
                &transaction,
                &self.lake,
                NewFile {
                    project_id: &partition.project_id,
                    day: partition.day,
                    path: &path,
                    rows: copied as u64,
                    bytes,
                    level: 1,
                },
                generation,
                now,
            )?;
            retire(&transaction, &retired, generation)?;
            advance_state(&transaction, &state, None, now)?;
            transaction.commit()?;
            crate::fault::hit("compact.after_commit");
            Ok(added)
        })();
        let added = match committed {
            Ok(added) => added,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        self.lake.activate(generation, vec![added], &retired);
        crate::fault::hit("compact.before_sweep");
        Ok(Compacted {
            project_id: partition.project_id.clone(),
            day: partition.day,
            input_files: inputs.len(),
            input_rows,
            output_rows: copied as u64,
        })
    }

    /// Retire every partition older than `days` in one generation.
    fn enforce_retention(&self, days: u32) -> Result<usize, LakeError> {
        let cutoff = Utc::now().date_naive() - chrono::Duration::days(i64::from(days));
        let retired: HashSet<i64> = self
            .lake
            .partitions()
            .into_iter()
            .filter(|partition| partition.day < cutoff)
            .flat_map(|partition| partition.files.into_iter().map(|file| file.id))
            .collect();
        if retired.is_empty() {
            return Ok(0);
        }
        let mut connection = self.lake.lock_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = read_state(&transaction)?;
        let generation = state.generation + 1;
        retire(&transaction, &retired, generation)?;
        advance_state(&transaction, &state, None, Utc::now().timestamp())?;
        transaction.commit()?;
        drop(connection);
        self.lake.activate(generation, Vec::new(), &retired);
        tracing::info!(files = retired.len(), %cutoff, "retention removed expired event files");
        Ok(retired.len())
    }
}

impl Compactor {
    /// Physically remove every event of `distinct_ids` from a project's
    /// files. Each affected partition is rewritten without them and swapped
    /// in as a new generation; the old files are retired like compaction
    /// inputs. Returns the number of events removed.
    pub fn erase(&self, project_id: &str, distinct_ids: &[String]) -> Result<u64, LakeError> {
        if distinct_ids.is_empty() {
            return Ok(0);
        }
        let ids = distinct_ids
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let mut removed = 0_u64;
        for partition in self
            .lake
            .partitions()
            .into_iter()
            .filter(|partition| partition.project_id == project_id)
        {
            let sources = partition
                .files
                .iter()
                .map(|file| sql_string(&file.path))
                .collect::<Vec<_>>()
                .join(", ");
            let matching: i64 = self
                .duck
                .query_row(
                    &format!(
                        "SELECT count(*) FROM read_parquet([{sources}], union_by_name = true)
                         WHERE distinct_id IN ({ids})"
                    ),
                    [],
                    |row| row.get(0),
                )
                .map_err(duck_error)?;
            if matching == 0 {
                continue;
            }
            let dir = self.lake.partition_dir(project_id, partition.day)?;
            let generation = self.lake.generation() + 1;
            let path = dir.join(format!("e{generation:012}.parquet"));
            let temporary = path.with_extension("parquet.tmp");
            let kept = self
                .duck
                .execute(
                    &format!(
                        "COPY (
                            SELECT * FROM read_parquet([{sources}], union_by_name = true)
                            WHERE distinct_id NOT IN ({ids})
                            QUALIFY row_number() OVER (PARTITION BY uuid ORDER BY timestamp) = 1
                            ORDER BY event, timestamp
                         ) TO {} (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 122880)",
                        sql_string(&temporary)
                    ),
                    [],
                )
                .map_err(duck_error)? as u64;
            crate::fault::hit("erase.after_tmp");
            let file = std::fs::File::open(&temporary).map_err(super::io_error(&temporary))?;
            file.sync_all().map_err(super::io_error(&temporary))?;
            let bytes = file.metadata().map_err(super::io_error(&temporary))?.len();
            std::fs::rename(&temporary, &path).map_err(super::io_error(&path))?;
            sync_dir(&dir)?;
            crate::fault::hit("erase.after_rename");

            let retired: HashSet<i64> = partition.files.iter().map(|file| file.id).collect();
            let now = Utc::now().timestamp();
            let mut connection = self.lake.lock_connection()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let state = read_state(&transaction)?;
            if state.generation + 1 != generation {
                return Err(LakeError::Corrupt(
                    "lake generation moved during erasure".to_owned(),
                ));
            }
            let added = (kept > 0)
                .then(|| {
                    insert_file(
                        &transaction,
                        &self.lake,
                        NewFile {
                            project_id,
                            day: partition.day,
                            path: &path,
                            rows: kept,
                            bytes,
                            level: 1,
                        },
                        generation,
                        now,
                    )
                })
                .transpose()?;
            retire(&transaction, &retired, generation)?;
            advance_state(&transaction, &state, None, now)?;
            transaction.commit()?;
            crate::fault::hit("erase.after_commit");
            drop(connection);
            if added.is_none() {
                let _ = std::fs::remove_file(&path);
            }
            self.lake
                .activate(generation, added.into_iter().collect(), &retired);
            removed += matching as u64;
        }
        Ok(removed)
    }
}

/// Files of one partition worth merging now, bounded.
fn merge_inputs(partition: &Partition, today: NaiveDate) -> Vec<Arc<LakeFile>> {
    let mut small: Vec<Arc<LakeFile>> = partition
        .files
        .iter()
        .filter(|file| file.bytes < SMALL_FILE_BYTES)
        .cloned()
        .collect();
    let fresh = small.iter().filter(|file| file.level == 0).count();
    let due = if partition.day >= today {
        fresh >= ACTIVE_PARTITION_MERGE_THRESHOLD
    } else {
        fresh >= 1 && small.len() >= 2
    };
    if !due {
        return Vec::new();
    }
    small.sort_by_key(|file| file.id);
    let mut bytes = 0_u64;
    small
        .into_iter()
        .take_while(|file| {
            bytes = bytes.saturating_add(file.bytes);
            bytes <= MAX_MERGE_BYTES
        })
        .take(MAX_MERGE_FILES)
        .collect()
}

fn retire(
    transaction: &rusqlite::Transaction<'_>,
    ids: &HashSet<i64>,
    generation: u64,
) -> Result<(), LakeError> {
    let mut statement = transaction.prepare(
        "UPDATE lake_files SET retired_generation = ?1
         WHERE id = ?2 AND retired_generation IS NULL",
    )?;
    for id in ids {
        if statement.execute(params![generation as i64, id])? != 1 {
            return Err(LakeError::Corrupt(format!(
                "event file {id} was already retired"
            )));
        }
    }
    Ok(())
}

/// The statement that merges `sources` (a quoted, comma-separated file list)
/// into one compacted file `destination` (a quoted path): duplicate-free,
/// sorted by `(event, timestamp)`, zstd, fixed-size row groups. The query
/// benchmark builds its "compacted" dataset with this same statement.
pub(crate) fn merge_sql(sources: &str, destination: &str) -> String {
    format!(
        "COPY (
            SELECT * FROM read_parquet([{sources}], union_by_name = true)
            QUALIFY row_number() OVER (PARTITION BY uuid ORDER BY timestamp) = 1
            ORDER BY event, timestamp
         ) TO {destination} (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 122880)"
    )
}

fn sql_string(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn duck_error(error: duckdb::Error) -> LakeError {
    LakeError::Corrupt(format!("duckdb: {error}"))
}
