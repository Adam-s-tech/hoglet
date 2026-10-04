//! `hoglet import posthog` — bring a PostHog project's history to Hoglet.
//!
//! The importer is an ordinary client of both systems: it reads PostHog
//! through its public query API with a personal API key, and writes to Hoglet
//! through Hoglet's own capture endpoint (`/batch/`) with the project token —
//! the same path a PostHog SDK uses. It therefore works against any running
//! Hoglet, local or remote, and every imported event takes the durable path.
//!
//! What moves:
//! - events, oldest first, keyset-paged by `(timestamp, uuid)`; resumable
//!   from a checkpoint file; `uuid`s are kept, so re-running never duplicates
//!   (Hoglet's compactor drops repeated uuids);
//! - identity: every person's distinct ids are merged with
//!   `$merge_dangerously`, then the person's current properties are `$set`;
//! - feature flags (optional, needs a Hoglet personal API key and project
//!   id), created through Hoglet's flag API with PostHog's own filters.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};

/// Rows per PostHog query page.
pub const PAGE_ROWS: usize = 5_000;
/// Events per Hoglet `/batch/` request.
pub const BATCH_EVENTS: usize = 500;
/// Upper bound on retries of one HTTP call.
const MAX_ATTEMPTS: u32 = 8;

#[derive(Debug, Clone)]
pub struct ImportConfig {
    /// e.g. `https://us.posthog.com`
    pub posthog_host: String,
    pub posthog_project_id: String,
    pub posthog_key: String,
    /// e.g. `http://localhost:8000`
    pub hoglet_host: String,
    pub hoglet_token: String,
    /// Optional: enables flag import.
    pub hoglet_key: Option<String>,
    pub hoglet_project_id: Option<String>,
    /// Only events at or after this ISO date.
    pub since: Option<String>,
    pub checkpoint: PathBuf,
    pub skip_events: bool,
    pub skip_persons: bool,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
    /// Last imported `(timestamp, uuid)`.
    after: Option<(String, String)>,
    events: u64,
    events_done: bool,
    persons_done: bool,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub events: u64,
    pub persons: u64,
    pub flags: u64,
}

pub struct Importer {
    config: ImportConfig,
    agent: ureq::Agent,
    log: Box<dyn FnMut(&str) + Send>,
}

