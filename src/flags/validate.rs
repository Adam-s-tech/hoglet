//! Flag definition validation. Every limit here is explicit: a definition
//! that passes is bounded in size and cost to evaluate.

use std::collections::BTreeSet;

use crate::contract::common::PropertyOperator;
use crate::contract::flags::{FeatureFlagInput, FlagFilters};

pub const MAX_FLAGS_PER_PROJECT: usize = 2_000;
pub const MAX_KEY_BYTES: usize = 400;
pub const MAX_NAME_BYTES: usize = 4_096;
pub const MAX_GROUPS: usize = 50;
pub const MAX_PROPERTIES_PER_GROUP: usize = 50;
pub const MAX_VARIANTS: usize = 50;
pub const MAX_PROPERTY_KEY_BYTES: usize = 400;
/// One property filter value (scalar or array), serialized.
pub const MAX_FILTER_VALUE_BYTES: usize = 16 * 1024;
/// One payload, serialized.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
/// The whole `filters` object, serialized.
pub const MAX_FILTERS_BYTES: usize = 512 * 1024;

/// Allowed rounding slack when variant percentages must sum to 100.
const VARIANT_SUM_EPSILON: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub field: String,
    pub message: String,
}

impl Invalid {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

pub fn validate_input(input: &FeatureFlagInput) -> Result<(), Invalid> {
    validate_key(&input.key)?;
    validate_name(&input.name)?;
    validate_filters(&input.filters)
}

/// PostHog's key rule: letters, digits, `-` and `_`.
pub fn validate_key(key: &str) -> Result<(), Invalid> {
    if key.is_empty() {
        return Err(Invalid::new("key", "must not be empty"));
    }
    if key.len() > MAX_KEY_BYTES {
        return Err(Invalid::new(
            "key",
            format!("must be at most {MAX_KEY_BYTES} bytes"),
        ));
    }
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(Invalid::new(
            "key",
            "may only contain letters, numbers, hyphens (-) and underscores (_)",
        ));
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<(), Invalid> {
    if name.len() > MAX_NAME_BYTES {
        return Err(Invalid::new(
            "name",
            format!("must be at most {MAX_NAME_BYTES} bytes"),
        ));
    }
    Ok(())
}

pub fn validate_filters(filters: &FlagFilters) -> Result<(), Invalid> {
    let encoded =
        serde_json::to_vec(filters).map_err(|error| Invalid::new("filters", error.to_string()))?;
    if encoded.len() > MAX_FILTERS_BYTES {
        return Err(Invalid::new(
            "filters",
            format!("must be at most {MAX_FILTERS_BYTES} bytes"),
        ));
    }
    if filters.groups.len() > MAX_GROUPS {
        return Err(Invalid::new(
            "filters.groups",
            format!("at most {MAX_GROUPS} condition groups"),
        ));
    }

    let mut variant_keys = BTreeSet::new();
    if let Some(multivariate) = &filters.multivariate {
        if multivariate.variants.is_empty() || multivariate.variants.len() > MAX_VARIANTS {
            return Err(Invalid::new(
                "filters.multivariate.variants",
                format!("must have between 1 and {MAX_VARIANTS} variants"),
            ));
        }
        let mut sum = 0.0;
        for (index, variant) in multivariate.variants.iter().enumerate() {
            let field = format!("filters.multivariate.variants[{index}]");
            validate_key(&variant.key)
                .map_err(|error| Invalid::new(format!("{field}.key"), error.message))?;
            if let Some(name) = &variant.name {
                validate_name(name)
                    .map_err(|error| Invalid::new(format!("{field}.name"), error.message))?;
            }
            percentage(
                variant.rollout_percentage,
                &format!("{field}.rollout_percentage"),
            )?;
            if !variant_keys.insert(variant.key.as_str()) {
                return Err(Invalid::new(
                    format!("{field}.key"),
                    "variant keys must be unique",
                ));
            }
            sum += variant.rollout_percentage;
        }
        if (sum - 100.0).abs() > VARIANT_SUM_EPSILON {
            return Err(Invalid::new(
                "filters.multivariate.variants",
                "variant rollout percentages must sum to 100",
            ));
        }
    }

    for (group_index, group) in filters.groups.iter().enumerate() {
        let field = format!("filters.groups[{group_index}]");
        if let Some(rollout) = group.rollout_percentage {
            percentage(rollout, &format!("{field}.rollout_percentage"))?;
        }
        if let Some(variant) = &group.variant
            && !variant_keys.contains(variant.as_str())
        {
            return Err(Invalid::new(
                format!("{field}.variant"),
                "must name one of the flag's variants",
            ));
        }
        if group.properties.len() > MAX_PROPERTIES_PER_GROUP {
            return Err(Invalid::new(
                format!("{field}.properties"),
                format!("at most {MAX_PROPERTIES_PER_GROUP} property filters"),
            ));
        }
        for (property_index, filter) in group.properties.iter().enumerate() {
            let field = format!("{field}.properties[{property_index}]");
            if filter.key.is_empty() || filter.key.len() > MAX_PROPERTY_KEY_BYTES {
                return Err(Invalid::new(
                    format!("{field}.key"),
                    format!("must be 1 to {MAX_PROPERTY_KEY_BYTES} bytes"),
                ));
            }
            let value_bytes = serde_json::to_vec(&filter.value)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if value_bytes > MAX_FILTER_VALUE_BYTES {
                return Err(Invalid::new(
                    format!("{field}.value"),
                    format!("must be at most {MAX_FILTER_VALUE_BYTES} bytes"),
                ));
            }
            match filter.operator {
                PropertyOperator::Regex | PropertyOperator::NotRegex
                    if super::eval::compile_regex(&super::eval::value_to_string(&filter.value))
                        .is_none() =>
                {
                    return Err(Invalid::new(
                        format!("{field}.value"),
                        "is not a valid regular expression",
                    ));
                }
                PropertyOperator::IsDateBefore | PropertyOperator::IsDateAfter
                    if super::eval::flag_date(&filter.value).is_none() =>
                {
                    return Err(Invalid::new(
                        format!("{field}.value"),
                        "must be a date (2024-01-31) or relative date (-7d)",
                    ));
                }
                _ => {}
            }
        }
    }

    for (key, payload) in &filters.payloads {
        let allowed = if filters.multivariate.is_some() {
            variant_keys.contains(key.as_str())
        } else {
            key == "true"
        };
        if !allowed {
            return Err(Invalid::new(
                format!("filters.payloads.{key}"),
                "payload keys must be \"true\" for boolean flags or a variant key",
            ));
        }
        let bytes = serde_json::to_vec(payload)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if bytes > MAX_PAYLOAD_BYTES {
            return Err(Invalid::new(
                format!("filters.payloads.{key}"),
                format!("must be at most {MAX_PAYLOAD_BYTES} bytes"),
            ));
        }
    }
    Ok(())
}

fn percentage(value: f64, field: &str) -> Result<(), Invalid> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(Invalid::new(field, "must be a number from 0 through 100"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::flags::{FlagConditionGroup, FlagVariant, Multivariate};
    use serde_json::json;

    fn input(filters: Value) -> FeatureFlagInput {
        serde_json::from_value(json!({"key": "my-flag", "filters": filters})).unwrap()
    }
    use serde_json::Value;

    #[test]
    fn keys_follow_posthog_format() {
        assert!(validate_key("new_checkout-2").is_ok());
        for bad in ["", "has space", "dot.key", "ünïcode", &"k".repeat(401)] {
            assert!(validate_key(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn variants_must_sum_to_100_and_be_unique() {
        let ok = json!({"groups": [{"rollout_percentage": 100}],
            "multivariate": {"variants": [
                {"key": "a", "rollout_percentage": 33.33},
                {"key": "b", "rollout_percentage": 33.33},
                {"key": "c", "rollout_percentage": 33.34}]}});
        assert!(validate_input(&input(ok)).is_ok());
        let short = json!({"multivariate": {"variants": [{"key": "a", "rollout_percentage": 90}]}});
        assert_eq!(
            validate_input(&input(short)).unwrap_err().field,
            "filters.multivariate.variants"
        );
        let duplicate = json!({"multivariate": {"variants": [
            {"key": "a", "rollout_percentage": 50}, {"key": "a", "rollout_percentage": 50}]}});
        assert!(validate_input(&input(duplicate)).is_err());
    }

    #[test]
    fn rollouts_overrides_and_payload_keys_are_checked() {
        let rollout = json!({"groups": [{"rollout_percentage": 101}]});
        assert_eq!(
            validate_input(&input(rollout)).unwrap_err().field,
            "filters.groups[0].rollout_percentage"
        );
        let override_without_variant = json!({"groups": [{"variant": "x"}]});
        assert!(validate_input(&input(override_without_variant)).is_err());
        let wrong_payload = json!({"payloads": {"control": 1}});
        assert!(validate_input(&input(wrong_payload)).is_err());
        let ok_payload = json!({"payloads": {"true": {"a": 1}}});
        assert!(validate_input(&input(ok_payload)).is_ok());
        let regex = json!({"groups": [{"properties": [
            {"key": "email", "type": "person", "operator": "regex", "value": "(unclosed"}]}]});
        assert!(validate_input(&input(regex)).is_err());
        let date = json!({"groups": [{"properties": [
            {"key": "signup", "operator": "is_date_after", "value": "-7d"}]}]});
        assert!(validate_input(&input(date)).is_ok());
    }

    #[test]
    fn sizes_are_bounded() {
        let groups: Vec<FlagConditionGroup> = (0..=MAX_GROUPS)
            .map(|_| FlagConditionGroup {
                properties: vec![],
                rollout_percentage: None,
                variant: None,
            })
            .collect();
        let filters = FlagFilters {
            groups,
            ..FlagFilters::default()
        };
        assert!(validate_filters(&filters).is_err());
        let mut payloads = std::collections::BTreeMap::new();
        payloads.insert("true".to_owned(), json!("x".repeat(MAX_PAYLOAD_BYTES)));
        assert!(
            validate_filters(&FlagFilters {
                payloads,
                ..FlagFilters::default()
            })
            .is_err()
        );
        let variants = (0..=MAX_VARIANTS)
            .map(|index| FlagVariant {
                key: format!("v{index}"),
                name: None,
                rollout_percentage: 0.0,
            })
            .collect();
        assert!(
            validate_filters(&FlagFilters {
                multivariate: Some(Multivariate { variants }),
                ..FlagFilters::default()
            })
            .is_err()
        );
    }
}
