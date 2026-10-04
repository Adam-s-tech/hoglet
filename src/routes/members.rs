//! Team routes: members, role changes and invite links (see
//! `control_members.rs` for the rules and `contract/members.rs` for shapes).
//!
//! Members and invites are managed by session only. The two invite-token
//! routes (`/api/invites/preview`, `/api/invites/accept`) are public: the
//! token is the credential. They share the sign-in throttle, so guessing
//! tokens or passwords is delayed per address and per invited email, and the
//! token travels in the request body, never in a URL that servers log.

use std::sync::Arc;
use std::time::Instant;

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use uuid::Uuid;

use crate::contract::members::{
    AcceptInviteRequest, CreateInviteRequest, InviteTokenRequest, UpdateMemberRequest,
};
use crate::control::{AccessError, Principal, ProjectAccess};
use crate::control_members::MemberError;
use crate::routes::guard;
use crate::security::{LoginThrottle, PeerAddr, SecurityConfig, forwarded_https, source_key};

/// Invite requests are tiny; anything bigger is not one.
const MAX_BODY_BYTES: usize = 8 * 1024;

#[derive(Clone)]
struct MembersState {
    access: Arc<ProjectAccess>,
    security: Arc<SecurityConfig>,
    throttle: Arc<LoginThrottle>,
}

pub fn router(
    access: Arc<ProjectAccess>,
    security: SecurityConfig,
    throttle: Arc<LoginThrottle>,
) -> Router {
    let state = MembersState {
        access,
        security: Arc::new(security),
        throttle,
    };
    Router::new()
        .route(
            "/api/organizations/{organization_id}/members",
            get(list_members),
        )
        .route(
            "/api/organizations/{organization_id}/members/{user_id}",
            delete(remove_member).patch(update_member),
        )
        .route(
            "/api/organizations/{organization_id}/invites",
            get(list_invites).post(create_invite),
        )
        .route(
            "/api/organizations/{organization_id}/invites/{invite_id}",
            delete(revoke_invite),
        )
        .route("/api/invites/preview", post(preview_invite))
        .route("/api/invites/accept", post(accept_invite))
        .with_state(state)
}

fn fail(error: MemberError, request_id: &str) -> Response {
    match error {
        MemberError::Access(error) => guard::access_error(error, request_id),
        MemberError::Conflict(conflict) => {
            guard::error(StatusCode::CONFLICT, conflict.code(), conflict.message(), request_id)
        }
        MemberError::InvalidInvite => guard::error(
            StatusCode::NOT_FOUND,
            "invite_invalid",
            "This invite link is invalid, expired or already used.",
            request_id,
        ),
        MemberError::Invalid(message) => {
            guard::error(StatusCode::BAD_REQUEST, "invalid_request", message, request_id)
        }
    }
}

fn bad_body(request_id: &str) -> Response {
    guard::error(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "The request is invalid.",
        request_id,
    )
}