impl Importer {
    pub fn new(config: ImportConfig, log: Box<dyn FnMut(&str) + Send>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("hoglet-import/", env!("CARGO_PKG_VERSION")))
            .build();
        Self { config, agent, log }
    }

    pub fn run(&mut self) -> Result<ImportReport, String> {
        let mut checkpoint = self.load_checkpoint();
        let mut report = ImportReport {
            events: checkpoint.events,
            ..ImportReport::default()
        };
        if !self.config.skip_events && !checkpoint.events_done {
            self.import_events(&mut checkpoint, &mut report)?;
        }
        if !self.config.skip_persons && !checkpoint.persons_done {
            report.persons = self.import_persons()?;
            checkpoint.persons_done = true;
            self.save_checkpoint(&checkpoint)?;
        }
        if self.config.hoglet_key.is_some() && self.config.hoglet_project_id.is_some() {
            report.flags = self.import_flags()?;
        }
        Ok(report)
    }

    fn import_events(
        &mut self,
        checkpoint: &mut Checkpoint,
        report: &mut ImportReport,
    ) -> Result<(), String> {
        let since = self
            .config
            .since
            .as_deref()
            .map(|since| format!("timestamp >= toDateTime({})", hogql_string(since)))
            .unwrap_or_else(|| "1 = 1".to_owned());
        loop {
            let keyset = match &checkpoint.after {
                Some((timestamp, uuid)) => format!(
                    "AND (timestamp > toDateTime64({ts}, 6, 'UTC') \
                     OR (timestamp = toDateTime64({ts}, 6, 'UTC') AND toString(uuid) > {uuid}))",
                    ts = hogql_string(timestamp),
                    uuid = hogql_string(uuid)
                ),
                None => String::new(),
            };
            let query = format!(
                "SELECT toString(uuid), event, distinct_id, \
                        formatDateTime(timestamp, '%Y-%m-%d %H:%i:%S.%f', 'UTC'), properties \
                 FROM events WHERE {since} {keyset} \
                 ORDER BY timestamp, toString(uuid) LIMIT {PAGE_ROWS}"
            );
            let rows = self.hogql(&query)?;
            if rows.is_empty() {
                checkpoint.events_done = true;
                self.save_checkpoint(checkpoint)?;
                return Ok(());
            }
            let mut batch = Vec::with_capacity(rows.len());
            let mut last = None;
            for row in &rows {
                let row = row.as_array().ok_or("PostHog returned a malformed row")?;
                let text = |index: usize| row.get(index).and_then(Value::as_str).unwrap_or("");
                let (uuid, event, distinct_id, timestamp) = (text(0), text(1), text(2), text(3));
                if uuid.is_empty() || event.is_empty() || distinct_id.is_empty() {
                    continue;
                }
                let properties = match row.get(4) {
                    Some(Value::String(encoded)) => serde_json::from_str(encoded).unwrap_or_else(|_| json!({})),
                    Some(Value::Object(map)) => Value::Object(map.clone()),
                    _ => json!({}),
                };
                batch.push(json!({
                    "uuid": uuid,
                    "event": event,
                    "distinct_id": distinct_id,
                    "timestamp": format!("{}Z", timestamp.replace(' ', "T")),
                    "properties": properties,
                }));
                last = Some((timestamp.to_owned(), uuid.to_owned()));
            }
            for chunk in batch.chunks(BATCH_EVENTS) {
                self.capture(chunk)?;
            }
            report.events += batch.len() as u64;
            checkpoint.events = report.events;
            checkpoint.after = last;
            self.save_checkpoint(checkpoint)?;
            (self.log)(&format!("events: {} imported", report.events));
            if rows.len() < PAGE_ROWS {
                checkpoint.events_done = true;
                self.save_checkpoint(checkpoint)?;
                return Ok(());
            }
        }
    }

    /// Merge each person's distinct ids and set their current properties.
    fn import_persons(&mut self) -> Result<u64, String> {
        let mut persons = 0_u64;
        let mut after = String::new();
        loop {
            let query = format!(
                "SELECT toString(p.id), p.properties, groupArray(pdi.distinct_id) \
                 FROM persons p JOIN person_distinct_ids pdi ON pdi.person_id = p.id \
                 WHERE toString(p.id) > {after} \
                 GROUP BY p.id, p.properties ORDER BY toString(p.id) LIMIT {PAGE_ROWS}",
                after = hogql_string(&after)
            );
            let rows = self.hogql(&query)?;
            if rows.is_empty() {
                return Ok(persons);
            }
            let mut events = Vec::new();
            for row in &rows {
                let Some(row) = row.as_array() else { continue };
                let id = row.first().and_then(Value::as_str).unwrap_or("").to_owned();
                let properties = match row.get(1) {
                    Some(Value::String(encoded)) => {
                        serde_json::from_str::<Map<String, Value>>(encoded).unwrap_or_default()
                    }
                    Some(Value::Object(map)) => map.clone(),
                    _ => Map::new(),
                };
                let mut ids: Vec<String> = row
                    .get(2)
                    .and_then(Value::as_array)
                    .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_default();
                ids.sort();
                ids.dedup();
                after = id;
                let Some(primary) = ids.first().cloned() else {
                    continue;
                };
                for other in &ids[1..] {
                    events.push(json!({
                        "event": "$merge_dangerously",
                        "distinct_id": primary,
                        "properties": {"alias": other, "$lib": "hoglet-import"},
                    }));
                }
                if !properties.is_empty() {
                    events.push(json!({
                        "event": "$set",
                        "distinct_id": primary,
                        "properties": {"$set": properties, "$lib": "hoglet-import"},
                    }));
                }
                persons += 1;
            }
            for chunk in events.chunks(BATCH_EVENTS) {
                self.capture(chunk)?;
            }
            (self.log)(&format!("persons: {persons} imported"));
            if rows.len() < PAGE_ROWS {
                return Ok(persons);
            }
        }
    }

    fn import_flags(&mut self) -> Result<u64, String> {
        let mut url = format!(
            "{}/api/projects/{}/feature_flags/?limit=100",
            self.config.posthog_host.trim_end_matches('/'),
            self.config.posthog_project_id
        );
        let mut imported = 0_u64;
        loop {
            let page = self.get_posthog(&url)?;
            for flag in page["results"].as_array().into_iter().flatten() {
                if flag["deleted"].as_bool() == Some(true) {
                    continue;
                }
                let input = json!({
                    "key": flag["key"],
                    "name": flag["name"].as_str().unwrap_or(""),
                    "active": flag["active"].as_bool().unwrap_or(true),
                    "filters": {
                        "groups": flag["filters"]["groups"],
                        "multivariate": flag["filters"]["multivariate"],
                        "payloads": flag["filters"]["payloads"].as_object().cloned().unwrap_or_default(),
                    },
                    "ensure_experience_continuity": flag["ensure_experience_continuity"].as_bool().unwrap_or(false),
                });
                match self.post_hoglet_flag(&input) {
                    Ok(()) => imported += 1,
                    Err(error) => (self.log)(&format!(
                        "flag {}: skipped ({error})",
                        flag["key"].as_str().unwrap_or("?")
                    )),
                }
            }
            match page["next"].as_str() {
                Some(next) if !next.is_empty() => url = next.to_owned(),
                _ => break,
            }
        }
        (self.log)(&format!("flags: {imported} imported"));
        Ok(imported)
    }

    fn hogql(&mut self, query: &str) -> Result<Vec<Value>, String> {
        let url = format!(
            "{}/api/projects/{}/query/",
            self.config.posthog_host.trim_end_matches('/'),
            self.config.posthog_project_id
        );
        let body = json!({"query": {"kind": "HogQLQuery", "query": query}});
        let response = self.with_retries(|agent, key| {
            agent
                .post(&url)
                .set("Authorization", &format!("Bearer {key}"))
                .send_json(body.clone())
        })?;
        Ok(response["results"].as_array().cloned().unwrap_or_default())
    }

    fn get_posthog(&mut self, url: &str) -> Result<Value, String> {
        let url = url.to_owned();
        self.with_retries(|agent, key| {
            agent
                .get(&url)
                .set("Authorization", &format!("Bearer {key}"))
                .call()
        })
    }

    fn capture(&mut self, events: &[Value]) -> Result<(), String> {
        let url = format!("{}/batch/", self.config.hoglet_host.trim_end_matches('/'));
        let body = json!({
            "api_key": self.config.hoglet_token,
            "historical_migration": true,
            "batch": events,
        });
        self.with_retries(|agent, _| agent.post(&url).send_json(body.clone()))
            .map(|_| ())
    }

    fn post_hoglet_flag(&mut self, input: &Value) -> Result<(), String> {
        let (Some(key), Some(project)) = (
            self.config.hoglet_key.clone(),
            self.config.hoglet_project_id.clone(),
        ) else {
            return Ok(());
        };
        let url = format!(
            "{}/api/projects/{project}/feature_flags",
            self.config.hoglet_host.trim_end_matches('/')
        );
        match self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {key}"))
            .send_json(input.clone())
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(409, _)) => Err("a flag with this key exists".to_owned()),
            Err(ureq::Error::Status(code, response)) => Err(format!(
                "HTTP {code}: {}",
                response.into_string().unwrap_or_default()
            )),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Retry 429/5xx/transport errors with exponential backoff; 4xx is final.
    fn with_retries(
        &mut self,
        call: impl Fn(&ureq::Agent, &str) -> Result<ureq::Response, ureq::Error>,
    ) -> Result<Value, String> {
        let mut delay = Duration::from_millis(500);
        for attempt in 1..=MAX_ATTEMPTS {
            match call(&self.agent, &self.config.posthog_key) {
                Ok(response) => {
                    let text = response.into_string().map_err(|error| error.to_string())?;
                    return Ok(if text.trim().is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_str(&text).unwrap_or(Value::Null)
                    });
                }
                Err(ureq::Error::Status(code, response)) if code == 429 || code >= 500 => {
                    let wait = response
                        .header("Retry-After")
                        .and_then(|value| value.parse::<u64>().ok())
                        .map(Duration::from_secs)
                        .unwrap_or(delay);
                    (self.log)(&format!("HTTP {code}; retrying in {}s (attempt {attempt})", wait.as_secs()));
                    std::thread::sleep(wait);
                }
                Err(ureq::Error::Status(code, response)) => {
                    return Err(format!(
                        "HTTP {code}: {}",
                        response.into_string().unwrap_or_default().chars().take(400).collect::<String>()
                    ));
                }
                Err(error) if attempt < MAX_ATTEMPTS => {
                    (self.log)(&format!("{error}; retrying"));
                    std::thread::sleep(delay);
                }
                Err(error) => return Err(error.to_string()),
            }
            delay = (delay * 2).min(Duration::from_secs(60));
        }
        Err("gave up after repeated failures".to_owned())
    }

    fn load_checkpoint(&self) -> Checkpoint {
        std::fs::read(&self.config.checkpoint)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        let temporary = self.config.checkpoint.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(checkpoint).unwrap_or_default())
            .and_then(|()| std::fs::rename(&temporary, &self.config.checkpoint))
            .map_err(|error| format!("cannot write checkpoint: {error}"))
    }
}

