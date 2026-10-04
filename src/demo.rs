//! Realistic demo data: 90 days of a small SaaS product ("Notably").
//!
//! The generator is deterministic for a seed and produces what the PostHog
//! SDKs would have sent: anonymous visitors with sessions, UTM and referrer
//! attribution, browsers and countries, a signup funnel with `$identify`
//! merges, activation and retention behaviour, revenue, AI generations and
//! the occasional exception. Events go through the real durable pipeline, so
//! the demo exercises exactly the code paths real traffic does.

use chrono::{DateTime, Duration, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::capture::event::CapturedEvent;

/// Upper bound on generated events, whatever the scale.
pub const MAX_DEMO_EVENTS: usize = 2_000_000;

pub struct DemoConfig {
    pub token: String,
    pub now: DateTime<Utc>,
    pub days: u32,
    /// Roughly the number of distinct visitors.
    pub visitors: u32,
    pub seed: u64,
}

impl DemoConfig {
    pub fn new(token: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            token: token.into(),
            now,
            days: 90,
            visitors: 4_000,
            seed: 7,
        }
    }
}

const HOST: &str = "notably.app";

const MARKETING_PAGES: &[(&str, u32)] = &[
    ("/", 40),
    ("/pricing", 14),
    ("/features", 12),
    ("/blog/second-brain", 8),
    ("/blog/markdown-tips", 6),
    ("/blog/notes-vs-docs", 5),
    ("/changelog", 4),
    ("/about", 3),
    ("/integrations", 4),
    ("/security", 2),
    ("/docs", 2),
];

const REFERRERS: &[(Option<(&str, &str)>, u32)] = &[
    (None, 34),
    (Some(("https://www.google.com/", "www.google.com")), 30),
    (Some(("https://news.ycombinator.com/", "news.ycombinator.com")), 9),
    (Some(("https://www.reddit.com/r/productivity/", "www.reddit.com")), 7),
    (Some(("https://twitter.com/", "twitter.com")), 6),
    (Some(("https://duckduckgo.com/", "duckduckgo.com")), 5),
    (Some(("https://www.producthunt.com/", "www.producthunt.com")), 4),
    (Some(("https://github.com/", "github.com")), 3),
    (Some(("https://www.linkedin.com/", "www.linkedin.com")), 2),
];

/// `(utm_source, utm_medium, utm_campaign)`.
type Campaign = (&'static str, &'static str, &'static str);

const CAMPAIGNS: &[(Option<Campaign>, u32)] = &[
    (None, 80),
    (Some(("newsletter", "email", "october_digest")), 7),
    (Some(("google", "cpc", "brand_search")), 6),
    (Some(("twitter", "social", "launch_week")), 4),
    (Some(("partner", "referral", "obsidian_friends")), 3),
];

const BROWSERS: &[(&str, &str, &str, u32)] = &[
    ("Chrome", "Mac OS X", "Desktop", 30),
    ("Chrome", "Windows", "Desktop", 22),
    ("Safari", "iOS", "Mobile", 16),
    ("Safari", "Mac OS X", "Desktop", 10),
    ("Firefox", "Linux", "Desktop", 5),
    ("Firefox", "Windows", "Desktop", 4),
    ("Chrome", "Android", "Mobile", 9),
    ("Edge", "Windows", "Desktop", 3),
    ("Safari", "iOS", "Tablet", 1),
];

const COUNTRIES: &[(&str, u32)] = &[
    ("US", 34),
    ("GB", 9),
    ("DE", 9),
    ("IN", 8),
    ("FR", 5),
    ("CA", 5),
    ("NL", 4),
    ("BR", 4),
    ("JP", 3),
    ("AU", 3),
    ("ES", 3),
    ("SE", 2),
    ("PL", 2),
    ("SG", 2),
    ("NG", 1),
];

const FIRST_NAMES: &[&str] = &[
    "Ada", "Grace", "Linus", "Margaret", "Alan", "Barbara", "Ken", "Radia", "Tim", "Frances",
    "Dennis", "Hedy", "Guido", "Katherine", "Bjarne", "Sophie", "Yukihiro", "Anita", "Brendan",
    "Jean", "Donald", "Lynn", "Edsger", "Shafi", "Leslie", "Karen", "Niklaus", "Mary", "John",
    "Priya",
];

const COMPANIES: &[&str] = &[
    "acme.io", "globex.com", "initech.dev", "umbrella.co", "hooli.xyz", "stark.ai",
    "wayne.org", "tyrell.tech", "gmail.com", "fastmail.com", "proton.me", "outlook.com",
];

const AI_MODELS: &[(&str, &str, f64, f64)] = &[
    ("gpt-4o-mini", "openai", 0.000_15, 0.000_6),
    ("claude-haiku-4-5", "anthropic", 0.001, 0.005),
    ("claude-sonnet-4-5", "anthropic", 0.003, 0.015),
];

fn pick<'a, T>(rng: &mut StdRng, items: &'a [(T, u32)]) -> &'a T {
    let total: u32 = items.iter().map(|(_, weight)| weight).sum();
    let mut roll = rng.gen_range(0..total);
    for (item, weight) in items {
        if roll < *weight {
            return item;
        }
        roll -= weight;
    }
    &items[0].0
}

