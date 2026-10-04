//! Shared plumbing of the explore routes: request ids, project
//! authorization (session cookie or personal API key), query-string parsing,
//! and the uniform `ApiError` body.
//!
//! Helpers return `Result<_, Response>` so handlers can bail out with the
//! finished error response; the large `Err` variant is deliberate.
#![allow(clippy::result_large_err)]

use axum::{
    Json,
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::contract::common::{ApiError, ApiErrorBody};
use crate::control::{AccessError, ProjectAccess};

use super::ExploreError;

#[derive(Clone)]
pub struct RequestId(pub String);

/// Middleware: stamp every request and response with an `x-request-id`.
pub async fn assign_request_id(mut request: Request, next: Next) -> Response {
    let request_id = RequestId(Uuid::now_v7().to_string());
    request.extensions_mut().insert(request_id.clone());
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

pub fn api_error(
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

pub fn invalid(message: &str, request_id: &RequestId) -> Response {
    api_error(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        message,
        request_id,
    )
}

pub fn not_found(request_id: &RequestId) -> Response {
    api_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "The requested resource was not found.",
        request_id,
    )
}

/// Authenticate, then authorize the path project. Returns the project id.
pub async fn authorize(
    access: &ProjectAccess,
    headers: &HeaderMap,
    project_id: &str,
    request_id: &RequestId,
) -> Result<String, Response> {
    let principal = crate::routes::workspace::authenticate(access, headers)
        .await
        .map_err(|error| access_error(error, request_id))?;
    if Uuid::parse_str(project_id).is_err() {
        return Err(not_found(request_id));
    }
    access
        .authorize_project(&principal, project_id)
        .await
        .map(|project| project.project_id)
        .map_err(|error| access_error(error, request_id))
}

/// Parse the query string, after authorization.
pub fn query<T: DeserializeOwned>(uri: &Uri, request_id: &RequestId) -> Result<T, Response> {
    axum::extract::Query::<T>::try_from_uri(uri)
        .map(|axum::extract::Query(query)| query)
        .map_err(|_| invalid("The query string is invalid.", request_id))
}

pub fn explore_error(error: ExploreError, request_id: &RequestId) -> Response {
    match error {
        ExploreError::Invalid { field, message } => {
            invalid(&format!("{field}: {message}"), request_id)
        }
        ExploreError::NotFound => not_found(request_id),
        ExploreError::Busy => {
            let mut response = error_response_busy(request_id);
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("1"),
            );
            response
        }
        ExploreError::Timeout => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "query_timeout",
            "The query exceeded its execution deadline.",
            request_id,
        ),
        other => {
            tracing::error!(request_id = %request_id.0, error = %other, "explore request failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "The request could not be completed.",
                request_id,
            )
        }
    }
}

fn error_response_busy(request_id: &RequestId) -> Response {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "query_busy",
        "Too many queries are running; retry shortly.",
        request_id,
    )
}

pub fn access_error(error: AccessError, request_id: &RequestId) -> Response {
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
    api_error(status, code, message, request_id)
}

/// State of every explore router.
#[derive(Clone)]
pub struct ExploreState {
    pub access: std::sync::Arc<ProjectAccess>,
    pub explorer: std::sync::Arc<super::Explorer>,
}

/// Longest path identifier (person id) accepted; longer ones cannot exist.
pub const MAX_PATH_ID: usize = 1_024;

/// Parse an optional RFC 3339 `before` cursor.
pub fn parse_before(
    before: Option<&str>,
    request_id: &RequestId,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, Response> {
    let bad = || invalid("before: not an RFC 3339 timestamp.", request_id);
    match before.map(str::trim).filter(|text| !text.is_empty()) {
        None => Ok(None),
        Some(text) if text.len() > 64 => Err(bad()),
        Some(text) => chrono::DateTime::parse_from_rfc3339(text)
            .map(|at| Some(at.with_timezone(&chrono::Utc)))
            .map_err(|_| bad()),
    }
}

/// Run explore work under admission control and serialize its outcome.
pub async fn respond<T, F>(state: &ExploreState, request_id: &RequestId, work: F) -> Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(&super::Explorer, &duckdb::Connection) -> Result<T, ExploreError> + Send + 'static,
{
    match state.explorer.run(work).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => explore_error(error, request_id),
    }
}
