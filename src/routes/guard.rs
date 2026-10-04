//! Shared project authorization for the smaller dashboard routes.
//!
//! One mapping of access failures to HTTP, one error body shape
//! (`ApiError` with a request id), one rule for writes: a session or a
//! write-scoped key, held by a project owner or admin.

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::contract::common::{ApiError, ApiErrorBody};
use crate::control::{AccessError, AuthorizedProject, ProjectAccess, Role};

/// What a request intends to do with the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Read,
    /// Change project data or settings: owner/admin, session or write key.
    Write,
}

pub fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            // Echoed into bodies and logs: plain identifier characters only.
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string())
}

pub fn error(status: StatusCode, code: &str, message: &str, request_id: &str) -> Response {
    let mut response = (
        status,
        Json(ApiError {
            error: ApiErrorBody {
                code: code.to_owned(),
                message: message.to_owned(),
                request_id: Some(request_id.to_owned()),
            },
        }),
    )
        .into_response();
    if let Ok(value) = request_id.parse() {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

pub fn access_error(failure: AccessError, request_id: &str) -> Response {
    match failure {
        AccessError::InvalidCredentials | AccessError::Unauthorized => error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication is required.",
            request_id,
        ),
        AccessError::Forbidden => error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "You do not have access to this resource.",
            request_id,
        ),
        AccessError::NotFound => error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.",
            request_id,
        ),
        AccessError::InvalidRequest | AccessError::InvalidToken => error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request is invalid.",
            request_id,
        ),
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Project access is temporarily unavailable.",
            request_id,
        ),
    }
}

/// Authenticate, authorize the project, and enforce `intent`.
pub async fn authorize(
    access: &ProjectAccess,
    headers: &HeaderMap,
    project_id: &str,
    intent: Intent,
) -> Result<AuthorizedProject, Response> {
    let request_id = request_id(headers);
    let principal = crate::routes::workspace::authenticate(access, headers)
        .await
        .map_err(|failure| access_error(failure, &request_id))?;
    if uuid::Uuid::parse_str(project_id).is_err() {
        return Err(access_error(AccessError::NotFound, &request_id));
    }
    let project = access
        .authorize_project(&principal, project_id)
        .await
        .map_err(|failure| access_error(failure, &request_id))?;
    if intent == Intent::Write
        && (!project.principal.may_write() || !matches!(project.role, Role::Owner | Role::Admin))
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "A project owner or administrator session, or a write-scoped key, is required.",
            &request_id,
        ));
    }
    Ok(project)
}
