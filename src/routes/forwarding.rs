//! `GET/PUT /api/projects/{project_id}/forwarding` — shadow-mode settings.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::control::{AccessError, AuthorizedProject, ProjectAccess, Role};
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
) -> Result<AuthorizedProject, Response> {
    let principal = crate::routes::workspace::authenticate(&state.access, headers)
        .await
        .map_err(access_error)?;
    state
        .access
        .authorize_project(&principal, project_id)
        .await
        .map_err(access_error)
}

async fn read(
    State(state): State<ForwardingState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match authorize(&state, &project_id, &headers).await {
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
    let project = match authorize(&state, &project_id, &headers).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    if !project.principal.may_write() || !matches!(project.role, Role::Owner | Role::Admin) {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only owners and admins can change forwarding.",
        );
    }
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

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"error": {"code": code, "message": message}}))).into_response()
}

fn access_error(failure: AccessError) -> Response {
    match failure {
        AccessError::Unauthorized | AccessError::InvalidToken => {
            error(StatusCode::UNAUTHORIZED, "unauthorized", "Log in to continue.")
        }
        AccessError::Forbidden | AccessError::NotFound => {
            error(StatusCode::NOT_FOUND, "not_found", "No such project.")
        }
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Project access is unavailable.",
        ),
    }
}
