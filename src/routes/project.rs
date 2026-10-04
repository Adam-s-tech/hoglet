//! Authenticated, project-scoped insight queries.
//!
//! `POST /api/projects/{project_id}/query` (`QueryRequest` → `QueryResponse`)
//! and `POST /api/projects/{project_id}/query/actors` (`ActorsRequest` →
//! `ActorsResponse`). The path project is authorized (session cookie or
//! personal API key) before the body is read, so an unauthorized caller
//! cannot use validation errors as an oracle. Errors are
//! `contract::common::ApiError` with a matching status: 400 invalid, 503
//! busy (with `Retry-After`), 504 timeout.

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::{
    contract::common::{ApiError, ApiErrorBody},
    contract::insight::{ActorsRequest, QueryRequest},
    control::{AccessError, AuthorizedProject, ProjectAccess},
    query::{QueryEngine, QueryError},
};

const MAX_QUERY_BODY_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct ProjectState {
    access: Arc<ProjectAccess>,
    engine: Arc<QueryEngine>,
}

#[derive(Clone)]
struct RequestId(String);

/// Builds the authenticated dashboard surface for project analytics.
pub fn router(access: Arc<ProjectAccess>, engine: Arc<QueryEngine>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/query", post(query))
        .route("/api/projects/{project_id}/query/actors", post(actors))
        .with_state(ProjectState { access, engine })
        .layer(middleware::from_fn(assign_request_id))
}

async fn assign_request_id(mut request: Request, next: Next) -> Response {
    let request_id = RequestId(Uuid::now_v7().to_string());
    request.extensions_mut().insert(request_id.clone());
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

/// Authorize the path project, then read and parse the body.
async fn authorized_body<T: DeserializeOwned>(
    state: &ProjectState,
    project_id: &str,
    request: Request,
    request_id: &RequestId,
) -> Result<(AuthorizedProject, T), Response> {
    let principal = crate::routes::workspace::authenticate(&state.access, request.headers())
        .await
        .map_err(|error| access_error(error, request_id))?;
    if Uuid::parse_str(project_id).is_err() {
        return Err(error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.",
            request_id,
        ));
    }
    let project = state
        .access
        .authorize_project(&principal, project_id)
        .await
        .map_err(|error| access_error(error, request_id))?;
    let body = to_bytes(request.into_body(), MAX_QUERY_BODY_BYTES)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The request body is invalid or too large.",
                request_id,
            )
        })?;
    let body = serde_json::from_slice(&body).map_err(|error| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            &format!("The request body is invalid: {error}"),
            request_id,
        )
    })?;
    Ok((project, body))
}

/// Admission, then the blocking engine call on the blocking pool.
async fn execute<T: Send + 'static>(
    engine: Arc<QueryEngine>,
    work: impl FnOnce(&QueryEngine) -> Result<T, QueryError> + Send + 'static,
) -> Result<T, QueryError> {
    let admission = engine.admit().await?;
    tokio::task::spawn_blocking(move || {
        let _admission = admission;
        work(&engine)
    })
    .await
    .map_err(|_| QueryError::internal("query task failed"))?
}

async fn query(
    State(state): State<ProjectState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    request: Request,
) -> Response {
    let (project, body) =
        match authorized_body::<QueryRequest>(&state, &project_id, request, &request_id).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let project_id = project.project_id;
    match execute(state.engine.clone(), move |engine| {
        engine.run(&project_id, &body)
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => query_error(error, &request_id),
    }
}

async fn actors(
    State(state): State<ProjectState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    request: Request,
) -> Response {
    let (project, body) =
        match authorized_body::<ActorsRequest>(&state, &project_id, request, &request_id).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let project_id = project.project_id;
    match execute(state.engine.clone(), move |engine| {
        engine.actors(&project_id, &body)
    })
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => query_error(error, &request_id),
    }
}

fn query_error(error: QueryError, request_id: &RequestId) -> Response {
    if let QueryError::Internal(message) = &error {
        tracing::error!(request_id = %request_id.0, "query failed: {message}");
    }
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = error_response(status, error.code(), &error.public_message(), request_id);
    if matches!(error, QueryError::Busy) {
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            HeaderValue::from_static("1"),
        );
    }
    response
}

fn access_error(error: AccessError, request_id: &RequestId) -> Response {
    let (status, code, message) = match error {
        AccessError::InvalidCredentials | AccessError::InvalidToken | AccessError::Unauthorized => {
            (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Authentication is required.",
            )
        }
        AccessError::Forbidden => (
            StatusCode::FORBIDDEN,
            "forbidden",
            "You do not have access to this project.",
        ),
        AccessError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.",
        ),
        AccessError::InvalidRequest => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request is invalid.",
        ),
        AccessError::SetupComplete => (
            StatusCode::CONFLICT,
            "conflict",
            "The request conflicts with the current state.",
        ),
        AccessError::Unavailable
        | AccessError::InvalidStorage
        | AccessError::Incompatible { .. } => (
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "The service is temporarily unavailable.",
        ),
        AccessError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "An internal error occurred.",
        ),
    };
    error_response(status, code, message, request_id)
}

fn error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    request_id: &RequestId,
) -> Response {
    (
        status,
        Json(ApiError {
            error: ApiErrorBody {
                code: code.to_owned(),
                message: message.to_owned(),
                request_id: Some(request_id.0.clone()),
            },
        }),
    )
        .into_response()
}
