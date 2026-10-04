//! Event-property filters compiled to DuckDB predicates.
//!
//! Every user-supplied value and property key is a bound parameter; only
//! fixed column names and operators are written into SQL. Promoted columns
//! are read directly, everything else through `json_extract_string` with a
//! JSON Pointer path (so any key, including ones with quotes or dots, is
//! addressable without escaping SQL).

use duckdb::types::Value as Db;
use serde_json::Value;

use crate::contract::common::{PropertyFilter, PropertyOperator, PropertySource};
use crate::lake::parquet::promoted_column;

use super::ExploreError;

/// Most filters in one request.
pub const MAX_FILTERS: usize = 20;
/// Most values in one array-valued filter.
pub const MAX_FILTER_VALUES: usize = 100;
/// Longest key or value.
pub const MAX_FILTER_TEXT: usize = 1_000;
/// Longest encoded `properties` query parameter.
pub const MAX_FILTER_JSON: usize = 16 * 1024;

fn invalid(message: &'static str) -> ExploreError {
    ExploreError::Invalid {
        field: "properties",
        message,
    }
}

/// Parse the URL-decoded `properties` parameter.
pub fn parse(encoded: Option<&str>) -> Result<Vec<PropertyFilter>, ExploreError> {
    let Some(encoded) = encoded.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(Vec::new());
    };
    if encoded.len() > MAX_FILTER_JSON {
        return Err(invalid("The property filter list is too long."));
    }
    let filters: Vec<PropertyFilter> = serde_json::from_str(encoded)
        .map_err(|_| invalid("properties must be a JSON array of property filters."))?;
    if filters.len() > MAX_FILTERS {
        return Err(invalid("Too many property filters."));
    }
    Ok(filters)
}

/// A compiled `AND` of filters: SQL text plus its parameters, in order.
#[derive(Debug, Default)]
pub struct Compiled {
    pub sql: String,
    pub params: Vec<Db>,
    /// Regex patterns to validate before running the query.
    pub regexes: Vec<String>,
}

fn scalar_text(value: &Value) -> Result<String, ExploreError> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => {
            return Err(invalid(
                "Filter values must be strings, numbers, or booleans.",
            ));
        }
    };
    if text.len() > MAX_FILTER_TEXT {
        return Err(invalid("A filter value is too long."));
    }
    Ok(text)
}

fn values(value: &Value) -> Result<Vec<String>, ExploreError> {
    match value {
        Value::Array(items) => {
            if items.is_empty() || items.len() > MAX_FILTER_VALUES {
                return Err(invalid("A filter has too few or too many values."));
            }
            items.iter().map(scalar_text).collect()
        }
        other => Ok(vec![scalar_text(other)?]),
    }
}

fn number(value: &Value) -> Result<f64, ExploreError> {
    let parsed = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    parsed
        .filter(|number| number.is_finite())
        .ok_or_else(|| invalid("Comparison filters need a numeric value."))
}

