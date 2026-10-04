//! Workspace and authentication HTTP routes backed by authoritative Control State.

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Request, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::control::{
    AccessError, LoginRequest, PersonalApiKey, Principal, ProjectAccess, SetupRequest,
};
use crate::security::{
    LoginThrottle, PeerAddr, SecurityConfig, constant_time_eq, forwarded_https, source_key,
};

const SESSION_COOKIE: &str = "hoglet_sid";
const SESSION_MAX_AGE_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone)]
struct WorkspaceState {
    access: Arc<ProjectAccess>,
    security: Arc<SecurityConfig>,
    throttle: Arc<LoginThrottle>,
}

#[derive(Clone)]
struct RequestId(String);

/// Where a request came from, for the login throttle (`None` when the peer is
/// the reverse proxy itself and no trusted forwarding header names the client).
#[derive(Clone)]
struct Source(Option<String>);

#[derive(Debug, Deserialize)]
struct SetupBody {
    email: String,
    password: String,
    organization_name: String,
    #[serde(default)]
    project_name: Option<String>,
    #[serde(default)]
    existing_project_token: Option<String>,
    #[serde(default)]
    setup_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginBody {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct NameBody {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CreateKeyBody {
    name: String,
    #[serde(default)]
    scope: crate::control::KeyScope,
}

#[derive(Serialize)]
struct BootstrapResponse {
    setup_required: bool,
}

#[derive(Serialize)]
struct CreatedKeyResponse {
    key: PersonalApiKey,
    secret: String,
}

#[derive(Serialize)]
struct StatusResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
    request_id: String,
}

/// Builds the dashboard workspace surface over one authoritative access service.
pub fn router(access: Arc<ProjectAccess>) -> Router {
    router_with(access, SecurityConfig::default())
}

/// [`router`] with operator security settings (setup token, proxy trust,
/// cookie policy).
pub fn router_with(access: Arc<ProjectAccess>, security: SecurityConfig) -> Router {
    let state = WorkspaceState {
        access,
        security: Arc::new(security),
        throttle: Arc::new(LoginThrottle::default()),
    };
    Router::new()
        .route("/api/auth/bootstrap", get(bootstrap))
        .route("/api/auth/setup", post(setup))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/auth/keys", get(list_keys).post(create_key))
        .route("/api/auth/keys/{key_id}", delete(revoke_key))
        .route(
            "/api/organizations",
            get(list_organizations).post(create_organization),
        )
        .route(
            "/api/organizations/{organization_id}/projects",
            post(create_project),
        )
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, assign_request_id))
}

async fn assign_request_id(
    State(state): State<WorkspaceState>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = RequestId(Uuid::now_v7().to_string());
    request.extensions_mut().insert(request_id.clone());
    let peer = request.extensions().get::<PeerAddr>().map(|peer| peer.0);
    let source = source_key(peer, request.headers(), state.security.trust_proxy);
    request.extensions_mut().insert(Source(source));
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

async fn bootstrap(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.access.setup_required().await {
        Ok(setup_required) => Json(BootstrapResponse { setup_required }).into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn setup(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Result<Json<SetupBody>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return invalid_request(&request_id, None),
    };
    if let Some(expected) = state.security.setup_token.as_deref() {
        let presented = headers
            .get("x-hoglet-setup-token")
            .and_then(|value| value.to_str().ok())
            .or(body.setup_token.as_deref())
            .unwrap_or_default();
        if !constant_time_eq(expected, presented) {
            return error_response(
                StatusCode::FORBIDDEN,
                "forbidden",
                "The setup token is missing or wrong.",
                Some("setup_token"),
                &request_id,
            );
        }
    }
    if body.email.trim().is_empty() {
        return invalid_request(&request_id, Some("email"));
    }
    if body.password.is_empty() {
        return invalid_request(&request_id, Some("password"));
    }
    if body.organization_name.trim().is_empty() {
        return invalid_request(&request_id, Some("organization_name"));
    }
    let project_name = body.project_name.unwrap_or_else(|| "Default".into());
    if project_name.trim().is_empty() {
        return invalid_request(&request_id, Some("project_name"));
    }

    match state
        .access
        .setup(SetupRequest {
            email: body.email,
            password: body.password,
            organization_name: body.organization_name,
            project_name,
            existing_project_token: body.existing_project_token,
        })
        .await
    {
        Ok(result) => {
            let mut response = Json(result.workspace).into_response();
            set_session_cookie(
                response.headers_mut(),
                &result.session_id,
                secure_cookie(&state, &headers),
            );
            response
        }
        Err(error) => access_error(error, &request_id),
    }
}

async fn login(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    Extension(Source(source)): Extension<Source>,
    headers: HeaderMap,
    body: Result<Json<LoginBody>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return invalid_request(&request_id, None),
    };
    if body.email.trim().is_empty() {
        return invalid_request(&request_id, Some("email"));
    }
    if body.password.is_empty() {
        return invalid_request(&request_id, Some("password"));
    }
    let now = std::time::Instant::now();
    if let Err(wait) = state.throttle.check(&body.email, source.as_deref(), now) {
        let mut response = error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            "Too many failed sign-in attempts. Wait and try again.",
            None,
            &request_id,
        );
        let seconds = wait.as_secs().max(1) + u64::from(wait.subsec_nanos() > 0);
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return response;
    }
    let email = body.email.clone();
    match state
        .access
        .login(LoginRequest {
            email: body.email,
            password: body.password,
        })
        .await
    {
        Ok(result) => {
            state.throttle.record_success(&email);
            let mut response = Json(result.workspace).into_response();
            set_session_cookie(
                response.headers_mut(),
                &result.session_id,
                secure_cookie(&state, &headers),
            );
            response
        }
        Err(error) => {
            if matches!(error, AccessError::InvalidCredentials) {
                state
                    .throttle
                    .record_failure(&email, source.as_deref(), std::time::Instant::now());
            }
            access_error(error, &request_id)
        }
    }
}

