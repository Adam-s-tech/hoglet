//! Flag evaluation with PostHog's exact semantics.
//!
//! Bucketing is byte-for-byte PostHog: `sha1("{flag_key}.{bucketing_id}{salt}")`,
//! the first 15 hex digits as an integer over `0xFFFFFFFFFFFFFFF`. Rollout
//! uses salt `""`, variant selection salt `"variant"` over cumulative variant
//! ranges. A flag at 30% selects the *same* people on Hoglet as on PostHog and
//! as in the SDKs' own local evaluation.
//!
//! Condition groups are evaluated in order. A group matches when every
//! property filter matches and the bucketing hash is inside its rollout. The
//! first matching group decides the value; its `variant` override wins when it
//! names a real variant.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Months, NaiveDate, NaiveDateTime, Utc};
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

use crate::contract::common::{PropertyFilter, PropertyOperator};
use crate::contract::flags::{FeatureFlag, FlagConditionGroup};

/// `0xFFFFFFFFFFFFFFF` — the largest 15-hex-digit value.
const LONG_SCALE: f64 = 0xFFF_FFFF_FFFF_FFFF_u64 as f64;

/// Compiled regex programs are bounded so a hostile pattern cannot allocate
/// unboundedly at flag-definition or evaluation time.
const MAX_REGEX_PROGRAM_BYTES: usize = 256 * 1024;

/// PostHog's relative-date guard: larger counts are rejected.
const MAX_RELATIVE_DATE_COUNT: i64 = 10_000;

/// PostHog's consistent hash in `[0, 1]`.
pub fn hash(flag_key: &str, bucketing_id: &str, salt: &str) -> f64 {
    let mut hasher = Sha1::new();
    hasher.update(flag_key.as_bytes());
    hasher.update(b".");
    hasher.update(bucketing_id.as_bytes());
    hasher.update(salt.as_bytes());
    let digest = hasher.finalize();
    // 15 hex digits = the top 60 bits of the first 8 digest bytes.
    let mut prefix = [0_u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    let value = u64::from_be_bytes(prefix) >> 4;
    value as f64 / LONG_SCALE
}

/// Why a flag has its value. Ordered so the "closest miss" wins when no
/// condition matches (PostHog reports out-of-rollout over no-match).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasonCode {
    Disabled,
    NoConditionMatch,
    OutOfRolloutBound,
    ConditionMatch,
}

impl ReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoConditionMatch => "no_condition_match",
            Self::OutOfRolloutBound => "out_of_rollout_bound",
            Self::ConditionMatch => "condition_match",
        }
    }

    pub fn description(self, condition_index: Option<usize>) -> String {
        match (self, condition_index) {
            (Self::ConditionMatch, Some(index)) => format!("Matched condition set {}", index + 1),
            (Self::ConditionMatch, None) => "Matched condition set".to_owned(),
            (Self::NoConditionMatch, _) => "No matching condition set".to_owned(),
            (Self::OutOfRolloutBound, _) => "Out of rollout bound".to_owned(),
            (Self::Disabled, _) => "Flag is disabled".to_owned(),
        }
    }
}

/// One flag's value for one person.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub enabled: bool,
    pub variant: Option<String>,
    pub reason: ReasonCode,
    pub condition_index: Option<usize>,
    /// The payload for the resulting value (`"true"` or the variant key).
    pub payload: Option<Value>,
}

/// A flag definition prepared for repeated evaluation: regexes are compiled
/// once, not per request.
#[derive(Debug, Clone)]
pub struct CompiledFlag {
    pub flag: FeatureFlag,
    pub version: i64,
    /// Pattern → compiled program. `None` = invalid pattern, which never
    /// matches (both `regex` and `not_regex` are false, as in PostHog).
    regexes: HashMap<String, Option<Regex>>,
}

