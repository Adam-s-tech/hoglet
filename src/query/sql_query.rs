//! Read-only SQL over one project's events, in a sandbox.
//!
//! Each query gets a fresh in-memory DuckDB with its own memory and thread
//! limits. The sandbox exposes one relation, `events`, over exactly this
//! project's files (person ids resolved), then disables external access —
//! `allowed_paths` keeps only those files readable — and locks its
//! configuration. `events.timestamp` is a UTC `TIMESTAMP` (no time zone) and
//! `now_utc()` is the current UTC time, so `date_trunc`, `extract`, casts and
//! interval arithmetic work without DuckDB's ICU extension, which the static
//! binary does not ship. The statement must parse as a single SELECT (DuckDB's own
//! parser via `json_serialize_sql`); results are capped at
//! [`MAX_SQL_ROWS`] rows and the engine watchdog interrupts it at the
//! deadline.

use std::sync::Arc;

use duckdb::Connection;
use duckdb::types::Value;
use serde_json::Value as Json;

use super::sql::{Params, string_literal};
use super::{Ctx, QueryError};
use crate::contract::insight::{InsightResult, SqlQuery};
use crate::lake::parquet::PROMOTED;

pub const MAX_SQL_ROWS: usize = 10_000;
/// Longest text one cell may carry; longer values are cut and marked.
pub const MAX_CELL_BYTES: usize = 64 * 1024;
/// Most bytes of cells one response may carry; past it `truncated` is set.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const TRUNCATION_MARK: &str = "…[truncated]";
/// Functions that reveal the host (settings, environment, file paths, view
/// definitions) or run SQL the validation never saw. Matched on the parsed
/// statement, case-insensitively; every `duckdb_*` and `pragma_*` table
/// function is refused too.
const DENIED_FUNCTIONS: &[&str] = &[
    "current_setting",
    "getenv",
    "glob",
    "query",
    "query_table",
    "json_execute_serialized_sql",
    "sniff_csv",
    "parquet_metadata",
    "parquet_schema",
    "parquet_file_metadata",
    "parquet_kv_metadata",
    "parquet_bloom_probe",
    "which_secret",
];
/// Explicit bound on one cell's JSON rendering.
const MAX_CELL_DEPTH: usize = 16;

fn sandbox(ctx: &Ctx<'_>) -> Result<Connection, QueryError> {
    let config = duckdb::Config::default()
        .max_memory(&format!("{}MB", ctx.sql_memory_limit_mb))?
        .threads(i64::from(ctx.threads))?
        .enable_autoload_extension(false)?;
    let conn = Connection::open_in_memory_with_flags(config)?;
    super::set_utc(&conn);
    // Spill under the data directory's `tmp/query` (absolute), or nowhere:
    // never DuckDB's default of a relative `.tmp`.
    match ctx.temp_directory {
        Some(directory) => conn.execute_batch(&format!(
            "SET temp_directory = {};",
            string_literal(&directory.to_string_lossy())?
        ))?,
        None => conn.execute_batch("SET temp_directory = '';")?,
    }
    conn.execute_batch(
        "SET autoinstall_known_extensions = false; \
         CREATE TEMP TABLE person_overrides (distinct_id VARCHAR, person_id VARCHAR); \
         CREATE TEMP MACRO now_utc() AS make_timestamp(epoch_us(now()));",
    )?;
    // This project's overrides, copied from the engine's synced table.
    let mut params = Params::new();
    let project = params.text(ctx.project_id);
    let mut statement = ctx.conn.prepare(&format!(
        "SELECT distinct_id, person_id FROM main.person_overrides WHERE project_id = {project}"
    ))?;
    let mut rows = statement.query(duckdb::params_from_iter(params.values()))?;
    {
        let mut appender = conn.appender_to_catalog_and_db("person_overrides", "temp", "main")?;
        while let Some(row) = rows.next()? {
            let distinct_id: String = row.get(0)?;
            let person_id: String = row.get(1)?;
            appender.append_row(duckdb::params![distinct_id, person_id])?;
        }
        appender.flush()?;
    }

    let mut columns = vec![
        "e.uuid AS uuid".to_owned(),
        "e.event AS event".to_owned(),
        "e.distinct_id AS distinct_id".to_owned(),
        "coalesce(o.person_id, e.distinct_id) AS person_id".to_owned(),
        "make_timestamp(epoch_us(e.timestamp)) AS timestamp".to_owned(),
        "e.properties AS properties".to_owned(),
    ];
    let files = ctx.source.files;
    if files.is_empty() {
        let mut empty = vec![
            "NULL::VARCHAR AS uuid".to_owned(),
            "NULL::VARCHAR AS event".to_owned(),
            "NULL::VARCHAR AS distinct_id".to_owned(),
            "NULL::VARCHAR AS person_id".to_owned(),
            "NULL::TIMESTAMP AS timestamp".to_owned(),
            "NULL::VARCHAR AS properties".to_owned(),
        ];
        for (column, _) in PROMOTED {
            empty.push(format!("NULL::VARCHAR AS {column}"));
        }
        conn.execute_batch(&format!(
            "CREATE TEMP VIEW events AS SELECT {} WHERE false",
            empty.join(", ")
        ))?;
    } else {
        for (index, (column, source)) in PROMOTED.iter().enumerate() {
            if ctx
                .source
                .schema
                .promoted_present
                .get(index)
                .copied()
                .unwrap_or(false)
            {
                columns.push(format!("e.{column} AS {column}"));
            } else {
                let pointer = super::sql::json_pointer_literal(source)?;
                columns.push(format!(
                    "NULLIF(CASE WHEN json_type(e.properties, {pointer}) IN ('OBJECT', 'ARRAY') \
                     THEN NULL ELSE json_extract_string(e.properties, {pointer}) END, '') AS {column}"
                ));
            }
        }
        let mut list = Vec::with_capacity(files.len());
        for path in files {
            list.push(string_literal(&path.to_string_lossy())?);
        }
        let list = list.join(", ");
        conn.execute_batch(&format!(
            "CREATE TEMP VIEW events AS SELECT {} \
             FROM read_parquet([{list}], union_by_name = true) e \
             LEFT JOIN person_overrides o ON o.distinct_id = e.distinct_id; \
             SET allowed_paths = [{list}];",
            columns.join(", ")
        ))?;
    }
    conn.execute_batch("SET enable_external_access = false; SET lock_configuration = true;")?;
    Ok(conn)
}