/// A HogQL string literal.
fn hogql_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Parse `hoglet import posthog --flag value …`.
pub fn parse_args(args: &[String]) -> Result<ImportConfig, String> {
    let mut values = std::collections::HashMap::new();
    let mut flags = std::collections::HashSet::new();
    let mut index = 0;
    while index < args.len() {
        let name = args[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument {:?}", args[index]))?;
        if matches!(name, "skip-events" | "skip-persons") {
            flags.insert(name.to_owned());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("--{name} needs a value"))?;
        values.insert(name.to_owned(), value.clone());
        index += 2;
    }
    let required = |name: &str| {
        values
            .get(name)
            .cloned()
            .ok_or_else(|| format!("missing --{name}"))
    };
    let posthog_project_id = required("posthog-project")?;
    Ok(ImportConfig {
        posthog_host: values
            .get("posthog-host")
            .cloned()
            .unwrap_or_else(|| "https://us.posthog.com".to_owned()),
        posthog_key: required("posthog-key")?,
        hoglet_host: values
            .get("hoglet-host")
            .cloned()
            .unwrap_or_else(|| "http://localhost:8000".to_owned()),
        hoglet_token: required("token")?,
        hoglet_key: values.get("hoglet-key").cloned(),
        hoglet_project_id: values.get("hoglet-project").cloned(),
        since: values.get("since").cloned(),
        checkpoint: PathBuf::from(
            values
                .get("checkpoint")
                .cloned()
                .unwrap_or_else(|| format!(".hoglet-import-{posthog_project_id}.json")),
        ),
        posthog_project_id,
        skip_events: flags.contains("skip-events"),
        skip_persons: flags.contains("skip-persons"),
    })
}

pub const HELP: &str = "\
hoglet import posthog — copy a PostHog project into Hoglet

USAGE:
    hoglet import posthog --posthog-project <id> --posthog-key <phx_…> --token <phc_…> [options]

REQUIRED:
    --posthog-project <id>   PostHog project id (Settings → Project → ID)
    --posthog-key <phx_…>    PostHog personal API key with query read access
    --token <phc_…>          Hoglet project token to import into

OPTIONS:
    --posthog-host <url>     default https://us.posthog.com (EU: https://eu.posthog.com)
    --hoglet-host <url>      default http://localhost:8000
    --hoglet-key <phx_…>     Hoglet personal API key with write scope; with
                             --hoglet-project, imports feature flags
    --hoglet-project <id>    Hoglet project id (for flags)
    --since <date>           only events on or after this ISO date
    --checkpoint <file>      progress file (default .hoglet-import-<id>.json); rerun resumes
    --skip-events            only import persons/flags
    --skip-persons           only import events/flags

Re-running is safe: events keep their PostHog uuids and Hoglet stores each once.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hogql_strings_are_escaped() {
        assert_eq!(hogql_string("a'b\\c"), "'a\\'b\\\\c'");
    }

    #[test]
    fn arguments_parse_with_defaults() {
        let args: Vec<String> = [
            "--posthog-project", "42", "--posthog-key", "phx_x", "--token", "phc_y", "--skip-persons",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let config = parse_args(&args).unwrap();
        assert_eq!(config.posthog_host, "https://us.posthog.com");
        assert_eq!(config.hoglet_host, "http://localhost:8000");
        assert!(config.skip_persons && !config.skip_events);
        assert_eq!(config.checkpoint, PathBuf::from(".hoglet-import-42.json"));
        assert!(parse_args(&args[..2]).is_err());
    }
}
