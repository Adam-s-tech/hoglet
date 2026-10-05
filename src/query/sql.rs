//! SQL text generation with one rule: no request value is ever spliced into
//! SQL text.
//!
//! Event names, property values, numbers and instants are bound parameters
//! (`$n`). The only request-derived text that reaches SQL is a property *key*,
//! and only inside a JSON Pointer (RFC 6901) string literal built by
//! [`json_pointer_literal`], which escapes `~`, `/` and `'` and rejects NUL.
//! File paths come from the [`EventSource`](crate::source::EventSource), never
//! from a request, and are quoted the same way.

use std::path::PathBuf;

use duckdb::types::{TimeUnit, Value};

use super::QueryError;
use super::range::{DAY_US, HOUR_US, WEEK_ORIGIN_US, WEEK_US};
use crate::contract::common::{Interval, PropertyFilter, PropertyOperator, PropertySource};
use crate::contract::insight::EventNode;
use crate::lake::parquet::{PROMOTED, promoted_column};

/// Explicit bounds on request-derived SQL inputs.
pub const MAX_KEY_BYTES: usize = 400;
pub const MAX_VALUE_BYTES: usize = 2_000;
pub const MAX_FILTER_VALUES: usize = 100;
pub const MAX_EVENT_NAME_BYTES: usize = 400;

/// The bucket other breakdown values fold into, and the missing-value bucket.
pub use crate::contract::common::{BREAKDOWN_NONE, BREAKDOWN_OTHER};

/// Strict decimal syntax a property must have to count as numeric.
pub const NUMERIC_PATTERN: &str = r"-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?";

/// Positional parameters for one statement.
#[derive(Default)]
pub struct Params {
    values: Vec<Value>,
}

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a value; returns its placeholder.
    pub fn bind(&mut self, value: Value) -> String {
        self.values.push(value);
        format!("${}", self.values.len())
    }

    pub fn text(&mut self, text: &str) -> String {
        self.bind(Value::Text(text.to_owned()))
    }

    pub fn int(&mut self, value: i64) -> String {
        self.bind(Value::BigInt(value))
    }

    pub fn double(&mut self, value: f64) -> String {
        self.bind(Value::Double(value))
    }

    /// An instant (µs) usable against `TIMESTAMPTZ` columns.
    pub fn instant(&mut self, us: i64) -> String {
        format!(
            "{}::TIMESTAMPTZ",
            self.bind(Value::Timestamp(TimeUnit::Microsecond, us))
        )
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }
}

/// A SQL string literal. Only for server-controlled text and escaped JSON
/// pointers; request values are always bound.
pub fn string_literal(text: &str) -> Result<String, QueryError> {
    if text.contains('\0') {
        return Err(QueryError::invalid("text must not contain NUL characters"));
    }
    Ok(format!("'{}'", text.replace('\'', "''")))
}

/// RFC 6901 pointer to a top-level member, as a SQL literal.
pub fn json_pointer_literal(key: &str) -> Result<String, QueryError> {
    validate_key(key)?;
    let pointer = format!("/{}", key.replace('~', "~0").replace('/', "~1"));
    string_literal(&pointer)
}

pub fn validate_key(key: &str) -> Result<(), QueryError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(QueryError::invalid(format!(
            "property keys are limited to {MAX_KEY_BYTES} bytes"
        )));
    }
    if key.contains('\0') {
        return Err(QueryError::invalid("property keys must not contain NUL"));
    }
    Ok(())
}

/// Which promoted columns physically exist in a file set. Legacy files
/// predate promotion; for them the column is derived from the JSON with the
/// promotion's exact semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSchema {
    pub promoted_present: Vec<bool>,
}

impl EventSchema {
    pub fn complete() -> Self {
        Self {
            promoted_present: vec![true; PROMOTED.len()],
        }
    }

    pub fn from_columns(columns: &[String]) -> Self {
        Self {
            promoted_present: PROMOTED
                .iter()
                .map(|(column, _)| columns.iter().any(|name| name == column))
                .collect(),
        }
    }
}

