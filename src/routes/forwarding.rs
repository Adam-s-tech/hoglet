//! `GET/PUT /api/projects/{project_id}/forwarding` — shadow-mode settings.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::control::{AuthorizedProject, ProjectAccess};
use crate::routes::guard::Intent;
use crate::forward::{ForwardingConfig, Forwarder};

#[derive(Clone)]
struct ForwardingState {
    access: Arc<ProjectAccess>,
    forwarder: Arc<Forwarder>,
}

pub fn router(access: Arc<ProjectAccess>, forwarder: Arc<Forwarder>) -> Router {
    Router::new()
        .route(
            "/api/projects/{project_id}/forwarding",
            get(read).put(update),
        )
        .with_state(ForwardingState { access, forwarder })
}

async fn authorize(
    state: &ForwardingState,
    project_id: &str,
    headers: &HeaderMap,
    intent: Intent,
) -> Result<AuthorizedProject, Response> {
    crate::routes::guard::authorize(&state.access, headers, project_id, intent).await
}

async fn read(
    State(state): State<ForwardingState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match authorize(&state, &project_id, &headers, Intent::Read).await {
        Ok(project) => Json(state.forwarder.status(&project.project_id)).into_response(),
        Err(response) => response,
    }
}

async fn update(
    State(state): State<ForwardingState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<ForwardingConfig>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let project = match authorize(&state, &project_id, &headers, Intent::Write).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let request_id = crate::routes::guard::request_id(&headers);
    let error = |status, code: &str, message: &str| {
        crate::routes::guard::error(status, code, message, &request_id)
    };
    let Ok(Json(config)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_request", "Expected {enabled, host, posthog_token}.");
    };
    let host_ok = (config.host.starts_with("https://") || config.host.starts_with("http://"))
        && config.host.len() <= 256;
    let token_ok = config.posthog_token.starts_with("phc_") && config.posthog_token.len() <= 128;
    if config.enabled && (!host_ok || !token_ok) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "host must be an http(s) URL and posthog_token a phc_ project key.",
        );
    }
    match state.forwarder.set_config(&project.project_id, config) {
        Ok(()) => Json(state.forwarder.status(&project.project_id)).into_response(),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Forwarding settings could not be saved.",
        ),
    }
}
