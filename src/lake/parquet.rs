//! Parquet encoding of captured events (event schema v2).
//!
//! Fixed columns cover the fields every query touches. The web-analytics
//! dimensions are promoted out of the JSON blob into their own columns so the
//! common dashboards never parse JSON per row; everything else stays in the
//! `properties` JSON string. DuckDB reads these files directly and the
//! compactor rewrites them with DuckDB, so readers always use
//! `union_by_name` — the schema may only ever grow.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use serde_json::{Map, Value};

use crate::capture::event::CapturedEvent;

/// On-disk event schema version, stamped into every file's key-value metadata.
/// Additive columns do not bump it.
pub const SCHEMA_VERSION: &str = "2";

/// Promoted string columns: `(column, source property)`. Order is the column
/// order on disk. Never remove or rename an entry; append only.
pub const PROMOTED: &[(&str, &str)] = &[
    ("session_id", "$session_id"),
    ("current_url", "$current_url"),
    ("pathname", "$pathname"),
    ("host", "$host"),
    ("referrer", "$referrer"),
    ("referring_domain", "$referring_domain"),
    ("browser", "$browser"),
    ("os", "$os"),
    ("device_type", "$device_type"),
    ("country", "$geoip_country_code"),
    ("utm_source", "utm_source"),
    ("utm_medium", "utm_medium"),
    ("utm_campaign", "utm_campaign"),
    ("lib", "$lib"),
];

/// Column holding the promoted value for an event property, if any.
pub fn promoted_column(property: &str) -> Option<&'static str> {
    PROMOTED
        .iter()
        .find(|(_, source)| *source == property)
        .map(|(column, _)| *column)
}

pub fn schema() -> Arc<Schema> {
    let mut fields = vec![
        Field::new("uuid", DataType::Utf8, false),
        Field::new("event", DataType::Utf8, false),
        Field::new("distinct_id", DataType::Utf8, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new("properties", DataType::Utf8, false),
    ];
    for (column, _) in PROMOTED {
        fields.push(Field::new(*column, DataType::Utf8, true));
    }
    Arc::new(Schema::new(fields))
}

/// Text form of a promoted property. Non-string scalars are stringified so a
/// numeric `utm_campaign` still groups; containers are not promotable.
fn promoted_value(properties: &Map<String, Value>, key: &str) -> Option<String> {
    match properties.get(key)? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn to_record_batch(events: &[CapturedEvent]) -> Result<RecordBatch, arrow::error::ArrowError> {
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            events.iter().map(|e| e.uuid.to_string()),
        )),
        Arc::new(StringArray::from_iter_values(
            events.iter().map(|e| e.event.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            events.iter().map(|e| e.distinct_id.as_str()),
        )),
        Arc::new(
            TimestampMicrosecondArray::from_iter_values(
                events.iter().map(|e| e.timestamp.timestamp_micros()),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(StringArray::from_iter_values(events.iter().map(|e| {
            serde_json::to_string(&e.properties).unwrap_or_else(|_| "{}".to_owned())
        }))),
    ];
    for (_, source) in PROMOTED {
        columns.push(Arc::new(StringArray::from(
            events
                .iter()
                .map(|e| promoted_value(&e.properties, source))
                .collect::<Vec<_>>(),
        )));
    }
    RecordBatch::try_new(schema(), columns)
}

/// Write `events` to `path` and fsync it. The caller owns atomic publication
/// (temporary name, rename, directory fsync).
pub fn write_file(events: &[CapturedEvent], path: &Path) -> std::io::Result<u64> {
    let batch = to_record_batch(events).map_err(std::io::Error::other)?;
    let file = File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .set_key_value_metadata(Some(vec![parquet::file::metadata::KeyValue::new(
            "hoglet_schema_version".to_string(),
            SCHEMA_VERSION.to_string(),
        )]))
        .build();
    let mut writer =
        ArrowWriter::try_new(file, schema(), Some(props)).map_err(std::io::Error::other)?;
    writer.write(&batch).map_err(std::io::Error::other)?;
    let file = writer.into_inner().map_err(std::io::Error::other)?;
    file.sync_all()?;
    Ok(file.metadata()?.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn promoted_columns_are_written_and_readable_by_duckdb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.parquet");
        let mut properties = Map::new();
        properties.insert("$pathname".into(), Value::String("/pricing".into()));
        properties.insert("$session_id".into(), Value::String("s1".into()));
        properties.insert("utm_campaign".into(), Value::from(42));
        let event = CapturedEvent {
            uuid: Uuid::now_v7(),
            event: "$pageview".into(),
            distinct_id: "u1".into(),
            token: "phc_t".into(),
            timestamp: Utc::now(),
            properties,
        };
        write_file(&[event], &path).unwrap();

        let duck = duckdb::Connection::open_in_memory().unwrap();
        let (pathname, session, campaign, browser): (String, String, String, Option<String>) = duck
            .query_row(
                &format!(
                    "SELECT pathname, session_id, utm_campaign, browser FROM read_parquet('{}')",
                    path.display()
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(pathname, "/pricing");
        assert_eq!(session, "s1");
        assert_eq!(campaign, "42");
        assert_eq!(browser, None);
    }
}
