//! `/metrics` — Prometheus text exposition of Hoglet's own counters
//! (spec/README.md "self-observability").

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::metrics::Metrics;
use crate::security::constant_time_eq;

#[derive(Clone)]
struct MetricsState {
    metrics: Arc<Metrics>,
    /// `HOGLET_METRICS_TOKEN`; `None` leaves `/metrics` open (the default,
    /// because the counters hold no project data).
    token: Option<Arc<str>>,
}

pub fn router(metrics: Arc<Metrics>) -> Router {
    router_with_token(metrics, None)
}

pub fn router_with_token(metrics: Arc<Metrics>, token: Option<String>) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state(MetricsState {
            metrics,
            token: token.map(Arc::from),
        })
}

async fn scrape(State(state): State<MetricsState>, headers: HeaderMap) -> Response {
    if let Some(expected) = state.token.as_deref() {
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();
        if !constant_time_eq(expected, presented) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    (
        [("content-type", "text/plain; version=0.0.4")],
        state.metrics.render(now),
    )
        .into_response()
}
