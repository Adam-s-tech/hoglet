//! The event lake: immutable Parquet files, catalogued in `projections.db`.
//!
//! Every file belongs to one `(project, day)` partition. A file row records
//! the generation that created it and, once compaction or retention replaces
//! it, the generation that retired it. Publication and compaction therefore
//! cost O(files touched), never O(files in the lake).
//!
//! Readers take a [`Lease`]: an `Arc` snapshot of the files they will read.
//! Retired files move to a graveyard and are unlinked only once no lease can
//! still name them, so a query can never race the deletion of its input.

pub mod compactor;
pub mod parquet;
pub mod publisher;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use chrono::NaiveDate;
use rusqlite::{Connection, OpenFlags, params};

use crate::pipeline::wal::WalCursor;

/// Lake catalog tables. Installed by storage bootstrap; idempotent.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projection_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    wal_segment INTEGER NOT NULL CHECK (wal_segment >= 1),
    wal_offset INTEGER NOT NULL CHECK (wal_offset >= 0),
    identity_seq INTEGER NOT NULL DEFAULT 0 CHECK (identity_seq >= 0),
    identity_epoch INTEGER NOT NULL DEFAULT 0 CHECK (identity_epoch >= 0),
    updated_at INTEGER NOT NULL
);
INSERT OR IGNORE INTO projection_state
    (singleton, generation, wal_segment, wal_offset, updated_at)
VALUES (1, 0, 1, 0, 0);
CREATE TABLE IF NOT EXISTS lake_files (
    id INTEGER PRIMARY KEY,
    project_id TEXT NOT NULL CHECK (length(project_id) > 0),
    day TEXT NOT NULL CHECK (length(day) = 10),
    relative_path TEXT NOT NULL UNIQUE,
    rows INTEGER NOT NULL CHECK (rows >= 0),
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    level INTEGER NOT NULL CHECK (level IN (0, 1)),
    created_generation INTEGER NOT NULL,
    retired_generation INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS lake_files_live
    ON lake_files(project_id, day) WHERE retired_generation IS NULL;
";

/// One immutable Parquet file.
#[derive(Debug)]
pub struct LakeFile {
    pub id: i64,
    pub project_id: String,
    pub day: NaiveDate,
    pub path: PathBuf,
    pub rows: u64,
    pub bytes: u64,
    pub level: u8,
    pub created_at: i64,
}

#[derive(Debug, Default)]
struct ProjectFiles {
    /// Generation of the last change to this project's files. Query caches
    /// key on this, so one project's ingest never invalidates another's.
    version: u64,
    days: BTreeMap<NaiveDate, Vec<Arc<LakeFile>>>,
}

#[derive(Debug, Default)]
struct Snapshot {
    generation: u64,
    projects: HashMap<String, Arc<ProjectFiles>>,
}

/// A stable view of the files one query reads.
#[derive(Debug, Clone)]
pub struct Lease {
    files: Vec<Arc<LakeFile>>,
    version: u64,
}

impl Lease {
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    pub fn files(&self) -> &[Arc<LakeFile>] {
        &self.files
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Changes whenever the project's visible files change.
    pub fn data_version(&self) -> u64 {
        self.version
    }

    pub fn rows(&self) -> u64 {
        self.files.iter().map(|file| file.rows).sum()
    }
}

/// Durable pipeline position, read and written inside publication
/// transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionState {
    pub generation: u64,
    pub checkpoint: WalCursor,
    pub identity_seq: u64,
    pub identity_epoch: u64,
}

pub fn read_state(connection: &Connection) -> Result<ProjectionState, LakeError> {
    let (generation, segment, offset, seq, epoch): (i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT generation, wal_segment, wal_offset, identity_seq, identity_epoch
             FROM projection_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;
    let unsigned = |value: i64| {
        u64::try_from(value)
            .map_err(|_| LakeError::Corrupt(format!("negative projection state value {value}")))
    };
    Ok(ProjectionState {
        generation: unsigned(generation)?,
        checkpoint: WalCursor::new(unsigned(segment)?, unsigned(offset)?),
        identity_seq: unsigned(seq)?,
        identity_epoch: unsigned(epoch)?,
    })
}

#[derive(Debug)]
pub enum LakeError {
    Database(rusqlite::Error),
    Io { path: PathBuf, source: std::io::Error },
    Corrupt(String),
    Unavailable,
}

impl fmt::Display for LakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "lake catalog error: {error}"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Corrupt(message) => write!(formatter, "corrupt lake catalog: {message}"),
            Self::Unavailable => formatter.write_str("lake catalog unavailable"),
        }
    }
}

