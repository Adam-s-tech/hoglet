//! Insight queries and their results — the saved-insight format and the
//! `/api/projects/{project_id}/query` wire contract.
//!
//! Shapes follow PostHog's query schema (`TrendsQuery`, `FunnelsQuery`, …) so
//! that PostHog's query API can be matched later without a second model.
//! Additive changes only: new optional fields, new enum variants.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::common::{Breakdown, DateRange, Interval, PropertyFilter};

/// One event (or all events) with its own filters and aggregation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct EventNode {
    /// Event name; `null` means every event.
    #[serde(default)]
    pub event: Option<String>,
    /// Display label overriding the event name.
    #[serde(default)]
    pub custom_name: Option<String>,
    /// Filters that apply to this series/step only.
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
    #[serde(default)]
    pub math: Math,
    /// Event property aggregated by the property maths (`sum`, `avg`, …).
    #[serde(default)]
    pub math_property: Option<String>,
}

impl EventNode {
    pub fn label(&self) -> String {
        self.custom_name
            .clone()
            .or_else(|| self.event.clone())
            .unwrap_or_else(|| "All events".to_owned())
    }
}

/// Aggregation for a trends series. Person counts are always counts of
/// *persons* (distinct ids resolved through identity merges), never raw
/// distinct ids.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum Math {
    /// Event count.
    #[default]
    Total,
    /// Unique persons within each interval bucket.
    Dau,
    /// Unique persons in the trailing 7 days ending at each bucket.
    WeeklyActive,
    /// Unique persons in the trailing 30 days ending at each bucket.
    MonthlyActive,
    /// Unique `$session_id` values within each bucket.
    UniqueSession,
    Sum,
    Avg,
    Min,
    Max,
    Median,
    P90,
    P95,
    P99,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub enum ChartDisplay {
    #[default]
    ActionsLineGraph,
    ActionsAreaGraph,
    ActionsBar,
    ActionsBarValue,
    ActionsTable,
    ActionsPie,
    BoldNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct TrendsQuery {
    pub series: Vec<EventNode>,
    #[serde(default)]
    pub date_range: DateRange,
    #[serde(default)]
    pub interval: Interval,
    /// Filters applied to every series.
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
    #[serde(default)]
    pub breakdown: Option<Breakdown>,
    /// Arithmetic over series letters, e.g. `"A / B * 100"`. When set the
    /// result contains the formula series instead of the raw series.
    #[serde(default)]
    pub formula: Option<String>,
    /// Also return the same series for the previous period.
    #[serde(default)]
    pub compare: bool,
    #[serde(default)]
    pub display: ChartDisplay,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum FunnelOrder {
    /// Steps in order; other events may happen in between.
    #[default]
    Ordered,
    /// Steps in order with no other events in between.
    Strict,
    /// Steps in any order.
    Unordered,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum WindowUnit {
    Minute,
    Hour,
    #[default]
    Day,
    Week,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FunnelWindow {
    pub interval: u32,
    pub unit: WindowUnit,
}

impl Default for FunnelWindow {
    fn default() -> Self {
        Self {
            interval: 14,
            unit: WindowUnit::Day,
        }
    }
}

impl FunnelWindow {
    pub fn seconds(&self) -> i64 {
        let unit = match self.unit {
            WindowUnit::Minute => 60,
            WindowUnit::Hour => 3_600,
            WindowUnit::Day => 86_400,
            WindowUnit::Week => 604_800,
        };
        i64::from(self.interval) * unit
    }
}

/// A person who performs `event` between steps `from_step` and `to_step`
/// (0-based) is excluded from the funnel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FunnelExclusion {
    pub event: String,
    pub from_step: usize,
    pub to_step: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FunnelsQuery {
    /// Funnel steps in order; `math` is ignored.
    pub series: Vec<EventNode>,
    #[serde(default)]
    pub date_range: DateRange,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
    /// Breaks the funnel down by a property of the first step's event
    /// (first-touch attribution) or of the person.
    #[serde(default)]
    pub breakdown: Option<Breakdown>,
    #[serde(default)]
    pub funnel_window: FunnelWindow,
    #[serde(default)]
    pub funnel_order: FunnelOrder,
    #[serde(default)]
    pub exclusions: Vec<FunnelExclusion>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum RetentionPeriod {
    #[default]
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum RetentionType {
    /// A person belongs to the cohort of every period they performed the
    /// target event in.
    #[default]
    RetentionRecurring,
    /// A person belongs only to the cohort of the first period they ever
    /// performed the target event in.
    RetentionFirstTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct RetentionQuery {
    /// Event that places a person into a cohort.
    pub target: EventNode,
    /// Event that counts as coming back.
    pub returning: EventNode,
    #[serde(default)]
    pub period: RetentionPeriod,
    /// Number of cohorts and of periods per cohort.
    #[serde(default = "default_total_intervals")]
    pub total_intervals: u32,
    #[serde(default)]
    pub retention_type: RetentionType,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
}

fn default_total_intervals() -> u32 {
    8
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct LifecycleQuery {
    pub series: EventNode,
    #[serde(default)]
    pub date_range: DateRange,
    #[serde(default)]
    pub interval: Interval,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct StickinessQuery {
    pub series: Vec<EventNode>,
    #[serde(default)]
    pub date_range: DateRange,
    #[serde(default)]
    pub interval: Interval,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum PathsType {
    /// Nodes are `$pathname` of `$pageview` events.
    #[default]
    Pageviews,
    /// Nodes are names of non-`$` events.
    CustomEvents,
    /// Pageviews by path plus custom events by name.
    All,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct PathsQuery {
    #[serde(default)]
    pub paths_type: PathsType,
    /// Only paths starting at this node (path or event name).
    #[serde(default)]
    pub start_point: Option<String>,
    /// Only paths ending at this node.
    #[serde(default)]
    pub end_point: Option<String>,
    /// Maximum steps per path.
    #[serde(default = "default_step_limit")]
    pub step_limit: u32,
    /// Maximum links returned (strongest first).
    #[serde(default = "default_edge_limit")]
    pub edge_limit: u32,
    #[serde(default)]
    pub date_range: DateRange,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
}

fn default_step_limit() -> u32 {
    5
}

fn default_edge_limit() -> u32 {
    50
}

/// Read-only SQL over the project's events. The only relation is `events`
/// (columns: uuid, event, distinct_id, person_id, timestamp, properties, and
/// the promoted columns). `timestamp` is a UTC `TIMESTAMP`; `now_utc()` is the
/// current UTC time. Capped in rows, time and memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct SqlQuery {
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(tag = "kind")]
pub enum InsightQuery {
    TrendsQuery(TrendsQuery),
    FunnelsQuery(FunnelsQuery),
    RetentionQuery(RetentionQuery),
    LifecycleQuery(LifecycleQuery),
    StickinessQuery(StickinessQuery),
    PathsQuery(PathsQuery),
    SqlQuery(SqlQuery),
}

impl InsightQuery {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TrendsQuery(_) => "TrendsQuery",
            Self::FunnelsQuery(_) => "FunnelsQuery",
            Self::RetentionQuery(_) => "RetentionQuery",
            Self::LifecycleQuery(_) => "LifecycleQuery",
            Self::StickinessQuery(_) => "StickinessQuery",
            Self::PathsQuery(_) => "PathsQuery",
            Self::SqlQuery(_) => "SqlQuery",
        }
    }
}

/// `POST /api/projects/{project_id}/query` body.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct QueryRequest {
    pub query: InsightQuery,
    /// Bypass the result cache.
    #[serde(default)]
    pub refresh: bool,
}

// ── Results ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct QueryResponse {
    pub result: InsightResult,
    pub meta: QueryMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct QueryMeta {
    pub kind: String,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub cached: bool,
    /// Changes whenever the project's stored events change.
    #[ts(type = "number")]
    pub data_version: u64,
    /// Resolved absolute range, RFC 3339 UTC.
    pub date_from: String,
    pub date_to: String,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(tag = "kind")]
pub enum InsightResult {
    Trends { series: Vec<TrendSeries> },
    Funnels {
        steps: Vec<FunnelStepResult>,
        breakdowns: Vec<FunnelBreakdownResult>,
        time_to_convert: Vec<HistogramBin>,
    },
    Retention {
        period: RetentionPeriod,
        cohorts: Vec<RetentionCohort>,
    },
    Lifecycle {
        /// Bucket starts, RFC 3339.
        days: Vec<String>,
        labels: Vec<String>,
        new: Vec<i64>,
        returning: Vec<i64>,
        resurrecting: Vec<i64>,
        /// Reported as negative counts, PostHog-style.
        dormant: Vec<i64>,
    },
    Stickiness { series: Vec<StickinessSeries> },
    Paths { links: Vec<PathLink> },
    Sql {
        columns: Vec<String>,
        types: Vec<String>,
        #[ts(type = "Array<Array<unknown>>")]
        rows: Vec<Vec<serde_json::Value>>,
        truncated: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct TrendSeries {
    pub label: String,
    /// Index into the query's `series`; `null` for a formula series.
    pub series_index: Option<usize>,
    pub breakdown_value: Option<String>,
    /// `"previous"` for the comparison period when `compare` is set.
    pub compare: Option<String>,
    /// Bucket starts, RFC 3339.
    pub days: Vec<String>,
    /// Human labels for the buckets ("Mon 3 Oct", "14:00", …).
    pub labels: Vec<String>,
    pub data: Vec<f64>,
    /// Total over the range for counts; the whole-range value for
    /// person-uniqueness and property maths.
    pub aggregated_value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FunnelStepResult {
    /// 0-based.
    pub order: usize,
    pub name: String,
    #[ts(type = "number")]
    pub count: u64,
    /// Percent, 0–100.
    pub conversion_from_previous: f64,
    /// Percent, 0–100.
    pub conversion_from_start: f64,
    #[ts(type = "number")]
    pub dropped_off: u64,
    pub average_conversion_time_s: Option<f64>,
    pub median_conversion_time_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FunnelBreakdownResult {
    pub breakdown_value: String,
    pub steps: Vec<FunnelStepResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct HistogramBin {
    pub from_s: f64,
    pub to_s: f64,
    #[ts(type = "number")]
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct RetentionCohort {
    /// Cohort period start, RFC 3339.
    pub date: String,
    pub label: String,
    #[ts(type = "number")]
    pub size: u64,
    /// `values[i]` = persons from this cohort who returned in period `i`
    /// (`values[0] == size`). Periods after now are omitted.
    #[ts(type = "Array<number>")]
    pub values: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct StickinessSeries {
    pub label: String,
    pub series_index: usize,
    /// `data[i]` = persons active in exactly `i + 1` intervals of the range.
    #[ts(type = "Array<number>")]
    pub data: Vec<u64>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct PathLink {
    /// Step-prefixed node names: `"1_/pricing"` → `"2_/signup"`.
    pub source: String,
    pub target: String,
    #[ts(type = "number")]
    pub value: u64,
    pub average_conversion_time_s: f64,
}

// ── Actor drill-down ──────────────────────────────────────────────

/// Which number of an insight result to open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(tag = "type")]
pub enum ActorSelection {
    TrendsPoint {
        series_index: usize,
        /// Bucket start, as in `TrendSeries.days`.
        day: String,
        #[serde(default)]
        breakdown_value: Option<String>,
    },
    FunnelStep {
        /// 0-based step.
        step: usize,
        /// `true`: persons who reached `step`. `false`: persons who reached
        /// `step - 1` but not `step`.
        converted: bool,
        #[serde(default)]
        breakdown_value: Option<String>,
    },
    RetentionCell {
        cohort_date: String,
        interval: u32,
    },
    LifecycleCell {
        status: LifecycleStatus,
        day: String,
    },
    StickinessBar {
        series_index: usize,
        intervals: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStatus {
    New,
    Returning,
    Resurrecting,
    Dormant,
}

/// `POST /api/projects/{project_id}/query/actors` body.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct ActorsRequest {
    pub query: InsightQuery,
    pub selection: ActorSelection,
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "default_actor_limit")]
    pub limit: u32,
}

fn default_actor_limit() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct ActorsResponse {
    pub persons: Vec<super::persons::PersonSummary>,
    pub has_more: bool,
}