/// JSON Pointer (RFC 6901) for a top-level key.
fn pointer(key: &str) -> String {
    format!("/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// Compile event-property filters against the table alias `alias`.
pub fn compile(
    filters: &[PropertyFilter],
    alias: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Compiled, ExploreError> {
    let mut out = Compiled::default();
    for filter in filters {
        if filter.source != PropertySource::Event {
            return Err(invalid(
                "Only event property filters are supported on this endpoint.",
            ));
        }
        if filter.key.is_empty() || filter.key.len() > MAX_FILTER_TEXT {
            return Err(invalid("A filter key is empty or too long."));
        }
        // Each use of the property expression binds its own key parameter, so
        // parameter order always follows the SQL text.
        let promoted = promoted_column(&filter.key);
        let key_pointer = pointer(&filter.key);
        let expr = |params: &mut Vec<Db>| match promoted {
            Some(column) => format!("{alias}.{column}"),
            None => {
                params.push(Db::Text(key_pointer.clone()));
                format!("json_extract_string({alias}.properties, ?)")
            }
        };
        let predicate = match filter.operator {
            PropertyOperator::Exact | PropertyOperator::IsNot => {
                let values = values(&filter.value)?;
                let expression = expr(&mut out.params);
                let marks = vec!["?"; values.len()].join(", ");
                out.params.extend(values.into_iter().map(Db::Text));
                if filter.operator == PropertyOperator::Exact {
                    format!("coalesce({expression} IN ({marks}), false)")
                } else {
                    format!("coalesce({expression} NOT IN ({marks}), true)")
                }
            }
            PropertyOperator::Icontains | PropertyOperator::NotIcontains => {
                let values = values(&filter.value)?;
                let mut tests = Vec::with_capacity(values.len());
                for value in values {
                    let expression = expr(&mut out.params);
                    out.params.push(Db::Text(value));
                    tests.push(format!("contains(lower({expression}), lower(?))"));
                }
                let test = format!("({})", tests.join(" OR "));
                if filter.operator == PropertyOperator::Icontains {
                    format!("coalesce({test}, false)")
                } else {
                    format!("NOT coalesce({test}, false)")
                }
            }
            PropertyOperator::Regex | PropertyOperator::NotRegex => {
                let pattern = scalar_text(&filter.value)?;
                let expression = expr(&mut out.params);
                out.regexes.push(pattern.clone());
                out.params.push(Db::Text(pattern));
                if filter.operator == PropertyOperator::Regex {
                    format!("coalesce(regexp_matches({expression}, ?), false)")
                } else {
                    format!("NOT coalesce(regexp_matches({expression}, ?), false)")
                }
            }
            PropertyOperator::Gt
            | PropertyOperator::Gte
            | PropertyOperator::Lt
            | PropertyOperator::Lte => {
                let op = match filter.operator {
                    PropertyOperator::Gt => ">",
                    PropertyOperator::Gte => ">=",
                    PropertyOperator::Lt => "<",
                    _ => "<=",
                };
                let threshold = number(&filter.value)?;
                let expression = expr(&mut out.params);
                out.params.push(Db::Double(threshold));
                format!("coalesce(TRY_CAST({expression} AS DOUBLE) {op} ?, false)")
            }
            PropertyOperator::IsSet => format!("{} IS NOT NULL", expr(&mut out.params)),
            PropertyOperator::IsNotSet => format!("{} IS NULL", expr(&mut out.params)),
            PropertyOperator::IsDateBefore | PropertyOperator::IsDateAfter => {
                let text = scalar_text(&filter.value)?;
                let range = super::dates::resolve(&text, None, now).map_err(|_| {
                    invalid("Date filters need an ISO 8601 or relative date value.")
                })?;
                let at = range
                    .from
                    .ok_or_else(|| invalid("Date filters need a concrete date."))?;
                let op = if filter.operator == PropertyOperator::IsDateBefore {
                    "<"
                } else {
                    ">"
                };
                let expression = expr(&mut out.params);
                out.params.push(Db::BigInt(at.timestamp_micros()));
                format!("coalesce(epoch_us(TRY_CAST({expression} AS TIMESTAMPTZ)) {op} ?, false)")
            }
        };
        out.sql.push_str(" AND ");
        out.sql.push_str(&predicate);
    }
    Ok(out)
}

/// Fail with a client error if any regex filter does not compile.
pub fn validate_regexes(
    connection: &duckdb::Connection,
    compiled: &Compiled,
) -> Result<(), ExploreError> {
    for pattern in &compiled.regexes {
        connection
            .query_row("SELECT regexp_matches('', ?)", [pattern], |_| Ok(()))
            .map_err(|_| invalid("A regex filter is not a valid RE2 pattern."))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_person_filters_and_oversized_input() {
        let person = parse(Some(r#"[{"key":"email","type":"person","value":"a"}]"#)).unwrap();
        assert!(compile(&person, "e", chrono::Utc::now()).is_err());
        let many = format!("[{}]", vec![r#"{"key":"a","value":"b"}"#; 21].join(","));
        assert!(parse(Some(&many)).is_err());
        assert!(parse(Some("{not json")).is_err());
        assert!(parse(Some("")).unwrap().is_empty());
    }

    #[test]
    fn pointer_escapes_reserved_characters() {
        assert_eq!(pointer("a/b~c"), "/a~1b~0c");
        assert_eq!(pointer("plan.tier"), "/plan.tier");
    }
}