fn throttled(wait: std::time::Duration, request_id: &str) -> Response {
    let mut response = guard::error(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        "Too many failed attempts. Wait and try again.",
        request_id,
    );
    let seconds = wait.as_secs().max(1) + u64::from(wait.subsec_nanos() > 0);
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Authenticate and parse the organization id. A malformed id is a 404 before
/// any lookup.
async fn caller(
    state: &MembersState,
    headers: &HeaderMap,
    organization_id: &str,
) -> Result<(Principal, String), Response> {
    let request_id = guard::request_id(headers);
    let principal = crate::routes::workspace::authenticate(&state.access, headers)
        .await
        .map_err(|error| guard::access_error(error, &request_id))?;
    if Uuid::parse_str(organization_id).is_err() {
        return Err(guard::access_error(AccessError::NotFound, &request_id));
    }
    Ok((principal, request_id))
}

/// A malformed id is a 404 before any lookup.
fn bad_id(value: &str, request_id: &str) -> Option<Response> {
    Uuid::parse_str(value)
        .is_err()
        .then(|| guard::access_error(AccessError::NotFound, request_id))
}

async fn list_members(
    State(state): State<MembersState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (principal, request_id) = match caller(&state, &headers, &organization_id).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match state.access.list_members(&principal, &organization_id).await {
        Ok(members) => Json(members).into_response(),
        Err(error) => fail(error, &request_id),
    }
}

async fn update_member(
    State(state): State<MembersState>,
    Path((organization_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<UpdateMemberRequest>, JsonRejection>,
) -> Response {
    let (principal, request_id) = match caller(&state, &headers, &organization_id).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if let Some(response) = bad_id(&user_id, &request_id) {
        return response;
    }
    let Ok(Json(body)) = body else {
        return bad_body(&request_id);
    };
    match state
        .access
        .update_member_role(&principal, &organization_id, &user_id, body.role)
        .await
    {
        Ok(member) => Json(member).into_response(),
        Err(error) => fail(error, &request_id),
    }
}

async fn remove_member(
    State(state): State<MembersState>,
    Path((organization_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (principal, request_id) = match caller(&state, &headers, &organization_id).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if let Some(response) = bad_id(&user_id, &request_id) {
        return response;
    }
    match state
        .access
        .remove_member(&principal, &organization_id, &user_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fail(error, &request_id),
    }
}

async fn list_invites(
    State(state): State<MembersState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (principal, request_id) = match caller(&state, &headers, &organization_id).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    match state.access.list_invites(&principal, &organization_id).await {
        Ok(invites) => Json(invites).into_response(),
        Err(error) => fail(error, &request_id),
    }
}

async fn create_invite(
    State(state): State<MembersState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<CreateInviteRequest>, JsonRejection>,
) -> Response {
    let (principal, request_id) = match caller(&state, &headers, &organization_id).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let Ok(Json(body)) = body else {
        return bad_body(&request_id);
    };
    match state
        .access
        .create_invite(&principal, &organization_id, &body.email, body.role)
        .await
    {
        Ok(created) => {
            let mut response = (StatusCode::CREATED, Json(created)).into_response();
            // The token is a credential: keep it out of every cache.
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => fail(error, &request_id),
    }
}

async fn revoke_invite(
    State(state): State<MembersState>,
    Path((organization_id, invite_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (principal, request_id) = match caller(&state, &headers, &organization_id).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if let Some(response) = bad_id(&invite_id, &request_id) {
        return response;
    }
    match state
        .access
        .revoke_invite(&principal, &organization_id, &invite_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fail(error, &request_id),
    }
}

// ---------------------------------------------------------------------------
// Public: the invite token is the credential
// ---------------------------------------------------------------------------

/// Throttle key for failed token guesses from one source address. Skipped when
/// the source is unknown (behind an untrusted proxy): a shared bucket would let
/// a stranger block every invitee.
fn token_key(source: Option<&str>) -> Option<String> {
    source.map(|source| format!("invite:{source}"))
}

/// `Some(response)` when this source has guessed wrong too often.
fn token_gate(state: &MembersState, source: Option<&str>, request_id: &str) -> Option<Response> {
    let key = token_key(source)?;
    state
        .throttle
        .check(&key, source, Instant::now())
        .err()
        .map(|wait| throttled(wait, request_id))
}

fn token_failed(state: &MembersState, source: Option<&str>) {
    if let Some(key) = token_key(source) {
        state.throttle.record_failure(&key, source, Instant::now());
    }
}

/// Parse a small JSON body from a public request, keeping the parts.
async fn public_body<T: serde::de::DeserializeOwned>(
    state: &MembersState,
    request: Request,
) -> Result<(T, HeaderMap, Option<String>, String), Response> {
    let (parts, body) = request.into_parts();
    let request_id = guard::request_id(&parts.headers);
    let source = source_key(
        parts.extensions.get::<PeerAddr>().map(|peer| peer.0),
        &parts.headers,
        state.security.trust_proxy,
    );
    let bytes = to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| bad_body(&request_id))?;
    let value = serde_json::from_slice(&bytes).map_err(|_| bad_body(&request_id))?;
    Ok((value, parts.headers, source, request_id))
}

async fn preview_invite(State(state): State<MembersState>, request: Request) -> Response {
    let (body, _headers, source, request_id) =
        match public_body::<InviteTokenRequest>(&state, request).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    if let Some(response) = token_gate(&state, source.as_deref(), &request_id) {
        return response;
    }
    match state.access.invite_preview(&body.token).await {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => {
            if matches!(error, MemberError::InvalidInvite) {
                token_failed(&state, source.as_deref());
            }
            fail(error, &request_id)
        }
    }
}

async fn accept_invite(State(state): State<MembersState>, request: Request) -> Response {
    let (body, headers, source, request_id) =
        match public_body::<AcceptInviteRequest>(&state, request).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    if let Some(response) = token_gate(&state, source.as_deref(), &request_id) {
        return response;
    }
    let preview = match state.access.invite_preview(&body.token).await {
        Ok(preview) => preview,
        Err(error) => {
            if matches!(error, MemberError::InvalidInvite) {
                token_failed(&state, source.as_deref());
            }
            return fail(error, &request_id);
        }
    };
    // An existing account signs in with its password: same per-email delay and
    // per-source cap as the login form, before any hashing.
    if preview.account_exists
        && let Err(wait) = state
            .throttle
            .check(&preview.email, source.as_deref(), Instant::now())
    {
        return throttled(wait, &request_id);
    }
    match state
        .access
        .accept_invite(&body.token, body.name.as_deref(), &body.password)
        .await
    {
        Ok(result) => {
            if preview.account_exists {
                state.throttle.record_success(&preview.email);
            }
            let mut response = Json(result.workspace).into_response();
            let secure = state.security.secure_cookies || forwarded_https(&headers);
            crate::routes::workspace::set_session_cookie(
                response.headers_mut(),
                &result.session_id,
                secure,
            );
            response
        }
        Err(error) => {
            match &error {
                MemberError::Access(AccessError::InvalidCredentials) => {
                    state.throttle.record_failure(
                        &preview.email,
                        source.as_deref(),
                        Instant::now(),
                    );
                }
                MemberError::InvalidInvite => token_failed(&state, source.as_deref()),
                _ => {}
            }
            fail(error, &request_id)
        }
    }
}