/// The statement must be exactly one SELECT (including `WITH … SELECT`).
fn validate(conn: &Connection, sql: &str) -> Result<String, QueryError> {
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    if trimmed.is_empty() {
        return Err(QueryError::invalid("SQL is empty"));
    }
    let parsed: String =
        conn.query_row("SELECT json_serialize_sql($1::VARCHAR)", [trimmed], |row| {
            row.get(0)
        })?;
    let parsed: Json = serde_json::from_str(&parsed)
        .map_err(|_| QueryError::internal("unreadable SQL parse result"))?;
    if parsed["error"].as_bool() == Some(true) {
        let message = parsed["error_message"]
            .as_str()
            .unwrap_or("invalid SQL")
            .to_owned();
        return Err(QueryError::invalid(if message.contains("Only SELECT") {
            "only a single SELECT statement is allowed".to_owned()
        } else {
            message
        }));
    }
    if let Some(name) = denied_function(&parsed) {
        return Err(QueryError::invalid(format!(
            "the function `{name}` is not available in SQL queries"
        )));
    }
    match parsed["statements"].as_array() {
        Some(statements) if statements.len() == 1 => Ok(trimmed.to_owned()),
        _ => Err(QueryError::invalid(
            "only a single SELECT statement is allowed",
        )),
    }
}

/// The first denied function (or table function) named anywhere in a parsed
/// statement.
fn denied_function(node: &Json) -> Option<String> {
    match node {
        Json::Object(map) => {
            if let Some(Json::String(name)) = map.get("function_name") {
                let lower = name.to_ascii_lowercase();
                if DENIED_FUNCTIONS.contains(&lower.as_str())
                    || lower.starts_with("duckdb_")
                    || lower.starts_with("pragma_")
                    || lower.starts_with("read_")
                {
                    return Some(lower);
                }
            }
            map.values().find_map(denied_function)
        }
        Json::Array(items) => items.iter().find_map(denied_function),
        _ => None,
    }
}

fn cap_text(mut text: String) -> String {
    if text.len() > MAX_CELL_BYTES {
        let mut end = MAX_CELL_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(TRUNCATION_MARK);
    }
    text
}