/// The event relation of one query: files, identity, person properties.
pub struct Source<'a> {
    pub files: &'a [PathBuf],
    pub schema: &'a EventSchema,
    pub project_id: &'a str,
    /// Person property keys loaded into temp tables `pp_<i>`.
    pub person_keys: &'a [String],
}

impl Source<'_> {
    /// A relation with columns `event, ts (µs), uuid, distinct_id,
    /// properties, <promoted…>, [person_id, pp_<i>…]` restricted to
    /// `[from, to)` when given. `person_id` (distinct id resolved through
    /// identity overrides) is present when `with_person` is set or person
    /// properties are loaded; it is never silently the raw distinct id.
    pub fn relation(
        &self,
        params: &mut Params,
        from: Option<i64>,
        to: Option<i64>,
        with_person: bool,
    ) -> Result<String, QueryError> {
        let with_person = with_person || !self.person_keys.is_empty();
        let mut columns = vec![
            "e.event AS event".to_owned(),
            "epoch_us(e.timestamp) AS ts".to_owned(),
            "e.uuid AS uuid".to_owned(),
            "e.distinct_id AS distinct_id".to_owned(),
            "e.properties AS properties".to_owned(),
        ];
        if self.files.is_empty() {
            let mut empty = vec![
                "NULL::VARCHAR AS event".to_owned(),
                "NULL::BIGINT AS ts".to_owned(),
                "NULL::VARCHAR AS uuid".to_owned(),
                "NULL::VARCHAR AS distinct_id".to_owned(),
                "NULL::VARCHAR AS properties".to_owned(),
            ];
            for (column, _) in PROMOTED {
                empty.push(format!("NULL::VARCHAR AS {column}"));
            }
            empty.push("NULL::VARCHAR AS person_id".to_owned());
            for index in 0..self.person_keys.len() {
                empty.push(format!("NULL::VARCHAR AS pp_{index}"));
            }
            return Ok(format!("SELECT {} WHERE false", empty.join(", ")));
        }
        for (index, (column, source)) in PROMOTED.iter().enumerate() {
            if self
                .schema
                .promoted_present
                .get(index)
                .copied()
                .unwrap_or(false)
            {
                columns.push(format!("e.{column} AS {column}"));
            } else {
                let pointer = json_pointer_literal(source)?;
                columns.push(format!(
                    "NULLIF(CASE WHEN json_type(e.properties, {pointer}) IN ('OBJECT', 'ARRAY') \
                     THEN NULL ELSE json_extract_string(e.properties, {pointer}) END, '') AS {column}"
                ));
            }
        }
        let person = "coalesce(o.person_id, e.distinct_id)";
        let mut joins = String::new();
        if with_person {
            columns.push(format!("{person} AS person_id"));
            joins = format!(
                " LEFT JOIN (SELECT distinct_id, person_id FROM main.person_overrides \
                 WHERE project_id = {}) o ON o.distinct_id = e.distinct_id",
                params.text(self.project_id)
            );
        }
        for index in 0..self.person_keys.len() {
            columns.push(format!("pp{index}.value AS pp_{index}"));
            joins.push_str(&format!(
                " LEFT JOIN pp_{index} pp{index} ON pp{index}.person_id = {person}"
            ));
        }
        let mut files = Vec::with_capacity(self.files.len());
        for path in self.files {
            files.push(string_literal(&path.to_string_lossy())?);
        }
        let mut conditions = Vec::new();
        if let Some(from) = from {
            conditions.push(format!("e.timestamp >= {}", params.instant(from)));
        }
        if let Some(to) = to {
            conditions.push(format!("e.timestamp < {}", params.instant(to)));
        }
        let filter = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        Ok(format!(
            "SELECT {} FROM read_parquet([{}], union_by_name = true) e{joins}{filter}",
            columns.join(", "),
            files.join(", ")
        ))
    }

    /// Text of a property on the `ev` relation (`NULL` when missing).
    pub fn property_text(&self, source: PropertySource, key: &str) -> Result<String, QueryError> {
        validate_key(key)?;
        match source {
            PropertySource::Event => Ok(match promoted_column(key) {
                Some(column) => column.to_owned(),
                None => format!(
                    "json_extract_string(properties, {})",
                    json_pointer_literal(key)?
                ),
            }),
            PropertySource::Person => {
                let index = self
                    .person_keys
                    .iter()
                    .position(|loaded| loaded == key)
                    .ok_or_else(|| QueryError::internal("person property was not loaded"))?;
                Ok(format!("pp_{index}"))
            }
        }
    }

    /// `TRUE` when every filter holds.
    pub fn filters(
        &self,
        filters: &[PropertyFilter],
        params: &mut Params,
    ) -> Result<String, QueryError> {
        let mut parts = vec!["TRUE".to_owned()];
        for filter in filters {
            parts.push(self.filter(filter, params)?);
        }
        Ok(parts.join(" AND "))
    }

    pub fn filter(
        &self,
        filter: &PropertyFilter,
        params: &mut Params,
    ) -> Result<String, QueryError> {
        let text = self.property_text(filter.source, &filter.key)?;
        let positive = |sql: String| format!("coalesce({sql}, false)");
        let negative = |sql: String| format!("(NOT coalesce({sql}, false))");
        Ok(match filter.operator {
            PropertyOperator::Exact | PropertyOperator::IsNot => {
                let values = filter_texts(&filter.value)?;
                let placeholders: Vec<String> =
                    values.iter().map(|value| params.text(value)).collect();
                let test = format!("{text} IN ({})", placeholders.join(", "));
                if filter.operator == PropertyOperator::Exact {
                    positive(test)
                } else {
                    negative(test)
                }
            }
            PropertyOperator::Icontains | PropertyOperator::NotIcontains => {
                let value = filter_scalar(&filter.value)?;
                let test = format!("contains(lower({text}), lower({}))", params.text(&value));
                if filter.operator == PropertyOperator::Icontains {
                    positive(test)
                } else {
                    negative(test)
                }
            }
            PropertyOperator::Regex | PropertyOperator::NotRegex => {
                let value = filter_scalar(&filter.value)?;
                let test = format!("regexp_matches({text}, {})", params.text(&value));
                if filter.operator == PropertyOperator::Regex {
                    positive(test)
                } else {
                    negative(test)
                }
            }
            PropertyOperator::Gt
            | PropertyOperator::Gte
            | PropertyOperator::Lt
            | PropertyOperator::Lte => {
                let value = filter_number(&filter.value)?;
                let op = match filter.operator {
                    PropertyOperator::Gt => ">",
                    PropertyOperator::Gte => ">=",
                    PropertyOperator::Lt => "<",
                    _ => "<=",
                };
                positive(format!(
                    "{} {op} {}",
                    numeric_expr(&text),
                    params.double(value)
                ))
            }
            PropertyOperator::IsSet => format!("({text} IS NOT NULL)"),
            PropertyOperator::IsNotSet => format!("({text} IS NULL)"),
            PropertyOperator::IsDateBefore | PropertyOperator::IsDateAfter => {
                let instant = filter_instant(&filter.value)?;
                let op = if filter.operator == PropertyOperator::IsDateBefore {
                    "<"
                } else {
                    ">"
                };
                positive(format!(
                    "TRY_CAST({text} AS TIMESTAMPTZ) {op} {}",
                    params.instant(instant)
                ))
            }
        })
    }
}