async fn logout(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate(&state.access, &headers).await {
        return access_error(error, &request_id);
    }
    if let Some(session_id) = session_id(&headers)
        && let Err(error) = state.access.logout(&session_id).await
    {
        return access_error(error, &request_id);
    }
    let mut response = Json(StatusResponse { status: "ok" }).into_response();
    clear_session_cookie(response.headers_mut(), secure_cookie(&state, &headers));
    response
}

async fn me(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    match state.access.workspace(&principal).await {
        Ok(workspace) => Json(workspace).into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn list_keys(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    match state.access.list_personal_keys(&principal).await {
        Ok(keys) => Json(keys).into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn create_key(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Result<Json<CreateKeyBody>, JsonRejection>,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return invalid_request(&request_id, None),
    };
    if body.name.trim().is_empty() {
        return invalid_request(&request_id, Some("name"));
    }
    match state
        .access
        .create_personal_key(&principal, &body.name, body.scope)
        .await
    {
        Ok(created) => (
            StatusCode::CREATED,
            Json(CreatedKeyResponse {
                key: created.key,
                secret: created.secret,
            }),
        )
            .into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn revoke_key(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    axum::extract::Path(key_id): axum::extract::Path<String>,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    match state.access.revoke_personal_key(&principal, &key_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn list_organizations(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    match state.access.workspace(&principal).await {
        Ok(workspace) => Json(workspace.organizations).into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn create_organization(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Result<Json<NameBody>, JsonRejection>,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return invalid_request(&request_id, None),
    };
    if body.name.trim().is_empty() {
        return invalid_request(&request_id, Some("name"));
    }
    match state
        .access
        .create_organization(&principal, &body.name)
        .await
    {
        Ok(organization) => (StatusCode::CREATED, Json(organization)).into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

async fn create_project(
    State(state): State<WorkspaceState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    axum::extract::Path(organization_id): axum::extract::Path<String>,
    body: Result<Json<NameBody>, JsonRejection>,
) -> Response {
    let principal = match authenticate(&state.access, &headers).await {
        Ok(principal) => principal,
        Err(error) => return access_error(error, &request_id),
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return invalid_request(&request_id, None),
    };
    if body.name.trim().is_empty() {
        return invalid_request(&request_id, Some("name"));
    }
    match state
        .access
        .create_project(&principal, &organization_id, &body.name)
        .await
    {
        Ok(project) => (StatusCode::CREATED, Json(project)).into_response(),
        Err(error) => access_error(error, &request_id),
    }
}

/// Resolves either a session cookie or a `phx_` bearer key to its dashboard principal.
pub async fn authenticate(
    access: &ProjectAccess,
    headers: &HeaderMap,
) -> Result<Principal, AccessError> {
    if let Some(session_id) = session_id(headers)
        && let Ok(principal) = access.validate_session(&session_id).await
    {
        return Ok(principal);
    }
    if let Some(secret) = bearer_key(headers) {
        return access.validate_personal_key(secret).await;
    }
    Err(AccessError::Unauthorized)
}

fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == SESSION_COOKIE && !value.is_empty()).then(|| value.to_owned())
            })
        })
}

fn bearer_key(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| value.starts_with("phx_"))
}

/// `Secure` when the operator forces it or the proxy says the request was TLS.
fn secure_cookie(state: &WorkspaceState, headers: &HeaderMap) -> bool {
    state.security.secure_cookies || forwarded_https(headers)
}

fn set_session_cookie(headers: &mut HeaderMap, session_id: &str, secure: bool) {
    let secure = if secure { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE}={session_id}; Max-Age={SESSION_MAX_AGE_SECONDS}; Path=/; HttpOnly; SameSite=Lax{secure}"
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, value);
    }
}

fn clear_session_cookie(headers: &mut HeaderMap, secure: bool) {
    let secure = if secure { "; Secure" } else { "" };
    let cookie = format!("{SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax{secure}");
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, value);
    }
}

fn invalid_request(request_id: &RequestId, field: Option<&'static str>) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "The request is invalid.",
        field,
        request_id,
    )
}

fn access_error(error: AccessError, request_id: &RequestId) -> Response {
    let (status, code, message) = match error {
        AccessError::InvalidRequest | AccessError::InvalidToken => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request is invalid.",
        ),
        AccessError::InvalidCredentials | AccessError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication is required.",
        ),
        AccessError::Forbidden => (
            StatusCode::FORBIDDEN,
            "forbidden",
            "You do not have access to this resource.",
        ),
        AccessError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.",
        ),
        AccessError::SetupComplete => (
            StatusCode::CONFLICT,
            "conflict",
            "Initial setup has already been completed.",
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
    error_response(status, code, message, None, request_id)
}

fn error_response(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    field: Option<&'static str>,
    request_id: &RequestId,
) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            error: ErrorBody {
                code,
                message,
                field,
                request_id: request_id.0.clone(),
            },
        }),
    )
        .into_response()
}