impl CompiledFlag {
    pub fn compile(flag: FeatureFlag, version: i64) -> Self {
        let mut regexes = HashMap::new();
        for group in &flag.filters.groups {
            for filter in &group.properties {
                if matches!(
                    filter.operator,
                    PropertyOperator::Regex | PropertyOperator::NotRegex
                ) {
                    let pattern = value_to_string(&filter.value);
                    regexes
                        .entry(pattern.clone())
                        .or_insert_with(|| compile_regex(&pattern));
                }
            }
        }
        Self {
            flag,
            version,
            regexes,
        }
    }

    /// Does any condition read person properties?
    pub fn reads_properties(&self) -> bool {
        self.flag
            .filters
            .groups
            .iter()
            .any(|group| !group.properties.is_empty())
    }

    /// Evaluate for one bucketing id against the merged person properties.
    pub fn evaluate(&self, bucketing_id: &str, properties: &Map<String, Value>) -> Evaluation {
        if !self.flag.active {
            return Evaluation {
                enabled: false,
                variant: None,
                reason: ReasonCode::Disabled,
                condition_index: None,
                payload: None,
            };
        }
        let mut closest = ReasonCode::NoConditionMatch;
        let mut closest_index = None;
        for (index, group) in self.flag.filters.groups.iter().enumerate() {
            match self.match_group(group, bucketing_id, properties) {
                GroupMatch::Match => {
                    let variant = group
                        .variant
                        .as_ref()
                        .filter(|key| self.has_variant(key))
                        .cloned()
                        .or_else(|| self.matching_variant(bucketing_id));
                    let payload_key = variant.as_deref().unwrap_or("true");
                    return Evaluation {
                        enabled: true,
                        payload: self.flag.filters.payloads.get(payload_key).cloned(),
                        variant,
                        reason: ReasonCode::ConditionMatch,
                        condition_index: Some(index),
                    };
                }
                GroupMatch::OutOfRollout => {
                    if closest < ReasonCode::OutOfRolloutBound {
                        closest = ReasonCode::OutOfRolloutBound;
                        closest_index = Some(index);
                    }
                }
                GroupMatch::NoMatch => {}
            }
        }
        Evaluation {
            enabled: false,
            variant: None,
            reason: closest,
            condition_index: closest_index,
            payload: None,
        }
    }

    fn match_group(
        &self,
        group: &FlagConditionGroup,
        bucketing_id: &str,
        properties: &Map<String, Value>,
    ) -> GroupMatch {
        for filter in &group.properties {
            if !match_property(filter, properties, &self.regexes) {
                return GroupMatch::NoMatch;
            }
        }
        match group.rollout_percentage {
            Some(rollout) if hash(&self.flag.key, bucketing_id, "") > rollout / 100.0 => {
                GroupMatch::OutOfRollout
            }
            _ => GroupMatch::Match,
        }
    }

    fn has_variant(&self, key: &str) -> bool {
        self.flag
            .filters
            .multivariate
            .as_ref()
            .is_some_and(|multivariate| multivariate.variants.iter().any(|v| v.key == key))
    }

    /// PostHog's variant lookup table: cumulative `[min, max)` ranges.
    fn matching_variant(&self, bucketing_id: &str) -> Option<String> {
        let multivariate = self.flag.filters.multivariate.as_ref()?;
        let value = hash(&self.flag.key, bucketing_id, "variant");
        let mut value_min = 0.0_f64;
        for variant in &multivariate.variants {
            let value_max = value_min + variant.rollout_percentage / 100.0;
            if value >= value_min && value < value_max {
                return Some(variant.key.clone());
            }
            value_min = value_max;
        }
        None
    }
}

enum GroupMatch {
    Match,
    NoMatch,
    OutOfRollout,
}

pub(crate) fn compile_regex(pattern: &str) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .size_limit(MAX_REGEX_PROGRAM_BYTES)
        .dfa_size_limit(MAX_REGEX_PROGRAM_BYTES)
        .build()
        .ok()
}

