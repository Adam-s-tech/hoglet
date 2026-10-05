//! Capture endpoints — one handler aliased across every path PostHog SDKs
//! post events to (spec/wire-compat.md "Endpoints").
//!
//! Response codes are load-bearing: 200 normally, 204 when `beacon=1`, 4xx
//! for anything the client must not retry, 503 only for retryable sink
//! failure. posthog-js retries 5xx and network errors, never 4xx.

pub mod decompress;
pub mod event;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;
use serde_json::json;

use crate::metrics::Metrics;
use crate::ratelimit::RateLimiter;
use crate::sink::{AuthorizedEventBatch, EventSink};

/// A project approved for capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedCapture {
    pub project_id: String,
}

/// Capture authorization is deliberately fail-closed at the HTTP edge.
///
/// Both variants become a 401 so SDKs retain the established no-retry
/// behavior. `Unavailable` exists so adapters can preserve useful diagnostics
/// without leaking control-plane failures into the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAuthorizationError {
    Rejected,
    Unavailable,
}

/// The only project-authentication dependency visible to capture handlers.
#[allow(clippy::double_must_use)] // async_trait's boxed future is already must_use
#[async_trait::async_trait]
pub trait CaptureAuthorizer: Send + Sync {
    async fn authorize(&self, token: &str) -> Result<AuthorizedCapture, CaptureAuthorizationError>;
}

/// Bound on cached token decisions.
const TOKEN_CACHE_ENTRIES: usize = 10_000;
/// How long a known token stays trusted without asking control state.
const TOKEN_CACHE_ACCEPT_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// How long an unknown token is remembered as unknown.
const TOKEN_CACHE_REJECT_TTL: std::time::Duration = std::time::Duration::from_secs(10);

/// Fail-closed adapter for authoritative `control.db` project access.
///
/// Every capture request names a token, so decisions are cached briefly:
/// otherwise each request would queue behind the single control-plane worker.
pub struct ProjectAccessCaptureAuthorizer {
    access: crate::control::ProjectAccess,
    cache: std::sync::Mutex<
        std::collections::HashMap<String, (Option<String>, std::time::Instant)>,
    >,
}

