//! Feature flags — PostHog's flag model, so local-evaluation definitions are
//! emitted verbatim and PostHog flags import without translation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::common::PropertyFilter;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FlagVariant {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Share of matched persons, 0–100. Variants must sum to 100.
    pub rollout_percentage: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct Multivariate {
    pub variants: Vec<FlagVariant>,
}

/// One release condition. Matches when every person-property filter matches,
/// then admits `rollout_percentage` of those persons (null = 100).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FlagConditionGroup {
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
    #[serde(default)]
    pub rollout_percentage: Option<f64>,
    /// Force this variant for persons matching the group.
    #[serde(default)]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FlagFilters {
    #[serde(default)]
    pub groups: Vec<FlagConditionGroup>,
    #[serde(default)]
    pub multivariate: Option<Multivariate>,
    /// Payload per flag value: key `"true"` for boolean flags, the variant
    /// key for multivariate flags.
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub payloads: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FeatureFlag {
    #[ts(type = "number")]
    pub id: i64,
    pub key: String,
    /// Free-text description.
    pub name: String,
    pub active: bool,
    pub filters: FlagFilters,
    /// Keep a person's value stable across `identify`.
    pub ensure_experience_continuity: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Body of `POST /api/projects/{project_id}/feature_flags` and
/// `PATCH /api/projects/{project_id}/feature_flags/{id}` (PATCH: all fields
/// optional except as noted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FeatureFlagInput {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub filters: FlagFilters,
    #[serde(default)]
    pub ensure_experience_continuity: bool,
}

fn default_true() -> bool {
    true
}

/// `GET /api/projects/{project_id}/feature_flags/{id}/evaluate?distinct_id=`
/// — what one person gets, and why. Powers the flag page's "test a user".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct FlagEvaluation {
    pub key: String,
    pub enabled: bool,
    pub variant: Option<String>,
    /// `condition_match`, `no_condition_match`, `out_of_rollout_bound`,
    /// `disabled`, …
    pub reason: String,
    pub condition_index: Option<usize>,
    #[ts(type = "unknown")]
    pub payload: Option<serde_json::Value>,
}
