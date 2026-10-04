//! `GET /api/projects/{project_id}/status` — data freshness, always visible
//! in the dashboard. If numbers are behind, the product says so.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::contract::persons::ProjectStatus;
use crate::control::{AccessError, ProjectAccess};
use crate::lake::Lake;
use crate::sink::PipelineStats;

#[derive(Clone)]
struct StatusState {
    access: Arc<ProjectAccess>,
    lake: Arc<Lake>,
    stats: Arc<PipelineStats>,
}

pub fn router(access: Arc<ProjectAccess>, lake: Arc<Lake>, stats: Arc<PipelineStats>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/status", get(status))
        .with_state(StatusState {
            access,
            lake,
            stats,
        })
}

async fn status(
    State(state): State<StatusState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let principal = match crate::routes::workspace::authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error),
    };
    let project = match state.access.authorize_project(&principal, &project_id).await {
        Ok(project) => project,
        Err(error) => return access_error(error),
    };
    let stored = state.lake.status(&project.project_id);
    let last_event_at = state
        .lake
        .lease(
            &project.project_id,
            stored.last_day.unwrap_or(chrono::NaiveDate::MIN),
            chrono::NaiveDate::MAX,
        )
        .files()
        .iter()
        .map(|file| file.created_at)
        .max()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|time| time.to_rfc3339());
    Json(ProjectStatus {
        has_events: stored.rows > 0,
        last_event_at,
        ingestion_lag_seconds: state.stats.lag_seconds(),
        stored_events: stored.rows,
        stored_bytes: stored.bytes,
        first_day: stored.first_day.map(|day| day.to_string()),
        last_day: stored.last_day.map(|day| day.to_string()),
    })
    .into_response()
}

fn access_error(error: AccessError) -> Response {
    let (status, code) = match error {
        AccessError::Unauthorized | AccessError::InvalidToken => {
            (StatusCode::UNAUTHORIZED, "unauthorized")
        }
        AccessError::Forbidden | AccessError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    (
        status,
        Json(json!({"error": {"code": code, "message": code.replace('_', " ")}})),
    )
        .into_response()
}
