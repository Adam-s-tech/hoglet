//! Persons, the event feed, catalog, and project status.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A person as listed anywhere in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct PersonSummary {
    pub id: String,
    /// `email`, then `name`, then the first distinct id.
    pub display_name: String,
    /// Up to 10; the detail endpoint returns all.
    pub distinct_ids: Vec<String>,
    #[ts(type = "Record<string, unknown>")]
    pub properties: serde_json::Value,
    pub is_identified: bool,
    /// RFC 3339.
    pub created_at: String,
    pub last_seen: Option<String>,
}

/// `GET /api/projects/{project_id}/persons?search=&cursor=&limit=`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct PersonListResponse {
    pub persons: Vec<PersonSummary>,
    /// Opaque; pass back as `cursor` for the next page.
    pub next_cursor: Option<String>,
}

/// `GET /api/projects/{project_id}/persons/{person_id}`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct PersonDetail {
    pub person: PersonSummary,
    pub distinct_ids: Vec<String>,
    #[ts(type = "number")]
    pub event_count: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    #[ts(type = "number")]
    pub session_count: u64,
}

/// One stored event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct EventRow {
    pub uuid: String,
    pub event: String,
    pub distinct_id: String,
    pub person_id: String,
    /// RFC 3339.
    pub timestamp: String,
    #[ts(type = "Record<string, unknown>")]
    pub properties: serde_json::Value,
}

/// `GET /api/projects/{project_id}/events?event=&person_id=&before=&limit=`
///
/// Newest first. `before` is an RFC 3339 timestamp; pass `next_before` back
/// to page.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct EventListResponse {
    pub events: Vec<EventRow>,
    pub next_before: Option<String>,
}

/// `GET /api/projects/{project_id}/catalog/events?search=`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct CatalogEvent {
    pub name: String,
    #[ts(type = "number")]
    pub count: u64,
    pub last_seen: Option<String>,
}

/// `GET /api/projects/{project_id}/catalog/properties?type=event|person&search=`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct CatalogProperty {
    pub key: String,
    /// `"event"` or `"person"`.
    #[serde(rename = "type")]
    pub source: String,
    /// `"string"`, `"number"`, `"boolean"`, `"array"`, `"object"`.
    pub property_type: String,
    #[ts(type = "number")]
    pub count: u64,
}

/// `GET /api/projects/{project_id}/catalog/values?key=&type=&search=`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct CatalogValue {
    pub value: String,
    #[ts(type = "number")]
    pub count: u64,
}

/// `GET /api/projects/{project_id}/status` — the always-visible freshness
/// signal. If numbers are behind, the product says so.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct ProjectStatus {
    /// Any event ever stored for this project.
    pub has_events: bool,
    /// Newest stored event's capture time, RFC 3339.
    pub last_event_at: Option<String>,
    /// Seconds between the oldest acknowledged-but-unqueryable event and
    /// now; 0 when everything acknowledged is queryable.
    pub ingestion_lag_seconds: f64,
    #[ts(type = "number")]
    pub stored_events: u64,
    #[ts(type = "number")]
    pub stored_bytes: u64,
    pub first_day: Option<String>,
    pub last_day: Option<String>,
}