impl ProjectAccessCaptureAuthorizer {
    pub fn new(access: crate::control::ProjectAccess) -> Self {
        Self {
            access,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn cached(&self, token: &str) -> Option<Option<String>> {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (decision, at) = cache.get(token)?;
        let ttl = if decision.is_some() {
            TOKEN_CACHE_ACCEPT_TTL
        } else {
            TOKEN_CACHE_REJECT_TTL
        };
        (at.elapsed() < ttl).then(|| decision.clone())
    }

    fn remember(&self, token: &str, decision: Option<String>) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= TOKEN_CACHE_ENTRIES {
            cache.retain(|_, (decision, at)| {
                at.elapsed()
                    < if decision.is_some() {
                        TOKEN_CACHE_ACCEPT_TTL
                    } else {
                        TOKEN_CACHE_REJECT_TTL
                    }
            });
            if cache.len() >= TOKEN_CACHE_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(token.to_owned(), (decision, std::time::Instant::now()));
    }
}

#[async_trait::async_trait]
impl CaptureAuthorizer for ProjectAccessCaptureAuthorizer {
    async fn authorize(&self, token: &str) -> Result<AuthorizedCapture, CaptureAuthorizationError> {
        if let Some(decision) = self.cached(token) {
            return decision
                .map(|project_id| AuthorizedCapture { project_id })
                .ok_or(CaptureAuthorizationError::Rejected);
        }
        match self.access.authorize_capture(token).await {
            Ok(project) => {
                self.remember(token, Some(project.project_id.clone()));
                Ok(AuthorizedCapture {
                    project_id: project.project_id,
                })
            }
            Err(crate::control::AccessError::InvalidToken)
            | Err(crate::control::AccessError::Unauthorized) => {
                self.remember(token, None);
                Err(CaptureAuthorizationError::Rejected)
            }
            Err(error) => {
                tracing::error!(?error, "capture authorization unavailable");
                Err(CaptureAuthorizationError::Unavailable)
            }
        }
    }
}

/// Fixed token → project map. For tests and embedded tooling only; production
/// composition uses [`ProjectAccessCaptureAuthorizer`].
#[derive(Debug, Default, Clone)]
pub struct StaticCaptureAuthorizer {
    projects: std::collections::HashMap<String, String>,
}

impl StaticCaptureAuthorizer {
    pub fn new<I, T, P>(projects: I) -> Self
    where
        I: IntoIterator<Item = (T, P)>,
        T: Into<String>,
        P: Into<String>,
    {
        Self {
            projects: projects
                .into_iter()
                .map(|(token, project)| (token.into(), project.into()))
                .collect(),
        }
    }
}

#[async_trait::async_trait]
impl CaptureAuthorizer for StaticCaptureAuthorizer {
    async fn authorize(&self, token: &str) -> Result<AuthorizedCapture, CaptureAuthorizationError> {
        self.projects
            .get(token)
            .map(|project_id| AuthorizedCapture {
                project_id: project_id.clone(),
            })
            .ok_or(CaptureAuthorizationError::Rejected)
    }
}

#[derive(Clone)]
pub struct CaptureState {
    pub sink: Arc<dyn EventSink>,
    pub authorizer: Arc<dyn CaptureAuthorizer>,
    pub limiter: Arc<RateLimiter>,
    pub metrics: Arc<Metrics>,
    pub enricher: Arc<crate::enrichment::Enricher>,
    /// Shadow mode: forward acknowledged events to PostHog.
    pub forwarder: Option<Arc<crate::forward::Forwarder>>,
}

/// Body limit for browser-SDK endpoints (/e and friends).
pub const MAX_EVENT_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Body limit for server-SDK batches (/batch).
pub const MAX_BATCH_BODY_BYTES: usize = 20 * 1024 * 1024;
/// Ceiling on a decompressed browser-SDK payload (a body is never refused for
/// being larger than its own raw size, so uncompressed bodies are unaffected).
pub const MAX_EVENT_DECODED_BYTES: usize = 16 * 1024 * 1024;
/// Ceiling on a decompressed server-SDK batch.
pub const MAX_BATCH_DECODED_BYTES: usize = 32 * 1024 * 1024;
/// Decodes that can expand run on the blocking pool, this many at a time.
const MAX_CONCURRENT_DECODES: usize = 2;
/// Parsed JSON costs up to ~16x its text in memory (tiny elements are 32-byte
/// values). Requests reserve `16 x decoded size` from this pool before
/// parsing, so a flood of anonymous payloads cannot exhaust the machine.
const PARSE_COST_FACTOR: usize = 16;
const PARSE_BUDGET_BYTES: usize = 512 * 1024 * 1024;
/// A request waits this long for memory budget before being told to retry.
const PARSE_BUDGET_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

static DECODE_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_CONCURRENT_DECODES);
/// One permit per KiB of parse budget.
static PARSE_BUDGET: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(PARSE_BUDGET_BYTES / 1024);

#[derive(serde::Deserialize, Default)]
pub struct CaptureQuery {
    /// sent_at in ms since epoch; doubles as cache buster.
    #[serde(rename = "_")]
    pub sent_at: Option<String>,
    pub compression: Option<String>,
    pub beacon: Option<String>,
}

pub fn router(state: CaptureState) -> Router {
    let small = Router::new()
        .route("/e", post(capture))
        .route("/e/", post(capture))
        .route("/capture", post(capture))
        .route("/capture/", post(capture))
        .route("/track", post(capture))
        .route("/track/", post(capture))
        .route("/engage", post(capture))
        .route("/engage/", post(capture))
        .route("/i/v0/e", post(capture))
        .route("/i/v0/e/", post(capture))
        .layer(DefaultBodyLimit::max(MAX_EVENT_BODY_BYTES));
    let batch = Router::new()
        .route("/batch", post(capture))
        .route("/batch/", post(capture))
        .layer(DefaultBodyLimit::max(MAX_BATCH_BODY_BYTES));
    small.merge(batch).with_state(state)
}

async fn capture(
    State(state): State<CaptureState>,
    uri: axum::http::Uri,
    Query(query): Query<CaptureQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let beacon = query.beacon.as_deref() == Some("1");

    let form_encoded = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));

    let decoded_limit = if uri.path().starts_with("/batch") {
        MAX_BATCH_DECODED_BYTES
    } else {
        MAX_EVENT_DECODED_BYTES
    }
    .max(body.len());
    let hint = query.compression.clone();
    let decoded = if decompress::may_expand(&body, form_encoded, hint.as_deref()) {
        // Anything that can expand is decoded off the async workers and at
        // most a couple at a time.
        let Ok(gate) = DECODE_GATE.acquire().await else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        let result = tokio::task::spawn_blocking(move || {
            decompress::decode_limited(&body, form_encoded, hint.as_deref(), decoded_limit)
        })
        .await;
        drop(gate);
        match result {
            Ok(result) => result,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    } else {
        decompress::decode_limited(&body, form_encoded, hint.as_deref(), decoded_limit)
    };
    let text = match decoded {
        Ok(text) => text,
        Err(decompress::DecodeError::TooLarge) => {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        Err(decompress::DecodeError::Undecodable) => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // Reserve memory for the parsed form before building it. Held until the
    // events are handed to the sink (or the request is refused).
    let cost_kib = (text.len().saturating_mul(PARSE_COST_FACTOR) / 1024 + 1)
        .min(PARSE_BUDGET_BYTES / 1024);
    let _budget = match tokio::time::timeout(
        PARSE_BUDGET_WAIT,
        PARSE_BUDGET.acquire_many(u32::try_from(cost_kib).unwrap_or(u32::MAX)),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    let now = Utc::now();
    let sent_at = query.sent_at.as_deref().and_then(event::parse_sent_at_ms);

    let parsed = if text.len() > 64 * 1024 {
        match tokio::task::spawn_blocking(move || event::parse_body(&text, sent_at, now)).await {
            Ok(parsed) => parsed,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    } else {
        event::parse_body(&text, sent_at, now)
    };
    let mut batch = match parsed {
        Ok(batch) => batch,
        Err(event::CaptureError::Malformed(_)) => {
            state.metrics.inc_rejected();
            return StatusCode::BAD_REQUEST.into_response();
        }
        Err(event::CaptureError::Unauthorized(_)) => {
            state.metrics.inc_rejected();
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    // Server SDKs send no `$browser`/`$os`; fill what the client did not set.
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok());
    let client_ip = client_ip(&headers);
    for event in &mut batch.events {
        state.enricher.enrich(event, client_ip.as_deref(), user_agent);
    }

    // Empty after filtering is still success — never make clients retry.
    if !batch.events.is_empty() {
        // Shape was checked during parsing. Authenticate every distinct token
        // before applying rate limits or making any durable write; a batch may
        // never inherit the first event's project authorization.
        let mut token_counts = BTreeMap::<&str, u32>::new();
        for event in &batch.events {
            let count = token_counts.entry(event.token.as_str()).or_default();
            *count = count.saturating_add(1);
        }
        let mut project_ids_by_token = BTreeMap::new();
        for token in token_counts.keys() {
            match state.authorizer.authorize(token).await {
                Ok(authorized) => {
                    project_ids_by_token.insert((*token).to_owned(), authorized.project_id);
                }
                Err(_) => {
                    state.metrics.inc_rejected();
                    return StatusCode::UNAUTHORIZED.into_response();
                }
            }
        }
        // Rate limit each authorized project independently; 429 is retry-safe
        // on the SDK's backoff.
        for (token, count) in token_counts {
            if !state.limiter.allow(token, count, now.timestamp()) {
                state.metrics.inc_rejected();
                return StatusCode::TOO_MANY_REQUESTS.into_response();
            }
        }
        // `historical_migration` is an offline-import authority, never a wire
        // property. A client cannot elevate an ordinary capture batch by
        // placing this implementation detail in its JSON body.
        let historical_migration = false;
        let events = batch.events;
        let n = events.len() as u64;
        // Imports (and PostHog's own migration tooling) mark batches as
        // historical; replaying history into PostHog again would duplicate it.
        let forwarded = state
            .forwarder
            .as_ref()
            .filter(|_| !batch.historical_migration)
            .map(|_| events.clone());
        let bindings = project_ids_by_token.clone();
        state.metrics.inc_captured(n);
        match state
            .sink
            .append(AuthorizedEventBatch {
                events,
                project_ids_by_token,
                historical_migration,
            })
            .await
        {
            Ok(()) => {
                state.metrics.inc_acked(n);
                if let (Some(forwarder), Some(events)) = (&state.forwarder, forwarded) {
                    let mut by_project: BTreeMap<&str, Vec<event::CapturedEvent>> = BTreeMap::new();
                    for event in events {
                        if let Some(project_id) = bindings.get(&event.token) {
                            by_project.entry(project_id).or_default().push(event);
                        }
                    }
                    for (project_id, events) in by_project {
                        forwarder.offer(project_id, &events);
                    }
                }
            }
            Err(crate::sink::SinkError::Retryable) => {
                state.metrics.inc_sink_errors();
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            Err(crate::sink::SinkError::Fatal) => {
                state.metrics.inc_rejected();
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
    }

    if beacon {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Json(json!({"status": 1})).into_response()
    }
}

/// The client address as reported by the reverse proxy in front of Hoglet.
/// Without a proxy header there is no trustworthy address to use.
fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value.len() <= 64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::MemorySink;
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    use tower::ServiceExt;

    #[derive(Default)]
    struct BatchSink {
        batches: std::sync::Mutex<Vec<AuthorizedEventBatch>>,
    }

    #[async_trait::async_trait]
    impl EventSink for BatchSink {
        async fn append(&self, batch: AuthorizedEventBatch) -> Result<(), crate::sink::SinkError> {
            self.batches.lock().unwrap().push(batch);
            Ok(())
        }
    }

    /// Accepts every well-formed token, or only `known` when it is non-empty.
    struct TestAuthorizer {
        known: Vec<&'static str>,
    }

    #[async_trait::async_trait]
    impl CaptureAuthorizer for TestAuthorizer {
        async fn authorize(
            &self,
            token: &str,
        ) -> Result<AuthorizedCapture, CaptureAuthorizationError> {
            if self.known.is_empty() || self.known.contains(&token) {
                Ok(AuthorizedCapture {
                    project_id: format!("project-{token}"),
                })
            } else {
                Err(CaptureAuthorizationError::Rejected)
            }
        }
    }

    fn open_authorizer() -> Arc<TestAuthorizer> {
        Arc::new(TestAuthorizer { known: Vec::new() })
    }

    fn app_with_sink() -> (Router, Arc<MemorySink>) {
        let sink = Arc::new(MemorySink::default());
        let state = CaptureState {
            sink: sink.clone(),
            authorizer: open_authorizer(),
            limiter: Arc::new(RateLimiter::new(crate::ratelimit::DEFAULT_MAX_PER_SEC)),
            metrics: Arc::new(Metrics::default()),
            enricher: Arc::new(crate::enrichment::Enricher::new(Default::default())),
            forwarder: None,
        };
        (router(state), sink)
    }

    async fn post_body(router: Router, uri: &str, body: impl Into<Body>) -> StatusCode {
        router
            .oneshot(Request::post(uri).body(body.into()).unwrap())
            .await
            .unwrap()
            .status()
    }

    const EVENT: &str = r#"[{"event":"click","distinct_id":"u1","token":"phc_t"}]"#;

    #[tokio::test]
    async fn plain_json_to_e_stores_event() {
        let (router, sink) = app_with_sink();
        let status = post_body(router, "/e/", EVENT).await;
        assert_eq!(status, StatusCode::OK);
        let events = sink.snapshot();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "click");
        assert_eq!(events[0].distinct_id, "u1");
    }

    #[tokio::test]
    async fn gzip_without_hint_is_sniffed() {
        let (router, sink) = app_with_sink();
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(EVENT.as_bytes()).unwrap();
        let status = post_body(router, "/e/", enc.finish().unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn base64_body_unwraps() {
        let (router, sink) = app_with_sink();
        let status = post_body(router, "/e/", BASE64.encode(EVENT)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn beacon_returns_204() {
        let (router, sink) = app_with_sink();
        let status = post_body(router, "/e/?beacon=1", EVENT).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(sink.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn batch_endpoint_accepts_batch_shape() {
        let (router, sink) = app_with_sink();
        let body = r#"{"api_key":"phc_t","batch":[{"event":"a","distinct_id":"u1"},{"event":"b","distinct_id":"u2"}]}"#;
        let status = post_body(router, "/batch/", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.snapshot().len(), 2);
    }

    #[tokio::test]
    async fn one_unknown_token_rejects_entire_batch_before_append() {
        let sink = Arc::new(MemorySink::default());
        let state = CaptureState {
            sink: sink.clone(),
            authorizer: Arc::new(TestAuthorizer {
                known: vec!["phc_known"],
            }),
            limiter: Arc::new(RateLimiter::new(crate::ratelimit::DEFAULT_MAX_PER_SEC)),
            metrics: Arc::new(Metrics::default()),
            enricher: Arc::new(crate::enrichment::Enricher::new(Default::default())),
            forwarder: None,
        };
        let body = r#"[{"event":"known","distinct_id":"u1","token":"phc_known"},{"event":"unknown","distinct_id":"u2","token":"phc_unknown"}]"#;

        let status = post_body(router(state), "/batch/", body).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(sink.snapshot().is_empty());
    }

    #[tokio::test]
    async fn capture_clients_cannot_mark_batches_as_historical_migrations() {
        let sink = Arc::new(BatchSink::default());
        let state = CaptureState {
            sink: sink.clone(),
            authorizer: open_authorizer(),
            limiter: Arc::new(RateLimiter::new(crate::ratelimit::DEFAULT_MAX_PER_SEC)),
            metrics: Arc::new(Metrics::default()),
            enricher: Arc::new(crate::enrichment::Enricher::new(Default::default())),
            forwarder: None,
        };
        let body = r#"{"api_key":"phc_t","historical_migration":true,"batch":[{"event":"a","distinct_id":"u1"}]}"#;

        let status = post_body(router(state), "/batch/", body).await;

        assert_eq!(status, StatusCode::OK);
        let batches = sink.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert!(!batches[0].historical_migration);
    }

    #[tokio::test]
    async fn all_aliases_accept() {
        for path in ["/e", "/capture", "/track", "/engage", "/i/v0/e", "/batch"] {
            let (router, _) = app_with_sink();
            let body = if path == "/engage" {
                r#"{"distinct_id":"u1","token":"phc_t","$set":{"a":1}}"#
            } else {
                EVENT
            };
            let status = post_body(router, path, body).await;
            assert_eq!(status, StatusCode::OK, "alias {path} failed");
        }
    }

    #[tokio::test]
    async fn malformed_json_is_400_never_retried() {
        let (router, _) = app_with_sink();
        assert_eq!(
            post_body(router, "/e/", "not json").await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn missing_token_is_401() {
        let (router, _) = app_with_sink();
        let body = r#"[{"event":"a","distinct_id":"u1"}]"#;
        assert_eq!(
            post_body(router, "/e/", body).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn form_encoded_beacon_path_works() {
        let (router, sink) = app_with_sink();
        let data = BASE64
            .encode(EVENT)
            .replace('+', "%2B")
            .replace('/', "%2F")
            .replace('=', "%3D");
        let status = router
            .oneshot(
                Request::post("/e/?beacon=1")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("data={data}")))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(sink.snapshot().len(), 1);
    }
}