fn pick_browser(rng: &mut StdRng) -> (&'static str, &'static str, &'static str) {
    let total: u32 = BROWSERS.iter().map(|b| b.3).sum();
    let mut roll = rng.gen_range(0..total);
    for &(browser, os, device, weight) in BROWSERS {
        if roll < weight {
            return (browser, os, device);
        }
        roll -= weight;
    }
    (BROWSERS[0].0, BROWSERS[0].1, BROWSERS[0].2)
}

struct Visitor {
    anon_id: String,
    user_id: Option<String>,
    browser: (&'static str, &'static str, &'static str),
    country: &'static str,
}

impl Visitor {
    fn distinct_id(&self) -> &str {
        self.user_id.as_deref().unwrap_or(&self.anon_id)
    }
}

struct Generator<'a> {
    config: &'a DemoConfig,
    rng: StdRng,
    events: Vec<CapturedEvent>,
}

impl Generator<'_> {
    fn base_properties(
        &self,
        visitor: &Visitor,
        session_id: &str,
        path: &str,
    ) -> Map<String, Value> {
        let mut properties = Map::new();
        properties.insert("$lib".into(), json!("web"));
        properties.insert("$lib_version".into(), json!("1.270.0"));
        properties.insert("$session_id".into(), json!(session_id));
        properties.insert("$host".into(), json!(HOST));
        properties.insert("$pathname".into(), json!(path));
        properties.insert("$current_url".into(), json!(format!("https://{HOST}{path}")));
        properties.insert("$browser".into(), json!(visitor.browser.0));
        properties.insert("$os".into(), json!(visitor.browser.1));
        properties.insert("$device_type".into(), json!(visitor.browser.2));
        properties.insert("$geoip_country_code".into(), json!(visitor.country));
        properties
    }

    fn push(&mut self, name: &str, distinct_id: &str, at: DateTime<Utc>, properties: Map<String, Value>) {
        if self.events.len() >= MAX_DEMO_EVENTS || at > self.config.now {
            return;
        }
        self.events.push(CapturedEvent {
            uuid: Uuid::now_v7(),
            event: name.to_owned(),
            distinct_id: distinct_id.to_owned(),
            token: self.config.token.clone(),
            timestamp: at,
            properties,
        });
    }

    /// One marketing or product session; returns when it ended.
    fn session(
        &mut self,
        visitor: &Visitor,
        start: DateTime<Utc>,
        pages: &[&str],
        attribution: Option<&Map<String, Value>>,
    ) -> DateTime<Utc> {
        let session_id = Uuid::now_v7().to_string();
        let mut at = start;
        for (index, path) in pages.iter().enumerate() {
            let mut properties = self.base_properties(visitor, &session_id, path);
            if index == 0
                && let Some(attribution) = attribution
            {
                for (key, value) in attribution {
                    properties.insert(key.clone(), value.clone());
                }
            }
            let distinct_id = visitor.distinct_id().to_owned();
            self.push("$pageview", &distinct_id, at, properties);
            // About half of single-page visits are quick glances (a bounce).
            let glance = pages.len() == 1 && self.rng.gen_bool(0.5);
            at += Duration::seconds(if glance {
                self.rng.gen_range(1..9)
            } else {
                self.rng.gen_range(12..240)
            });
        }
        let mut properties = self.base_properties(visitor, &session_id, pages[pages.len() - 1]);
        properties.insert("$prev_pageview_pathname".into(), json!(pages[pages.len() - 1]));
        let distinct_id = visitor.distinct_id().to_owned();
        self.push("$pageleave", &distinct_id, at, properties);
        at
    }

    fn attribution(&mut self) -> Map<String, Value> {
        let mut properties = Map::new();
        if let Some((url, domain)) = pick(&mut self.rng, REFERRERS) {
            properties.insert("$referrer".into(), json!(url));
            properties.insert("$referring_domain".into(), json!(domain));
        } else {
            properties.insert("$referrer".into(), json!("$direct"));
            properties.insert("$referring_domain".into(), json!("$direct"));
        }
        if let Some((source, medium, campaign)) = pick(&mut self.rng, CAMPAIGNS) {
            properties.insert("utm_source".into(), json!(source));
            properties.insert("utm_medium".into(), json!(medium));
            properties.insert("utm_campaign".into(), json!(campaign));
        }
        properties
    }

    fn marketing_path(&mut self) -> Vec<&'static str> {
        let length = match self.rng.gen_range(0..100) {
            0..=44 => 1,
            45..=69 => 2,
            70..=86 => 3,
            _ => 4,
        };
        let mut pages: Vec<&'static str> = Vec::with_capacity(length);
        pages.push(pick(&mut self.rng, MARKETING_PAGES));
        while pages.len() < length {
            let next = match pages[pages.len() - 1] {
                "/" => *pick(&mut self.rng, &[("/features", 5), ("/pricing", 6), ("/blog/second-brain", 2)]),
                "/features" => *pick(&mut self.rng, &[("/pricing", 7), ("/integrations", 3)]),
                "/pricing" => *pick(&mut self.rng, &[("/signup", 6), ("/features", 2), ("/security", 1)]),
                path if path.starts_with("/blog") => *pick(&mut self.rng, &[("/", 4), ("/pricing", 3), ("/blog/markdown-tips", 2)]),
                _ => *pick(&mut self.rng, &[("/pricing", 3), ("/", 2)]),
            };
            pages.push(next);
        }
        pages
    }

    /// Signed-up users come back: app sessions with product events.
    fn product_life(&mut self, visitor: &Visitor, joined: DateTime<Utc>, plan: &str) {
        let engaged: f64 = self.rng.gen_range(0.15..0.95);
        let mut day = joined + Duration::days(1);
        let mut notes = 0_u32;
        let mut upgraded = plan != "free";
        while day < self.config.now {
            // Engagement decays; engaged users keep coming back.
            let age_days = (day - joined).num_days() as f64;
            let p = engaged * (0.85_f64).powf(age_days / 7.0).max(0.08);
            let weekday = day.format("%u").to_string().parse::<u32>().unwrap_or(1);
            let weekend_factor = if weekday >= 6 { 0.45 } else { 1.0 };
            if self.rng.gen_bool((p * weekend_factor).clamp(0.0, 1.0)) {
                let start = day + Duration::minutes(self.rng.gen_range(7 * 60..22 * 60));
                let at = self.session(visitor, start, &["/app", "/app/notes"], None);
                let distinct_id = visitor.distinct_id().to_owned();
                let actions = self.rng.gen_range(1..6);
                let mut t = at;
                for _ in 0..actions {
                    t += Duration::seconds(self.rng.gen_range(20..600));
                    notes += 1;
                    let mut properties = Map::new();
                    properties.insert("$session_id".into(), json!(Uuid::now_v7().to_string()));
                    properties.insert("words".into(), json!(self.rng.gen_range(12..1800)));
                    properties.insert("template".into(), json!(*pick(&mut self.rng, &[("blank", 6), ("meeting", 3), ("journal", 2), ("todo", 2)])));
                    self.push("note_created", &distinct_id, t, properties);
                    if self.rng.gen_bool(0.12) {
                        self.ai_generation(&distinct_id, t + Duration::seconds(5));
                    }
                }
                if notes > 5 && self.rng.gen_bool(0.04) {
                    let mut properties = Map::new();
                    properties.insert("invitee_count".into(), json!(self.rng.gen_range(1..5)));
                    self.push("teammate_invited", &distinct_id, t + Duration::seconds(30), properties);
                }
                if !upgraded && notes > 8 && self.rng.gen_bool(0.05) {
                    upgraded = true;
                    let (new_plan, amount) = *pick(&mut self.rng, &[(("pro", 12.0), 4), (("team", 49.0), 1)]);
                    let mut properties = Map::new();
                    properties.insert("plan".into(), json!(new_plan));
                    properties.insert("amount".into(), json!(amount));
                    properties.insert("currency".into(), json!("USD"));
                    properties.insert("$set".into(), json!({"plan": new_plan}));
                    self.push("subscription_started", &distinct_id, t + Duration::seconds(60), properties);
                }
                if self.rng.gen_bool(0.01) {
                    let mut properties = Map::new();
                    properties.insert("$exception_type".into(), json!("TypeError"));
                    properties.insert("$exception_message".into(), json!("Cannot read properties of undefined (reading 'blocks')"));
                    properties.insert("$exception_source".into(), json!("https://notably.app/assets/editor.js"));
                    self.push("$exception", &distinct_id, t + Duration::seconds(2), properties);
                }
            }
            day += Duration::days(1);
        }
    }

    fn ai_generation(&mut self, distinct_id: &str, at: DateTime<Utc>) {
        let (model, provider, input_price, output_price) =
            AI_MODELS[self.rng.gen_range(0..AI_MODELS.len())];
        let input_tokens = self.rng.gen_range(200..4000);
        let output_tokens = self.rng.gen_range(50..900);
        let cost = input_tokens as f64 / 1000.0 * input_price
            + output_tokens as f64 / 1000.0 * output_price;
        let mut properties = Map::new();
        properties.insert("$ai_model".into(), json!(model));
        properties.insert("$ai_provider".into(), json!(provider));
        properties.insert("$ai_input_tokens".into(), json!(input_tokens));
        properties.insert("$ai_output_tokens".into(), json!(output_tokens));
        properties.insert("$ai_latency".into(), json!(self.rng.gen_range(0.4..6.5)));
        properties.insert("$ai_total_cost_usd".into(), json!((cost * 1e6).round() / 1e6));
        properties.insert("$ai_trace_id".into(), json!(Uuid::now_v7().to_string()));
        properties.insert("feature".into(), json!("summarize_note"));
        self.push("$ai_generation", distinct_id, at, properties);
    }

    fn visitor_life(&mut self, index: u32) {
        let days = i64::from(self.config.days.max(1));
        // Growth: later days see more first visits (quadratic ramp).
        let position: f64 = self.rng.gen_range(0.0_f64..1.0).sqrt();
        let first_day = self.config.now - Duration::days(days) + Duration::days((position * days as f64) as i64);
        let start = first_day + Duration::minutes(self.rng.gen_range(6 * 60..23 * 60));
        let (browser, os, device) = pick_browser(&mut self.rng);
        let mut visitor = Visitor {
            anon_id: format!("anon-{:08x}-{index}", self.rng.r#gen::<u32>()),
            user_id: None,
            browser: (browser, os, device),
            country: pick(&mut self.rng, COUNTRIES),
        };

        let attribution = self.attribution();
        let pages = self.marketing_path();
        let mut at = self.session(&visitor, start, &pages, Some(&attribution));

        // Some visitors return before deciding.
        let mut converted = pages.contains(&"/signup") && self.rng.gen_bool(0.55);
        let mut returns = 0;
        while !converted && returns < 3 && self.rng.gen_bool(0.3) {
            returns += 1;
            let later = at + Duration::hours(self.rng.gen_range(3..120));
            let pages = self.marketing_path();
            let attribution = self.attribution();
            at = self.session(&visitor, later, &pages, Some(&attribution));
            converted = pages.contains(&"/signup") && self.rng.gen_bool(0.5);
        }
        if !converted {
            return;
        }

        // Signup: identify merges the anonymous history into the user.
        let first = FIRST_NAMES[self.rng.gen_range(0..FIRST_NAMES.len())];
        let company = COMPANIES[self.rng.gen_range(0..COMPANIES.len())];
        let email = format!("{}.{index}@{company}", first.to_lowercase());
        let user_id = format!("user_{index}");
        let plan = *pick(&mut self.rng, &[("free", 85), ("pro", 12), ("team", 3)]);
        at += Duration::seconds(self.rng.gen_range(30..240));
        let mut identify = Map::new();
        identify.insert("$anon_distinct_id".into(), json!(visitor.anon_id));
        identify.insert(
            "$set".into(),
            json!({"email": email, "name": format!("{first} {company}"), "plan": plan, "company": company}),
        );
        identify.insert("$set_once".into(), json!({"initial_referring_domain": attribution.get("$referring_domain").cloned().unwrap_or(json!("$direct"))}));
        self.push("$identify", &user_id, at, identify);
        visitor.user_id = Some(user_id.clone());
        let mut signup = Map::new();
        signup.insert("plan".into(), json!(plan));
        signup.insert("method".into(), json!(*pick(&mut self.rng, &[("email", 5), ("google", 4), ("github", 2)])));
        self.push("signed_up", &user_id, at + Duration::seconds(1), signup);

        // Activation: the first note, usually the same day.
        if self.rng.gen_bool(0.68) {
            let first_note = at + Duration::minutes(self.rng.gen_range(1..180));
            let mut properties = Map::new();
            properties.insert("words".into(), json!(self.rng.gen_range(5..400)));
            properties.insert("template".into(), json!("onboarding"));
            self.push("note_created", &user_id, first_note, properties);
            self.product_life(&visitor, first_note, plan);
        }
    }
}