/// Event-name match of a series or step.
pub fn event_match(node: &EventNode, params: &mut Params) -> String {
    match &node.event {
        Some(name) => format!("(event = {})", params.text(name)),
        None => "TRUE".to_owned(),
    }
}

/// Numeric value of a property text, `NULL` unless strictly decimal.
pub fn numeric_expr(text: &str) -> String {
    format!(
        "(CASE WHEN regexp_full_match({text}, '{NUMERIC_PATTERN}') THEN TRY_CAST({text} AS DOUBLE) END)"
    )
}

/// Breakdown text: missing and empty values become `$$_none`.
pub fn breakdown_expr(text: &str) -> String {
    format!("coalesce(NULLIF({text}, ''), '{BREAKDOWN_NONE}')")
}

/// Start (µs) of the bucket containing `ts`.
pub fn bucket_expr(interval: Interval, ts: &str) -> String {
    match interval {
        Interval::Hour => floor_expr(ts, HOUR_US, 0),
        Interval::Day => floor_expr(ts, DAY_US, 0),
        Interval::Week => floor_expr(ts, WEEK_US, WEEK_ORIGIN_US),
        Interval::Month => format!("epoch_us(date_trunc('month', make_timestamp({ts})))"),
    }
}

fn floor_expr(ts: &str, unit: i64, origin: i64) -> String {
    format!("({ts} - ((({ts} - ({origin})) % {unit}) + {unit}) % {unit})")
}