impl std::error::Error for LakeError {}

impl From<rusqlite::Error> for LakeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub(crate) fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> LakeError + '_ {
    move |source| LakeError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Summary of one project's stored events.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LakeStatus {
    pub files: usize,
    pub rows: u64,
    pub bytes: u64,
    pub first_day: Option<NaiveDate>,
    pub last_day: Option<NaiveDate>,
}

/// A partition with its live files, as seen by the compactor.
pub struct Partition {
    pub project_id: String,
    pub day: NaiveDate,
    pub files: Vec<Arc<LakeFile>>,
}

pub struct Lake {
    root: PathBuf,
    connection: Mutex<Connection>,
    current: RwLock<Arc<Snapshot>>,
    graveyard: Mutex<Vec<Arc<LakeFile>>>,
}

impl Lake {
    /// Open the catalog in an already-bootstrapped projections database and
    /// reconcile the event directory with it: files a crash left behind before
    /// their publication committed are deleted, as are retired files (no
    /// reader survives a restart).
    pub fn open(projections_db: &Path, root: &Path) -> Result<Self, LakeError> {
        std::fs::create_dir_all(root).map_err(io_error(root))?;
        let root = std::fs::canonicalize(root).map_err(io_error(root))?;
        let connection = Connection::open_with_flags(
            projections_db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        connection.execute_batch(SCHEMA)?;
        crate::projections::initialize_schema(&connection)
            .map_err(|error| LakeError::Corrupt(error.to_string()))?;

        let retired: Vec<String> = connection
            .prepare("SELECT relative_path FROM lake_files WHERE retired_generation IS NOT NULL")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        for relative in &retired {
            remove_if_exists(&root.join(relative))?;
        }
        connection.execute(
            "DELETE FROM lake_files WHERE retired_generation IS NOT NULL",
            [],
        )?;

        let state = read_state(&connection)?;
        let mut snapshot = Snapshot {
            generation: state.generation,
            projects: HashMap::new(),
        };
        let mut live_paths = HashSet::new();
        {
            let mut statement = connection.prepare(
                "SELECT id, project_id, day, relative_path, rows, bytes, level,
                        created_generation, created_at
                 FROM lake_files ORDER BY id",
            )?;
            let mut rows = statement.query([])?;
            let mut grouped: HashMap<String, ProjectFiles> = HashMap::new();
            while let Some(row) = rows.next()? {
                let relative: String = row.get(3)?;
                let day: String = row.get(2)?;
                let day = NaiveDate::parse_from_str(&day, "%Y-%m-%d")
                    .map_err(|_| LakeError::Corrupt(format!("bad partition day {day}")))?;
                let path = root.join(&relative);
                live_paths.insert(path.clone());
                let file = Arc::new(LakeFile {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    day,
                    path,
                    rows: row.get::<_, i64>(4)? as u64,
                    bytes: row.get::<_, i64>(5)? as u64,
                    level: row.get::<_, i64>(6)? as u8,
                    created_at: row.get(8)?,
                });
                let created: i64 = row.get(7)?;
                let project = grouped.entry(file.project_id.clone()).or_default();
                project.version = project.version.max(created as u64);
                project.days.entry(day).or_default().push(file);
            }
            snapshot.projects = grouped
                .into_iter()
                .map(|(project, files)| (project, Arc::new(files)))
                .collect();
        }
        for path in &live_paths {
            if !path.is_file() {
                return Err(LakeError::Corrupt(format!(
                    "catalogued event file is missing: {}",
                    path.display()
                )));
            }
        }
        remove_orphans(&root, &live_paths)?;

        Ok(Self {
            root,
            connection: Mutex::new(connection),
            current: RwLock::new(Arc::new(snapshot)),
            graveyard: Mutex::new(Vec::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Files of `project_id` for days in `[from, to_exclusive)`.
    pub fn lease(&self, project_id: &str, from: NaiveDate, to_exclusive: NaiveDate) -> Lease {
        let snapshot = self.snapshot();
        match snapshot.projects.get(project_id) {
            Some(project) => Lease {
                files: project
                    .days
                    .range(from..to_exclusive)
                    .flat_map(|(_, files)| files.iter().cloned())
                    .collect(),
                version: project.version,
            },
            None => Lease {
                files: Vec::new(),
                version: 0,
            },
        }
    }

    /// Every file of one project.
    pub fn lease_all(&self, project_id: &str) -> Lease {
        self.lease(project_id, NaiveDate::MIN, NaiveDate::MAX)
    }

    pub fn generation(&self) -> u64 {
        self.snapshot().generation
    }

    pub fn status(&self, project_id: &str) -> LakeStatus {
        let snapshot = self.snapshot();
        let Some(project) = snapshot.projects.get(project_id) else {
            return LakeStatus::default();
        };
        let mut status = LakeStatus {
            first_day: project.days.keys().next().copied(),
            last_day: project.days.keys().next_back().copied(),
            ..LakeStatus::default()
        };
        for file in project.days.values().flatten() {
            status.files += 1;
            status.rows += file.rows;
            status.bytes += file.bytes;
        }
        status
    }

    /// Every live partition, for compaction and retention planning.
    pub fn partitions(&self) -> Vec<Partition> {
        let snapshot = self.snapshot();
        let mut partitions = Vec::new();
        for (project_id, project) in &snapshot.projects {
            for (day, files) in &project.days {
                partitions.push(Partition {
                    project_id: project_id.clone(),
                    day: *day,
                    files: files.clone(),
                });
            }
        }
        partitions.sort_by(|a, b| (&a.project_id, a.day).cmp(&(&b.project_id, b.day)));
        partitions
    }

    /// Directory for one partition, created durably on first use.
    pub(crate) fn partition_dir(
        &self,
        project_id: &str,
        day: NaiveDate,
    ) -> Result<PathBuf, LakeError> {
        let project = self.root.join(project_dir_name(project_id));
        create_dir_durable(&project)?;
        let dir = project.join(day.format("%Y-%m-%d").to_string());
        create_dir_durable(&dir)?;
        Ok(dir)
    }

    pub(crate) fn relative(&self, path: &Path) -> Result<String, LakeError> {
        path.strip_prefix(&self.root)
            .map(|relative| relative.to_string_lossy().into_owned())
            .map_err(|_| LakeError::Corrupt(format!("{} is outside the lake", path.display())))
    }

    pub(crate) fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, LakeError> {
        self.connection.lock().map_err(|_| LakeError::Unavailable)
    }

    /// Install a committed generation: add new files, retire replaced ones.
    /// Retired files go to the graveyard; old leases keep them alive.
    pub(crate) fn activate(&self, generation: u64, added: Vec<LakeFile>, retired: &HashSet<i64>) {
        let mut current = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut projects = current.projects.clone();
        let mut buried = Vec::new();
        let touched: HashSet<String> = added
            .iter()
            .map(|file| file.project_id.clone())
            .chain(
                projects
                    .iter()
                    .filter(|(_, project)| {
                        project
                            .days
                            .values()
                            .flatten()
                            .any(|file| retired.contains(&file.id))
                    })
                    .map(|(project, _)| project.clone()),
            )
            .collect();
        for project_id in touched {
            let old = projects.get(&project_id).cloned().unwrap_or_default();
            let mut days = BTreeMap::new();
            for (day, files) in &old.days {
                let mut kept = Vec::with_capacity(files.len());
                for file in files {
                    if retired.contains(&file.id) {
                        buried.push(file.clone());
                    } else {
                        kept.push(file.clone());
                    }
                }
                if !kept.is_empty() {
                    days.insert(*day, kept);
                }
            }
            for file in added.iter().filter(|file| file.project_id == project_id) {
                days.entry(file.day).or_insert_with(Vec::new);
            }
            projects.insert(
                project_id,
                Arc::new(ProjectFiles {
                    version: generation,
                    days,
                }),
            );
        }
        for file in added {
            let project = projects
                .get_mut(&file.project_id)
                .expect("touched project was inserted above");
            Arc::get_mut(project)
                .expect("freshly built project files are uniquely owned")
                .days
                .entry(file.day)
                .or_default()
                .push(Arc::new(file));
        }
        projects.retain(|_, project| !project.days.is_empty());
        *current = Arc::new(Snapshot {
            generation,
            projects,
        });
        drop(current);
        self.graveyard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(buried);
    }

    /// Unlink retired files no lease can still read. Returns how many were
    /// removed. Failures leave the file buried for the next sweep.
    pub fn sweep_graveyard(&self) -> usize {
        let mut graveyard = self
            .graveyard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut removed = Vec::new();
        graveyard.retain(|file| {
            if Arc::strong_count(file) > 1 {
                return true;
            }
            match remove_if_exists(&file.path) {
                Ok(()) => {
                    crate::fault::hit("lake.sweep.after_unlink");
                    removed.push(file.id);
                    false
                }
                Err(error) => {
                    tracing::warn!(%error, "retired event file could not be removed yet");
                    true
                }
            }
        });
        drop(graveyard);
        if removed.is_empty() {
            return 0;
        }
        if let Ok(connection) = self.lock_connection() {
            for id in &removed {
                if let Err(error) = connection.execute("DELETE FROM lake_files WHERE id = ?1", [id])
                {
                    tracing::warn!(%error, "retired event file row could not be removed");
                }
            }
        }
        removed.len()
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// A file about to be catalogued.
pub(crate) struct NewFile<'a> {
    pub project_id: &'a str,
    pub day: NaiveDate,
    pub path: &'a Path,
    pub rows: u64,
    pub bytes: u64,
    pub level: u8,
}

/// Persist a new file row inside the caller's transaction.
pub(crate) fn insert_file(
    transaction: &rusqlite::Transaction<'_>,
    lake: &Lake,
    file: NewFile<'_>,
    generation: u64,
    now: i64,
) -> Result<LakeFile, LakeError> {
    let NewFile {
        project_id,
        day,
        path,
        rows,
        bytes,
        level,
    } = file;
    let relative = lake.relative(path)?;
    transaction.execute(
        "INSERT INTO lake_files
            (project_id, day, relative_path, rows, bytes, level, created_generation, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            project_id,
            day.format("%Y-%m-%d").to_string(),
            relative,
            rows as i64,
            bytes as i64,
            level,
            generation as i64,
            now,
        ],
    )?;
    Ok(LakeFile {
        id: transaction.last_insert_rowid(),
        project_id: project_id.to_owned(),
        day,
        path: path.to_path_buf(),
        rows,
        bytes,
        level,
        created_at: now,
    })
}

/// Advance the generation (and optionally the WAL checkpoint) inside the
/// caller's transaction, refusing to proceed if another writer moved it.
pub(crate) fn advance_state(
    transaction: &rusqlite::Transaction<'_>,
    expected: &ProjectionState,
    checkpoint: Option<WalCursor>,
    now: i64,
) -> Result<u64, LakeError> {
    let next = expected.generation + 1;
    let checkpoint = checkpoint.unwrap_or(expected.checkpoint);
    let updated = transaction.execute(
        "UPDATE projection_state
         SET generation = ?1, wal_segment = ?2, wal_offset = ?3, updated_at = ?4
         WHERE singleton = 1 AND generation = ?5",
        params![
            next as i64,
            checkpoint.segment as i64,
            checkpoint.byte_offset as i64,
            now,
            expected.generation as i64,
        ],
    )?;
    if updated != 1 {
        return Err(LakeError::Corrupt(
            "projection state changed underneath its single writer".to_owned(),
        ));
    }
    Ok(next)
}

/// Project ids are UUIDs; anything else is hex-encoded so it can never escape
/// the lake directory.
fn project_dir_name(project_id: &str) -> String {
    if !project_id.is_empty()
        && project_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        project_id.to_owned()
    } else {
        format!("x{}", hex::encode(project_id))
    }
}

fn create_dir_durable(path: &Path) -> Result<(), LakeError> {
    match std::fs::create_dir(path) {
        Ok(()) => {
            sync_dir(path)?;
            if let Some(parent) = path.parent() {
                sync_dir(parent)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(source) => Err(LakeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(crate) fn sync_dir(path: &Path) -> Result<(), LakeError> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error(path))
}

fn remove_if_exists(path: &Path) -> Result<(), LakeError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(LakeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Delete Parquet and temporary files the catalog does not know about. They
/// can only be leftovers of a publication or compaction that never committed.
fn remove_orphans(root: &Path, live: &HashSet<PathBuf>) -> Result<(), LakeError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(io_error(&dir))? {
            let entry = entry.map_err(io_error(&dir))?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(io_error(&path))?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && !live.contains(&path) {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                if name.ends_with(".parquet") || name.ends_with(".tmp") {
                    tracing::info!(path = %path.display(), "removing uncommitted event file");
                    remove_if_exists(&path)?;
                }
            }
        }
    }
    Ok(())
}