/// Match one property filter. A missing property matches nothing except
/// `is_not_set` (PostHog's server-side behavior).
fn match_property(
    filter: &PropertyFilter,
    properties: &Map<String, Value>,
    regexes: &HashMap<String, Option<Regex>>,
) -> bool {
    let Some(actual) = properties.get(&filter.key) else {
        return filter.operator == PropertyOperator::IsNotSet;
    };
    let expected = &filter.value;
    match filter.operator {
        PropertyOperator::IsSet => true,
        PropertyOperator::IsNotSet => false,
        PropertyOperator::Exact => exact_match(expected, actual),
        PropertyOperator::IsNot => !exact_match(expected, actual),
        _ if actual.is_null() => false,
        PropertyOperator::Icontains => icontains(expected, actual),
        PropertyOperator::NotIcontains => !icontains(expected, actual),
        PropertyOperator::Regex | PropertyOperator::NotRegex => {
            let pattern = value_to_string(expected);
            let compiled = match regexes.get(&pattern) {
                Some(compiled) => compiled.clone(),
                None => compile_regex(&pattern),
            };
            let Some(regex) = compiled else {
                return false;
            };
            let found = regex.is_match(&value_to_string(actual));
            if filter.operator == PropertyOperator::Regex {
                found
            } else {
                !found
            }
        }
        PropertyOperator::Gt
        | PropertyOperator::Gte
        | PropertyOperator::Lt
        | PropertyOperator::Lte => compare(filter.operator, expected, actual),
        PropertyOperator::IsDateBefore | PropertyOperator::IsDateAfter => {
            let (Some(boundary), Some(value)) = (flag_date(expected), property_date(actual)) else {
                return false;
            };
            if filter.operator == PropertyOperator::IsDateBefore {
                value < boundary
            } else {
                value > boundary
            }
        }
    }
}

/// `exact`: scalar or array (any-of), case-insensitive string comparison;
/// truthy/falsy filter values compare by truthiness (property matching v1).
fn exact_match(expected: &Value, actual: &Value) -> bool {
    if expected.as_array().is_some_and(Vec::is_empty) || is_truthy_or_falsy(expected) {
        return is_truthy(expected) == is_truthy(actual);
    }
    let actual = value_to_string(actual).to_lowercase();
    match expected {
        Value::Array(candidates) => candidates
            .iter()
            .any(|candidate| value_to_string(candidate).to_lowercase() == actual),
        scalar => value_to_string(scalar).to_lowercase() == actual,
    }
}

fn is_truthy_or_falsy(value: &Value) -> bool {
    match value {
        Value::Bool(_) => true,
        Value::String(text) => {
            text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false")
        }
        Value::Array(items) => items.iter().all(is_truthy_or_falsy),
        _ => false,
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.eq_ignore_ascii_case("true"),
        Value::Array(items) => items.iter().all(is_truthy),
        _ => false,
    }
}

fn icontains(expected: &Value, actual: &Value) -> bool {
    value_to_string(actual)
        .to_ascii_lowercase()
        .contains(&value_to_string(expected).to_ascii_lowercase())
}

/// Numeric when both sides parse as numbers, else string comparison.
fn compare(operator: PropertyOperator, expected: &Value, actual: &Value) -> bool {
    use std::cmp::Ordering;
    let ordering = match (as_number(expected), as_number(actual)) {
        (Some(expected), Some(actual)) => actual.partial_cmp(&expected),
        _ => Some(value_to_string(actual).cmp(&value_to_string(expected))),
    };
    let Some(ordering) = ordering else {
        return false;
    };
    match operator {
        PropertyOperator::Gt => ordering == Ordering::Greater,
        PropertyOperator::Gte => ordering != Ordering::Less,
        PropertyOperator::Lt => ordering == Ordering::Less,
        PropertyOperator::Lte => ordering != Ordering::Greater,
        _ => false,
    }
}

fn as_number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

