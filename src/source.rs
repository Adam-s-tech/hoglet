//! Where readers get event files from.
//!
//! Query code never discovers Parquet files itself: it asks an
//! [`EventSource`] for one project's files over a day range and keeps the
//! returned [`EventFiles`] alive for the duration of the read. In production
//! the source is the lake catalog, whose guard stops compaction from deleting
//! files under a running query. [`DirectorySource`] serves tests and tools.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;

/// Files for one read. Hold it until the read finishes.
pub struct EventFiles {
    pub paths: Vec<PathBuf>,
    /// Changes whenever this project's files change. Cache keys use it.
    pub data_version: u64,
    _guard: Option<Arc<dyn Any + Send + Sync>>,
}

impl EventFiles {
    pub fn new(paths: Vec<PathBuf>, data_version: u64) -> Self {
        Self {
            paths,
            data_version,
            _guard: None,
        }
    }

    pub fn with_guard(mut self, guard: Arc<dyn Any + Send + Sync>) -> Self {
        self._guard = Some(guard);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

pub trait EventSource: Send + Sync {
    /// Files of `project_id` holding events whose UTC day is in
    /// `[from, to_exclusive)`.
    fn files(&self, project_id: &str, from: NaiveDate, to_exclusive: NaiveDate) -> EventFiles;
}

impl EventSource for crate::lake::Lake {
    fn files(&self, project_id: &str, from: NaiveDate, to_exclusive: NaiveDate) -> EventFiles {
        let lease = self.lease(project_id, from, to_exclusive);
        EventFiles::new(lease.paths(), lease.data_version()).with_guard(Arc::new(lease))
    }
}

/// Reads `root/<project_id>/<YYYY-MM-DD>/*.parquet` straight from disk — the
/// lake's layout without its catalog. For tests and offline tools only.
pub struct DirectorySource {
    root: PathBuf,
}

impl DirectorySource {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Directory a test should write a partition's files into.
    pub fn partition_dir(&self, project_id: &str, day: NaiveDate) -> PathBuf {
        self.root
            .join(project_id)
            .join(day.format("%Y-%m-%d").to_string())
    }
}

impl EventSource for DirectorySource {
    fn files(&self, project_id: &str, from: NaiveDate, to_exclusive: NaiveDate) -> EventFiles {
        let mut paths = Vec::new();
        let mut version = 0_u64;
        let Ok(days) = std::fs::read_dir(self.root.join(project_id)) else {
            return EventFiles::new(paths, version);
        };
        for day in days.flatten() {
            let name = day.file_name();
            let Some(date) = name
                .to_str()
                .and_then(|name| NaiveDate::parse_from_str(name, "%Y-%m-%d").ok())
            else {
                continue;
            };
            if date < from || date >= to_exclusive {
                continue;
            }
            let Ok(files) = std::fs::read_dir(day.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("parquet") {
                    if let Ok(metadata) = file.metadata() {
                        version = version
                            .wrapping_mul(31)
                            .wrapping_add(metadata.len())
                            .wrapping_add(
                                metadata
                                    .modified()
                                    .ok()
                                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|duration| duration.as_nanos() as u64)
                                    .unwrap_or(0),
                            );
                    }
                    paths.push(path);
                }
            }
        }
        paths.sort();
        EventFiles::new(paths, version)
    }
}
