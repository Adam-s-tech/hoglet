//! `hoglet reconcile posthog` — do Hoglet and PostHog agree?
//!
//! Runs the same aggregation on both systems — events and unique persons per
//! event per day — and prints where they differ. Use it during shadow mode:
//! when the numbers match, cutting over is safe; when they don't, the table
//! says exactly which event and day to look at.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct ReconcileConfig {
    pub posthog_host: String,
    pub posthog_project_id: String,
    pub posthog_key: String,
    pub hoglet_host: String,
    pub hoglet_project_id: String,
    pub hoglet_key: String,
    /// Days back from today, inclusive of today.
    pub days: u32,
    /// Differences at or below this percentage are reported as matching.
    pub tolerance_percent: f64,
}

/// Counts for one `(day, event)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counts {
    pub events: u64,
    pub persons: u64,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub day: String,
    pub event: String,
    pub hoglet: Counts,
    pub posthog: Counts,
}

impl Row {
    fn diff(&self, pick: impl Fn(&Counts) -> u64) -> f64 {
        let (a, b) = (pick(&self.hoglet) as f64, pick(&self.posthog) as f64);
        if a == b {
            0.0
        } else {
            (a - b).abs() / a.max(b) * 100.0
        }
    }

    pub fn events_diff_percent(&self) -> f64 {
        self.diff(|counts| counts.events)
    }

    pub fn persons_diff_percent(&self) -> f64 {
        self.diff(|counts| counts.persons)
    }
}

pub fn run(config: &ReconcileConfig) -> Result<Vec<Row>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build();
    let days = config.days.clamp(1, 90);

    let posthog = {
        let url = format!(
            "{}/api/projects/{}/query/",
            config.posthog_host.trim_end_matches('/'),
            config.posthog_project_id
        );
        let query = format!(
            "SELECT toString(toDate(timestamp)), event, count(), count(DISTINCT person_id) \
             FROM events WHERE timestamp >= toStartOfDay(now() - INTERVAL {} DAY) \
             GROUP BY 1, 2 ORDER BY 1, 2 LIMIT 50000",
            days - 1
        );
        let body = post(
            &agent,
            &url,
            &config.posthog_key,
            json!({"query": {"kind": "HogQLQuery", "query": query}}),
        )?;
        rows(&body["results"])
    };
    let hoglet = {
        let url = format!(
            "{}/api/projects/{}/query",
            config.hoglet_host.trim_end_matches('/'),
            config.hoglet_project_id
        );
        // Computed here: DuckDB's date arithmetic on TIMESTAMPTZ needs ICU,
        // which the static binary does not ship.
        let cutoff = (chrono::Utc::now().date_naive()
            - chrono::Duration::days(i64::from(days - 1)))
        .format("%Y-%m-%d 00:00:00");
        let query = format!(
            "SELECT strftime(timestamp, '%Y-%m-%d') AS day, event, count(*) AS events, \
             count(DISTINCT person_id) AS persons FROM events \
             WHERE timestamp >= TIMESTAMP '{cutoff}' \
             GROUP BY 1, 2 ORDER BY 1, 2"
        );
        let body = post(
            &agent,
            &url,
            &config.hoglet_key,
            json!({"query": {"kind": "SqlQuery", "query": query}, "refresh": true}),
        )?;
        rows(&body["result"]["rows"])
    };

    let mut merged: BTreeMap<(String, String), Row> = BTreeMap::new();
    for (key, counts) in hoglet {
        merged
            .entry(key.clone())
            .or_insert_with(|| Row {
                day: key.0.clone(),
                event: key.1.clone(),
                hoglet: Counts::default(),
                posthog: Counts::default(),
            })
            .hoglet = counts;
    }
    for (key, counts) in posthog {
        merged
            .entry(key.clone())
            .or_insert_with(|| Row {
                day: key.0.clone(),
                event: key.1.clone(),
                hoglet: Counts::default(),
                posthog: Counts::default(),
            })
            .posthog = counts;
    }
    Ok(merged.into_values().collect())
}