/// The string form PostHog compares: strings verbatim, everything else as
/// compact JSON (integral floats without a fraction, like JavaScript).
pub(crate) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                integer.to_string()
            } else if let Some(integer) = number.as_u64() {
                integer.to_string()
            } else {
                let float = number.as_f64().unwrap_or(f64::NAN);
                if float.fract() == 0.0 && float.abs() < 1e15 {
                    format!("{}", float as i64)
                } else {
                    float.to_string()
                }
            }
        }
        other => other.to_string(),
    }
}

/// The boundary a date filter compares against: relative (`-7d`, `3h`,
/// `2w`, `1m`, `1y`) or absolute.
pub(crate) fn flag_date(value: &Value) -> Option<DateTime<Utc>> {
    let text = value.as_str()?;
    relative_date(text, Utc::now()).or_else(|| parse_date(text))
}

fn property_date(value: &Value) -> Option<DateTime<Utc>> {
    parse_date(value.as_str()?)
}

fn relative_date(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let body = text.strip_prefix('-').unwrap_or(text);
    let unit = body.chars().last()?;
    let digits = &body[..body.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let count: i64 = digits.parse().ok()?;
    if count >= MAX_RELATIVE_DATE_COUNT {
        return None;
    }
    match unit {
        'h' => now.checked_sub_signed(Duration::hours(count)),
        'd' => now.checked_sub_signed(Duration::days(count)),
        'w' => now.checked_sub_signed(Duration::weeks(count)),
        'm' => now.checked_sub_months(Months::new(u32::try_from(count).ok()?)),
        'y' => now.checked_sub_months(Months::new(u32::try_from(count.checked_mul(12)?).ok()?)),
        _ => None,
    }
}

fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%z",
    ] {
        if let Ok(parsed) = DateTime::parse_from_str(text, format) {
            return Some(parsed.with_timezone(&Utc));
        }
    }
    let naive = text.strip_suffix('Z').unwrap_or(text);
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(naive, format) {
            return Some(parsed.and_utc());
        }
    }
    NaiveDate::parse_from_str(naive, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|datetime| datetime.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::common::PropertySource;
    use crate::contract::flags::{FlagFilters, FlagVariant, Multivariate};
    use serde_json::json;

    fn flag(key: &str, filters: FlagFilters) -> CompiledFlag {
        CompiledFlag::compile(
            FeatureFlag {
                id: 1,
                key: key.into(),
                name: String::new(),
                active: true,
                filters,
                ensure_experience_continuity: false,
                created_at: String::new(),
                updated_at: String::new(),
            },
            1,
        )
    }

    fn group(rollout: Option<f64>, properties: Vec<PropertyFilter>) -> FlagConditionGroup {
        FlagConditionGroup {
            properties,
            rollout_percentage: rollout,
            variant: None,
        }
    }

    fn filter(key: &str, operator: PropertyOperator, value: Value) -> PropertyFilter {
        PropertyFilter {
            key: key.into(),
            source: PropertySource::Person,
            operator,
            value,
        }
    }

    fn props(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn matches(operator: PropertyOperator, expected: Value, actual: Value) -> bool {
        let compiled = flag(
            "f",
            FlagFilters {
                groups: vec![group(None, vec![filter("p", operator, expected)])],
                ..FlagFilters::default()
            },
        );
        compiled
            .evaluate("id", &props(json!({ "p": actual })))
            .enabled
    }

    fn variants(splits: &[(&str, f64)]) -> Option<Multivariate> {
        Some(Multivariate {
            variants: splits
                .iter()
                .map(|(key, rollout)| FlagVariant {
                    key: (*key).into(),
                    name: None,
                    rollout_percentage: *rollout,
                })
                .collect(),
        })
    }

    const VECTORS: &str = include_str!("testdata/posthog_consistency.txt");

    /// posthog-python `test_simple_flag_consistency`: 1000 distinct ids at a
    /// 45% rollout — every expected value, verbatim.
    #[test]
    fn rollout_matches_posthog_python_vectors() {
        let expected = VECTORS.lines().next().expect("simple vector line");
        assert_eq!(expected.len(), 1000);
        let compiled = flag(
            "simple-flag",
            FlagFilters {
                groups: vec![group(Some(45.0), vec![])],
                ..FlagFilters::default()
            },
        );
        for (index, want) in expected.bytes().enumerate() {
            let got = compiled
                .evaluate(&format!("distinct_id_{index}"), &Map::new())
                .enabled;
            assert_eq!(got, want == b'1', "distinct_id_{index}");
        }
    }

    /// posthog-python `test_multivariate_flag_consistency`: 55% rollout, five
    /// variants 50/20/20/5/5.
    #[test]
    fn variants_match_posthog_python_vectors() {
        let expected = VECTORS.lines().nth(1).expect("multivariate vector line");
        assert_eq!(expected.len(), 1000);
        let compiled = flag(
            "multivariate-flag",
            FlagFilters {
                groups: vec![group(Some(55.0), vec![])],
                multivariate: variants(&[
                    ("first-variant", 50.0),
                    ("second-variant", 20.0),
                    ("third-variant", 20.0),
                    ("fourth-variant", 5.0),
                    ("fifth-variant", 5.0),
                ]),
                ..FlagFilters::default()
            },
        );
        let names = [
            "first-variant",
            "second-variant",
            "third-variant",
            "fourth-variant",
            "fifth-variant",
        ];
        for (index, want) in expected.bytes().enumerate() {
            let got = compiled.evaluate(&format!("distinct_id_{index}"), &Map::new());
            match want {
                b'-' => assert!(!got.enabled, "distinct_id_{index}"),
                digit => {
                    let name = names[usize::from(digit - b'1')];
                    assert_eq!(got.variant.as_deref(), Some(name), "distinct_id_{index}");
                }
            }
        }
    }

    #[test]
    fn hash_is_the_sha1_prefix_fraction() {
        // PostHog's formula computed the slow way: hex digest, first 15 hex
        // digits parsed as an integer, over 0xFFFFFFFFFFFFFFF.
        let digest = hex::encode(Sha1::digest(b"simple-flag.distinct_id_0"));
        let expected = u64::from_str_radix(&digest[..15], 16).unwrap() as f64 / LONG_SCALE;
        assert_eq!(hash("simple-flag", "distinct_id_0", ""), expected);
        let salted = hex::encode(Sha1::digest(b"f.uvariant"));
        let expected = u64::from_str_radix(&salted[..15], 16).unwrap() as f64 / LONG_SCALE;
        assert_eq!(hash("f", "u", "variant"), expected);
    }

    #[test]
    fn rollout_is_monotonic_and_roughly_accurate() {
        let on = |rollout: f64, id: &str| {
            flag(
                "f",
                FlagFilters {
                    groups: vec![group(Some(rollout), vec![])],
                    ..FlagFilters::default()
                },
            )
            .evaluate(id, &Map::new())
            .enabled
        };
        let mut enabled = 0;
        for index in 0..2000 {
            let id = format!("u{index}");
            if on(30.0, &id) {
                assert!(on(60.0, &id), "{id} dropped when rollout rose");
            }
            enabled += usize::from(on(50.0, &id));
        }
        assert!((900..1100).contains(&enabled), "{enabled}/2000");
        assert!(on(100.0, "anyone"));
        assert!(!on(0.0, "anyone"));
    }

    #[test]
    fn variant_distribution_follows_weights() {
        let compiled = flag(
            "exp",
            FlagFilters {
                groups: vec![group(None, vec![])],
                multivariate: variants(&[("a", 25.0), ("b", 75.0)]),
                ..FlagFilters::default()
            },
        );
        let a = (0..4000)
            .filter(|index| {
                compiled
                    .evaluate(&format!("u{index}"), &Map::new())
                    .variant
                    .as_deref()
                    == Some("a")
            })
            .count();
        assert!((850..1150).contains(&a), "{a}/4000");
    }

    #[test]
    fn groups_evaluate_in_order_with_variant_override_and_payloads() {
        let mut payloads = std::collections::BTreeMap::new();
        payloads.insert("b".to_owned(), json!({"color": "blue"}));
        let compiled = flag(
            "exp",
            FlagFilters {
                groups: vec![
                    FlagConditionGroup {
                        properties: vec![filter(
                            "email",
                            PropertyOperator::Icontains,
                            json!("@corp.com"),
                        )],
                        rollout_percentage: Some(100.0),
                        variant: Some("b".into()),
                    },
                    FlagConditionGroup {
                        properties: vec![],
                        rollout_percentage: Some(100.0),
                        variant: Some("missing".into()),
                    },
                ],
                multivariate: variants(&[("a", 100.0), ("b", 0.0)]),
                payloads,
            },
        );
        let staff = compiled.evaluate("u", &props(json!({"email": "Ana@CORP.com"})));
        assert_eq!(staff.variant.as_deref(), Some("b"));
        assert_eq!(staff.condition_index, Some(0));
        assert_eq!(staff.reason, ReasonCode::ConditionMatch);
        assert_eq!(staff.payload, Some(json!({"color": "blue"})));
        // An override naming no real variant falls back to hashing.
        let other = compiled.evaluate("u", &props(json!({"email": "x@else.com"})));
        assert_eq!(other.variant.as_deref(), Some("a"));
        assert_eq!(other.condition_index, Some(1));
        assert_eq!(other.payload, None);
    }

    #[test]
    fn reasons_follow_posthog() {
        let compiled = flag(
            "f",
            FlagFilters {
                groups: vec![
                    group(
                        None,
                        vec![filter("plan", PropertyOperator::Exact, json!("pro"))],
                    ),
                    group(Some(0.0), vec![]),
                ],
                ..FlagFilters::default()
            },
        );
        let miss = compiled.evaluate("u", &Map::new());
        assert_eq!(miss.reason, ReasonCode::OutOfRolloutBound);
        assert_eq!(miss.condition_index, Some(1));
        let hit = compiled.evaluate("u", &props(json!({"plan": "PRO"})));
        assert_eq!(
            (hit.enabled, hit.reason),
            (true, ReasonCode::ConditionMatch)
        );

        let none = flag(
            "f",
            FlagFilters {
                groups: vec![group(
                    None,
                    vec![filter("plan", PropertyOperator::Exact, json!("pro"))],
                )],
                ..FlagFilters::default()
            },
        )
        .evaluate("u", &Map::new());
        assert_eq!(
            (none.reason, none.condition_index),
            (ReasonCode::NoConditionMatch, None)
        );

        let mut disabled = flag("f", FlagFilters::default());
        disabled.flag.active = false;
        assert_eq!(
            disabled.evaluate("u", &Map::new()).reason,
            ReasonCode::Disabled
        );
        // No groups at all: nothing can match.
        assert!(
            !flag("f", FlagFilters::default())
                .evaluate("u", &Map::new())
                .enabled
        );
    }

    #[test]
    fn boolean_flag_payload_uses_true_key() {
        let mut payloads = std::collections::BTreeMap::new();
        payloads.insert("true".to_owned(), json!([1, 2]));
        let compiled = flag(
            "f",
            FlagFilters {
                groups: vec![group(Some(100.0), vec![])],
                payloads,
                ..FlagFilters::default()
            },
        );
        assert_eq!(
            compiled.evaluate("u", &Map::new()).payload,
            Some(json!([1, 2]))
        );
    }

    #[test]
    fn exact_and_is_not() {
        use PropertyOperator::{Exact, IsNot};
        assert!(matches(Exact, json!("Pro"), json!("pro")));
        assert!(matches(Exact, json!(["free", "PRO"]), json!("pro")));
        assert!(!matches(Exact, json!(["free"]), json!("pro")));
        assert!(matches(Exact, json!(5), json!("5")));
        assert!(matches(Exact, json!("5"), json!(5.0)));
        assert!(matches(Exact, json!(true), json!("TRUE")));
        assert!(matches(Exact, json!(["true"]), json!(true)));
        assert!(!matches(Exact, json!(false), json!(true)));
        assert!(matches(IsNot, json!("pro"), json!("free")));
        assert!(!matches(IsNot, json!(["pro", "team"]), json!("Team")));
        assert!(matches(IsNot, json!("pro"), Value::Null));
    }

    #[test]
    fn string_operators() {
        use PropertyOperator::{Icontains, NotIcontains, NotRegex, Regex};
        assert!(matches(Icontains, json!("CORP"), json!("a@corp.com")));
        assert!(!matches(Icontains, json!("x"), json!("a@corp.com")));
        assert!(matches(NotIcontains, json!("x"), json!("a@corp.com")));
        assert!(!matches(NotIcontains, json!("corp"), json!("a@CORP.com")));
        assert!(!matches(Icontains, json!("null"), Value::Null));
        assert!(matches(Regex, json!(r"^\d+$"), json!("123")));
        assert!(!matches(Regex, json!(r"^\d+$"), json!("12a")));
        assert!(matches(NotRegex, json!(r"^\d+$"), json!("12a")));
        // An invalid pattern never matches either way.
        assert!(!matches(Regex, json!("(unclosed"), json!("x")));
        assert!(!matches(NotRegex, json!("(unclosed"), json!("x")));
    }

    #[test]
    fn numeric_and_string_comparisons() {
        use PropertyOperator::{Gt, Gte, Lt, Lte};
        assert!(matches(Gt, json!(100), json!(500)));
        assert!(matches(Gt, json!("100"), json!("500")));
        assert!(!matches(Gt, json!(100), json!(50)));
        assert!(matches(Gte, json!(100), json!(100)));
        assert!(matches(Lt, json!(100), json!("99.5")));
        assert!(matches(Lte, json!(1), json!(1.0)));
        // Not both numeric → lexicographic.
        assert!(matches(Gt, json!("apple"), json!("banana")));
        assert!(!matches(Lt, json!("apple"), json!("banana")));
        assert!(!matches(Gt, json!(1), Value::Null));
    }

    #[test]
    fn set_operators_and_missing_properties() {
        use PropertyOperator::{Exact, Gt, Icontains, IsNot, IsNotSet, IsSet};
        let check = |operator, properties: Value| {
            flag(
                "f",
                FlagFilters {
                    groups: vec![group(None, vec![filter("p", operator, json!("x"))])],
                    ..FlagFilters::default()
                },
            )
            .evaluate("u", &props(properties))
            .enabled
        };
        assert!(check(IsSet, json!({"p": null})));
        assert!(!check(IsSet, json!({})));
        assert!(check(IsNotSet, json!({})));
        assert!(!check(IsNotSet, json!({"p": 1})));
        for operator in [Exact, IsNot, Icontains, Gt] {
            assert!(
                !check(operator, json!({})),
                "{operator:?} on a missing property"
            );
        }
    }

    #[test]
    fn date_operators() {
        use PropertyOperator::{IsDateAfter, IsDateBefore};
        assert!(matches(
            IsDateBefore,
            json!("2024-01-01"),
            json!("2023-12-31T23:00:00Z")
        ));
        assert!(!matches(
            IsDateBefore,
            json!("2024-01-01"),
            json!("2024-01-02")
        ));
        assert!(matches(
            IsDateAfter,
            json!("2024-01-01T00:00:00Z"),
            json!("2024-01-01 00:00:01")
        ));
        assert!(matches(
            IsDateAfter,
            json!("-7d"),
            json!(Utc::now().to_rfc3339())
        ));
        assert!(matches(IsDateBefore, json!("-1y"), json!("2000-01-01")));
        assert!(!matches(
            IsDateBefore,
            json!("garbage"),
            json!("2000-01-01")
        ));
        assert!(!matches(IsDateBefore, json!("-7d"), json!("not a date")));
        assert!(relative_date("-10000d", Utc::now()).is_none());
        assert!(relative_date("-3x", Utc::now()).is_none());
    }
}