fn to_json(value: Value, depth: usize) -> Json {
    if depth > MAX_CELL_DEPTH {
        return Json::Null;
    }
    match value {
        Value::Null => Json::Null,
        Value::Boolean(flag) => Json::Bool(flag),
        Value::TinyInt(n) => Json::from(n),
        Value::SmallInt(n) => Json::from(n),
        Value::Int(n) => Json::from(n),
        Value::BigInt(n) => Json::from(n),
        Value::HugeInt(n) => i64::try_from(n)
            .map(Json::from)
            .unwrap_or_else(|_| Json::String(n.to_string())),
        Value::UTinyInt(n) => Json::from(n),
        Value::USmallInt(n) => Json::from(n),
        Value::UInt(n) => Json::from(n),
        Value::UBigInt(n) => Json::from(n),
        Value::Float(n) => serde_json::Number::from_f64(f64::from(n))
            .map(Json::Number)
            .unwrap_or(Json::Null),
        Value::Double(n) => serde_json::Number::from_f64(n)
            .map(Json::Number)
            .unwrap_or(Json::Null),
        Value::Decimal(n) => Json::String(n.to_string()),
        Value::Timestamp(unit, n) => {
            let micros = unit.to_micros(n);
            Json::String(super::range::rfc3339_micros(micros))
        }
        Value::Text(text) => Json::String(cap_text(text)),
        Value::Enum(text) => Json::String(cap_text(text)),
        Value::Blob(bytes) => {
            let shown = bytes.len().min(MAX_CELL_BYTES / 2);
            let mut text = hex::encode(&bytes[..shown]);
            if shown < bytes.len() {
                text.push_str(TRUNCATION_MARK);
            }
            Json::String(text)
        }
        Value::Date32(days) => Json::String(
            chrono::NaiveDate::from_num_days_from_ce_opt(days + 719_163)
                .map(|date| date.to_string())
                .unwrap_or_default(),
        ),
        Value::Time64(unit, n) => {
            let micros = unit.to_micros(n);
            Json::String(format!(
                "{:02}:{:02}:{:02}.{:06}",
                micros / 3_600_000_000,
                micros / 60_000_000 % 60,
                micros / 1_000_000 % 60,
                micros % 1_000_000
            ))
        }
        Value::Interval {
            months,
            days,
            nanos,
        } => Json::String(format!("{months} months {days} days {nanos} ns")),
        Value::List(items) | Value::Array(items) => Json::Array(
            items
                .into_iter()
                .map(|item| to_json(item, depth + 1))
                .collect(),
        ),
        Value::Struct(fields) => Json::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), to_json(value.clone(), depth + 1)))
                .collect(),
        ),
        Value::Map(entries) => Json::Array(
            entries
                .iter()
                .map(|(key, value)| {
                    Json::Array(vec![
                        to_json(key.clone(), depth + 1),
                        to_json(value.clone(), depth + 1),
                    ])
                })
                .collect(),
        ),
        Value::Union(inner) => to_json(*inner, depth + 1),
    }
}

pub(crate) fn run(ctx: &Ctx<'_>, q: &SqlQuery) -> Result<InsightResult, QueryError> {
    let conn = sandbox(ctx)?;
    // The engine watchdog only knows the pooled connection; interrupt the
    // sandbox at the same deadline.
    let handle: Arc<duckdb::InterruptHandle> = conn.interrupt_handle();
    let remaining = ctx
        .deadline
        .saturating_duration_since(std::time::Instant::now());
    let (stop, wait) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(remaining) {
            handle.interrupt();
        }
    });
    let result = execute(&conn, q);
    drop(stop);
    let _ = watchdog.join();
    if std::time::Instant::now() >= ctx.deadline && result.is_err() {
        return Err(QueryError::Timeout);
    }
    result
}

fn execute(conn: &Connection, q: &SqlQuery) -> Result<InsightResult, QueryError> {
    let sql = validate(conn, &q.query)?;
    let user_error = |error: duckdb::Error| {
        let message = error.to_string();
        if message.contains("Out of Memory") {
            QueryError::too_large("the SQL query exceeded its memory limit")
        } else {
            QueryError::invalid(message)
        }
    };
    let mut describe = conn
        .prepare(&format!("DESCRIBE SELECT * FROM (\n{sql}\n)"))
        .map_err(user_error)?;
    let described: Vec<(String, String)> = describe
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(user_error)?
        .collect::<Result<_, _>>()
        .map_err(user_error)?;
    let (columns, types): (Vec<String>, Vec<String>) = described.into_iter().unzip();
    let mut statement = conn
        .prepare(&format!(
            "SELECT * FROM (\n{sql}\n) LIMIT {}",
            MAX_SQL_ROWS + 1
        ))
        .map_err(user_error)?;
    let mut rows = statement.query([]).map_err(user_error)?;
    let mut out = Vec::new();
    let mut truncated = false;
    let mut bytes = 0_usize;
    while let Some(row) = rows.next().map_err(user_error)? {
        if out.len() == MAX_SQL_ROWS {
            truncated = true;
            break;
        }
        let mut cells = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let value: Value = row.get(index).map_err(user_error)?;
            let cell = to_json(value, 0);
            bytes = bytes.saturating_add(cell.to_string().len());
            cells.push(cell);
        }
        if bytes > MAX_RESPONSE_BYTES {
            truncated = true;
            break;
        }
        out.push(cells);
    }
    Ok(InsightResult::Sql {
        columns,
        types,
        rows: out,
        truncated,
    })
}
