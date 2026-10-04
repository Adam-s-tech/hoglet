//! Web analytics: the one-screen Plausible-grade view over `$pageview`s.
//!
//! Visitors are persons; sessions are `$session_id`s (events without one are
//! sessionized by 30 minutes of inactivity per person). A bounce is a session
//! with exactly one pageview and no other event.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::common::{Interval, PropertyFilter};

/// Query string of both web endpoints:
/// `date_from`, `date_to` (as `DateRange`), `interval`, and `properties`
/// (URL-encoded JSON array of `PropertyFilter`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct WebQuery {
    #[serde(default = "default_date_from")]
    pub date_from: String,
    #[serde(default)]
    pub date_to: Option<String>,
    #[serde(default)]
    pub interval: Option<Interval>,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
}

fn default_date_from() -> String {
    "-7d".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct WebMetric {
    pub value: f64,
    /// Same metric over the immediately preceding period of equal length.
    pub previous: Option<f64>,
}

/// `GET /api/projects/{project_id}/web/overview`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct WebOverview {
    pub visitors: WebMetric,
    pub pageviews: WebMetric,
    pub sessions: WebMetric,
    /// Percent, 0–100.
    pub bounce_rate: WebMetric,
    pub session_duration_s: WebMetric,
    pub interval: Interval,
    /// Bucket starts, RFC 3339.
    pub days: Vec<String>,
    #[ts(type = "Array<number>")]
    pub visitors_series: Vec<u64>,
    #[ts(type = "Array<number>")]
    pub pageviews_series: Vec<u64>,
    /// Persons with a pageview in the last 5 minutes.
    #[ts(type = "number")]
    pub live_visitors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum WebDimension {
    Page,
    EntryPage,
    ExitPage,
    ReferringDomain,
    UtmSource,
    UtmMedium,
    UtmCampaign,
    Browser,
    Os,
    DeviceType,
    Country,
}

/// `GET /api/projects/{project_id}/web/breakdown?dimension=&limit=` plus the
/// `WebQuery` fields.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct WebBreakdownRow {
    pub value: String,
    #[ts(type = "number")]
    pub visitors: u64,
    #[ts(type = "number")]
    pub views: u64,
    /// Percent; only for page-like dimensions.
    pub bounce_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct WebBreakdown {
    pub dimension: WebDimension,
    pub rows: Vec<WebBreakdownRow>,
}
