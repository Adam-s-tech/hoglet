//! Building blocks shared by every API shape: filters, ranges, breakdowns.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What a property filter or breakdown reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum PropertySource {
    /// A property of the event (`$current_url`, `plan`, …).
    #[default]
    Event,
    /// A property of the person the event's distinct id resolves to.
    Person,
}

/// PostHog's property operators. `exact`/`is_not` accept a scalar or an
/// array (array = any of / none of).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum PropertyOperator {
    #[default]
    Exact,
    IsNot,
    Icontains,
    NotIcontains,
    Regex,
    NotRegex,
    Gt,
    Gte,
    Lt,
    Lte,
    IsSet,
    IsNotSet,
    IsDateBefore,
    IsDateAfter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct PropertyFilter {
    pub key: String,
    #[serde(rename = "type", default)]
    pub source: PropertySource,
    #[serde(default)]
    pub operator: PropertyOperator,
    /// String, number, boolean, or an array of those. Ignored by
    /// `is_set`/`is_not_set`.
    #[serde(default)]
    #[ts(type = "string | number | boolean | Array<string | number | boolean> | null")]
    pub value: serde_json::Value,
}

/// Relative or absolute range, PostHog conventions.
///
/// `date_from`: `"-24h"`, `"-7d"`, `"-4w"`, `"-3m"`, `"-1y"`, `"dStart"`
/// (today), `"mStart"` (this month), `"yStart"`, `"all"`, or an ISO 8601
/// date/datetime. `date_to`: same forms, `null` = now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct DateRange {
    pub date_from: String,
    #[serde(default)]
    pub date_to: Option<String>,
}

impl Default for DateRange {
    fn default() -> Self {
        Self {
            date_from: "-7d".to_owned(),
            date_to: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum Interval {
    Hour,
    #[default]
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct Breakdown {
    pub property: String,
    #[serde(rename = "type", default)]
    pub source: PropertySource,
    /// Top-N values kept; the rest fold into `"$$_other"`.
    #[serde(default = "default_breakdown_limit")]
    pub limit: u32,
}

fn default_breakdown_limit() -> u32 {
    10
}

/// The bucket other breakdown values fold into.
pub const BREAKDOWN_OTHER: &str = "$$_other";
/// The bucket for events without the property.
pub const BREAKDOWN_NONE: &str = "$$_none";

/// Uniform JSON error body: `{"error": {"code", "message", "request_id"}}`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct ApiError {
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
}
