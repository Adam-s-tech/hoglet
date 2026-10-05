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

use crate::control::ProjectAccess;
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
    let project = match crate::routes::guard::authorize(
        &state.access,
        &headers,
        &project_id,
        crate::routes::guard::Intent::Write,
    )
    .await
    {
        Ok(project) => project,
        Err(response) => return response,
    };
    let request_id = crate::routes::guard::request_id(&headers);
    let error = |status, code: &str, message: &str| {
        crate::routes::guard::error(status, code, message, &request_id)
    };
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
