//! Feature flags: storage, validation, and PostHog-exact evaluation.
//!
//! - [`store`] owns definitions in `control.db` (project-scoped, versioned,
//!   soft-deleted) and a bounded cache of compiled definitions.
//! - [`eval`] decides which flags are on for whom: PostHog's hash, rollout,
//!   variant ranges, property operators, and reason codes.
//! - [`validate`] bounds every definition (explicit limits on flags, groups,
//!   filters, variants, payload sizes).
//!
//! Wire shapes (`/flags`, `/decide`, local evaluation, dashboard API) live in
//! `routes/flags.rs`. The model is `contract::flags`, so local-evaluation
//! definitions are emitted in PostHog's own format.

pub mod eval;
pub mod store;
pub mod validate;

use serde_json::{Map, Value};

pub use eval::{CompiledFlag, Evaluation, ReasonCode};
pub use store::{FeatureFlagPatch, FlagStore, FlagStoreError, ProjectFlags};

use crate::persons::PersonStore;

/// Who a flag is evaluated for: merged person properties plus the two
/// possible bucketing ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    pub distinct_id: String,
    /// Stored person properties overridden by request `person_properties`.
    pub properties: Map<String, Value>,
    /// Bucketing id for flags with `ensure_experience_continuity`: the
    /// person's first-seen key, which survives identify merges.
    pub continuity_id: String,
}

impl Subject {
    pub fn bucketing_id(&self, flag: &CompiledFlag) -> &str {
        if flag.flag.ensure_experience_continuity {
            &self.continuity_id
        } else {
            &self.distinct_id
        }
    }

    pub fn evaluate(&self, flag: &CompiledFlag) -> Evaluation {
        flag.evaluate(self.bucketing_id(flag), &self.properties)
    }
}

/// What the caller knows about the person besides the distinct id.
#[derive(Debug, Clone, Default)]
pub struct SubjectHints<'a> {
    /// posthog-js sends the pre-identify id as `$anon_distinct_id`; until the
    /// identify merge is published, its first-seen key is the continuity key.
    pub anon_distinct_id: Option<&'a str>,
    pub person_properties: Option<&'a Map<String, Value>>,
}

/// Resolve the subject for evaluation. Returns `true` alongside when the
/// person store failed: evaluation still proceeds on request properties, and
/// the response reports `errorsWhileComputingFlags`.
pub fn resolve_subject(
    persons: &PersonStore,
    project_id: &str,
    distinct_id: &str,
    hints: &SubjectHints<'_>,
    needs_person: bool,
) -> (Subject, bool) {
    let mut failed = false;
    let mut properties = Map::new();
    let mut continuity_id = None;
    if needs_person {
        match persons.person_for_distinct_id(project_id, distinct_id) {
            Ok(Some(person)) => {
                properties = person.properties;
                continuity_id = Some(person.first_seen_key);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "person lookup failed during flag evaluation");
                failed = true;
            }
        }
        if continuity_id.is_none()
            && !failed
            && let Some(anon) = hints.anon_distinct_id.filter(|anon| !anon.is_empty())
        {
            continuity_id = Some(match persons.person_for_distinct_id(project_id, anon) {
                Ok(Some(person)) => person.first_seen_key,
                Ok(None) => anon.to_owned(),
                Err(error) => {
                    tracing::warn!(%error, "anonymous person lookup failed during flag evaluation");
                    failed = true;
                    anon.to_owned()
                }
            });
        }
    }
    if let Some(overrides) = hints.person_properties {
        for (key, value) in overrides {
            properties.insert(key.clone(), value.clone());
        }
    }
    let continuity_id = continuity_id
        .or_else(|| {
            hints
                .anon_distinct_id
                .filter(|anon| !anon.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| distinct_id.to_owned());
    (
        Subject {
            distinct_id: distinct_id.to_owned(),
            properties,
            continuity_id,
        },
        failed,
    )
}

/// Does evaluating these flags need the stored person at all?
pub fn needs_person<'a>(flags: impl IntoIterator<Item = &'a CompiledFlag>) -> bool {
    flags.into_iter().any(|flag| {
        flag.flag.active && (flag.flag.ensure_experience_continuity || flag.reads_properties())
    })
}
