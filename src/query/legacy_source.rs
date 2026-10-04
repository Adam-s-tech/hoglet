//! TEMPORARY: serves the query engine from the generation-based
//! `event_lake::VersionedEventLake` until the production composition
//! constructs `crate::lake::Lake` (which implements `EventSource`
//! directly). The pipeline owner deletes this module when that lands.
//!
//! Only project-scoped files are returned; `LegacyUnknown` files mix
//! projects and are never shown to a project-scoped reader.

use std::sync::Arc;

use chrono::NaiveDate;

use crate::event_lake::{EventScope, FileScope, VersionedEventLake};
use crate::source::{EventFiles, EventSource};

pub struct VersionedLakeSource {
    lake: Arc<VersionedEventLake>,
}

impl VersionedLakeSource {
    pub fn new(lake: Arc<VersionedEventLake>) -> Self {
        Self { lake }
    }
}

impl EventSource for VersionedLakeSource {
    fn files(&self, project_id: &str, from: NaiveDate, to_exclusive: NaiveDate) -> EventFiles {
        let generation = self.lake.current_generation_id().get();
        let Ok(scope) = EventScope::new(project_id, from, to_exclusive) else {
            return EventFiles::new(Vec::new(), generation);
        };
        let lease = self.lake.acquire(&scope);
        let paths = lease
            .files()
            .filter(|file| matches!(file.scope, FileScope::ProjectDate { .. }))
            .map(|file| file.path.clone())
            .collect();
        EventFiles::new(paths, lease.generation_id().get()).with_guard(Arc::new(lease))
    }
}