/// Generate the demo dataset, oldest first.
pub fn generate(config: &DemoConfig) -> Vec<CapturedEvent> {
    let mut generator = Generator {
        config,
        rng: StdRng::seed_from_u64(config.seed),
        events: Vec::new(),
    };
    for index in 0..config.visitors {
        generator.visitor_life(index);
        if generator.events.len() >= MAX_DEMO_EVENTS {
            break;
        }
    }
    let mut events = generator.events;
    events.sort_by_key(|event| event.timestamp);
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_deterministic_and_realistic() {
        let now = Utc::now();
        let config = DemoConfig {
            visitors: 600,
            ..DemoConfig::new("phc_demo", now)
        };
        let first = generate(&config);
        let second = generate(&config);
        assert_eq!(first.len(), second.len());
        assert!(first.iter().all(|event| event.timestamp <= now));
        let count = |name: &str| first.iter().filter(|event| event.event == name).count();
        assert!(count("$pageview") > 600);
        assert!(count("$identify") > 10, "some visitors sign up");
        assert!(count("signed_up") == count("$identify"));
        assert!(count("note_created") > count("signed_up"), "activated users create notes");
        assert!(count("$ai_generation") > 0);
        assert!(first.windows(2).all(|pair| pair[0].timestamp <= pair[1].timestamp));
    }
}