fn rows(value: &Value) -> Vec<((String, String), Counts)> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let row = row.as_array()?;
            let text = |index: usize| row.get(index).and_then(Value::as_str).map(str::to_owned);
            let number = |index: usize| {
                row.get(index).and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                })
            };
            Some((
                (text(0)?, text(1)?),
                Counts {
                    events: number(2)?,
                    persons: number(3)?,
                },
            ))
        })
        .collect()
}

fn post(agent: &ureq::Agent, url: &str, key: &str, body: Value) -> Result<Value, String> {
    let response = agent
        .post(url)
        .set("Authorization", &format!("Bearer {key}"))
        .send_json(body)
        .map_err(|error| match error {
            ureq::Error::Status(code, response) => format!(
                "{url}: HTTP {code}: {}",
                response
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>()
            ),
            other => format!("{url}: {other}"),
        })?;
    response.into_json().map_err(|error| error.to_string())
}

/// Render the comparison for a terminal. Returns `(text, all_match)`.
pub fn render(rows: &[Row], tolerance_percent: f64) -> (String, bool) {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<10}  {:<28} {:>10} {:>10} {:>7}  {:>9} {:>9} {:>7}\n",
        "day", "event", "hoglet", "posthog", "Δ%", "persons H", "persons P", "Δ%"
    ));
    let mut all_match = true;
    for row in rows {
        let events = row.events_diff_percent();
        let persons = row.persons_diff_percent();
        let ok = events <= tolerance_percent && persons <= tolerance_percent;
        all_match &= ok;
        out.push_str(&format!(
            "{:<10}  {:<28} {:>10} {:>10} {:>6.1}{} {:>9} {:>9} {:>6.1}\n",
            row.day,
            row.event.chars().take(28).collect::<String>(),
            row.hoglet.events,
            row.posthog.events,
            events,
            if ok { " " } else { "!" },
            row.hoglet.persons,
            row.posthog.persons,
            persons,
        ));
    }
    out.push_str(if all_match {
        "\nAll counts match within tolerance.\n"
    } else {
        "\nRows marked ! differ beyond tolerance.\n"
    });
    (out, all_match)
}

pub const HELP: &str = "\
hoglet reconcile posthog — compare Hoglet's numbers with PostHog's

USAGE:
    hoglet reconcile posthog --posthog-project <id> --posthog-key <phx_…>
                             --hoglet-project <id> --hoglet-key <phx_…> [options]

OPTIONS:
    --posthog-host <url>   default https://us.posthog.com
    --hoglet-host <url>    default http://localhost:8000
    --days <n>             days to compare, up to 90 (default 7)
    --tolerance <percent>  allowed difference (default 0.5)

Compares events and unique persons per event per day. Exit status 1 when
any row differs beyond the tolerance.
";

pub fn parse_args(args: &[String]) -> Result<ReconcileConfig, String> {
    let mut values = std::collections::HashMap::new();
    let mut index = 0;
    while index < args.len() {
        let name = args[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument {:?}", args[index]))?;
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
    Ok(ReconcileConfig {
        posthog_host: values
            .get("posthog-host")
            .cloned()
            .unwrap_or_else(|| "https://us.posthog.com".to_owned()),
        posthog_project_id: required("posthog-project")?,
        posthog_key: required("posthog-key")?,
        hoglet_host: values
            .get("hoglet-host")
            .cloned()
            .unwrap_or_else(|| "http://localhost:8000".to_owned()),
        hoglet_project_id: required("hoglet-project")?,
        hoglet_key: required("hoglet-key")?,
        days: values
            .get("days")
            .map(|days| days.parse().map_err(|_| "--days must be a number".to_owned()))
            .transpose()?
            .unwrap_or(7),
        tolerance_percent: values
            .get("tolerance")
            .map(|value| value.parse().map_err(|_| "--tolerance must be a number".to_owned()))
            .transpose()?
            .unwrap_or(0.5),
    })
}
