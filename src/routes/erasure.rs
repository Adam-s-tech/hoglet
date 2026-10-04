//! `POST /api/projects/{project_id}/persons/{person_id}/erase` — GDPR
//! erasure. Physical: the person, their distinct ids, and every stored event
//! of those ids are removed from the lake, not hidden.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;

use crate::control::{AccessError, ProjectAccess, Role};
use crate::sink::Eraser;

#[derive(Clone)]
struct ErasureState {
    access: Arc<ProjectAccess>,
    eraser: Eraser,
}

pub fn router(access: Arc<ProjectAccess>, eraser: Eraser) -> Router {
    Router::new()
        .route(
            "/api/projects/{project_id}/persons/{person_id}/erase",
            post(erase),
        )
        .with_state(ErasureState { access, eraser })
}

async fn erase(
    State(state): State<ErasureState>,
    Path((project_id, person_id)): Path<(String, String)>,
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
    if !matches!(project.role, Role::Owner | Role::Admin) {
        return error(StatusCode::FORBIDDEN, "forbidden", "Only owners and admins can erase people.");
    }
    if person_id.is_empty() || person_id.len() > 400 {
        return error(StatusCode::NOT_FOUND, "not_found", "No such person.");
    }
    match state
        .eraser
        .erase_person(&project.project_id, &person_id)
        .await
    {
        Ok(report) if report.distinct_ids == 0 => {
            error(StatusCode::NOT_FOUND, "not_found", "No such person.")
        }
        Ok(report) => Json(report).into_response(),
        Err(failure) => {
            tracing::error!(%failure, "person erasure failed");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Erasure could not complete; nothing was reported erased. Try again.",
            )
        }
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
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