/// Text form of a scalar filter value.
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn check_value_size(text: &str) -> Result<(), QueryError> {
    if text.len() > MAX_VALUE_BYTES {
        return Err(QueryError::invalid(format!(
            "filter values are limited to {MAX_VALUE_BYTES} bytes"
        )));
    }
    Ok(())
}

pub fn filter_texts(value: &serde_json::Value) -> Result<Vec<String>, QueryError> {
    let values = match value {
        serde_json::Value::Array(items) => {
            if items.is_empty() || items.len() > MAX_FILTER_VALUES {
                return Err(QueryError::invalid(format!(
                    "filter value lists need 1 to {MAX_FILTER_VALUES} values"
                )));
            }
            items
                .iter()
                .map(|item| {
                    scalar_text(item)
                        .ok_or_else(|| QueryError::invalid("filter values must be scalars"))
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        other => vec![
            scalar_text(other).ok_or_else(|| QueryError::invalid("filter value is required"))?,
        ],
    };
    for text in &values {
        check_value_size(text)?;
    }
    Ok(values)
}

pub fn filter_scalar(value: &serde_json::Value) -> Result<String, QueryError> {
    let text = scalar_text(value)
        .ok_or_else(|| QueryError::invalid("this operator needs a single scalar value"))?;
    check_value_size(&text)?;
    Ok(text)
}

/// Strict decimal parse shared by filters and the numeric maths.
pub fn parse_numeric(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut index = 0;
    if bytes.first() == Some(&b'-') {
        index += 1;
    }
    let digits = |index: &mut usize| {
        let start = *index;
        while *index < bytes.len() && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        *index > start
    };
    if !digits(&mut index) {
        return None;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        if !digits(&mut index) {
            return None;
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        if !digits(&mut index) {
            return None;
        }
    }
    if index != bytes.len() {
        return None;
    }
    text.parse().ok()
}

fn filter_number(value: &serde_json::Value) -> Result<f64, QueryError> {
    let number = match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => parse_numeric(text),
        _ => None,
    };
    number
        .filter(|number| number.is_finite())
        .ok_or_else(|| QueryError::invalid("numeric operators need a numeric value"))
}

fn filter_instant(value: &serde_json::Value) -> Result<i64, QueryError> {
    value
        .as_str()
        .and_then(super::range::parse_instant)
        .ok_or_else(|| QueryError::invalid("date operators need an ISO 8601 date or datetime"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_literals_escape_everything_hostile() {
        assert_eq!(json_pointer_literal("plan").unwrap(), "'/plan'");
        assert_eq!(json_pointer_literal("a/b~c").unwrap(), "'/a~1b~0c'");
        assert_eq!(
            json_pointer_literal("'); DROP TABLE x; --").unwrap(),
            "'/''); DROP TABLE x; --'"
        );
        assert_eq!(json_pointer_literal("$.a\"b").unwrap(), "'/$.a\"b'");
        assert!(json_pointer_literal("a\0b").is_err());
        assert!(json_pointer_literal(&"k".repeat(MAX_KEY_BYTES + 1)).is_err());
    }

    #[test]
    fn strict_numeric_syntax() {
        for (text, expected) in [
            ("12", Some(12.0)),
            ("-0.25", Some(-0.25)),
            ("1e3", Some(1000.0)),
            ("2.5E-1", Some(0.25)),
            (" 12", None),
            ("12.", None),
            (".5", None),
            ("abc", None),
            ("true", None),
            ("1_000", None),
            ("", None),
            ("-", None),
        ] {
            assert_eq!(parse_numeric(text), expected, "{text}");
        }
    }
}
