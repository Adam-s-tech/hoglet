//! Feature flag HTTP surfaces.
//!
//! - **Wire** (`/flags`, `/decide`): one handler, `?v=` selects the response
//!   shape. Shapes are the ones `@posthog/core` and posthog-js parse:
//!   `/flags?v=2` and `/decide?v=4` → `{flags: {key: FlagDetails}}`;
//!   `/flags` (no `v`, `v=1`) and `/decide?v=3` → `{featureFlags,
//!   featureFlagPayloads}`; `/decide?v=2` → `{featureFlags: map}`;
//!   `/decide?v=1` → `{featureFlags: [enabled keys]}`. Every shape flattens
//!   the remote-config fields. Payloads go out as JSON strings, as PostHog
//!   sends them; the SDKs `JSON.parse` them back.
//! - **Local evaluation** (`/flags/definitions`, `/api/feature_flag/
//!   local_evaluation`): PostHog flag definitions for server SDKs, behind a
//!   personal API key that must have access to the token's project.
//! - **Dashboard API** (`/api/projects/{project_id}/feature_flags…`): CRUD +
//!   "test a user", under the same project authorization as every other
//!   dashboard resource.
//!
//! Response codes are load-bearing: 4xx is never retried by the SDKs, 5xx is.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    body::{Bytes, to_bytes},
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use uuid::Uuid;

use crate::capture::CaptureAuthorizer;
use crate::contract::common::{ApiError, ApiErrorBody};
use crate::contract::flags::{FeatureFlag, FeatureFlagInput, FlagEvaluation};
use crate::control::{AccessError, Authentication, AuthorizedProject, ProjectAccess, Role};
use crate::flags::{
    FeatureFlagPatch, FlagStore, FlagStoreError, SubjectHints, needs_person, resolve_subject,
};
use crate::persons::PersonStore;
use crate::token;

/// Compressed request bodies above this are refused before decoding.
const MAX_FLAGS_BODY_BYTES: usize = 1024 * 1024;
/// Decoded request text above this is refused.
const MAX_FLAGS_DECODED_BYTES: usize = 1024 * 1024;
/// JSON nesting deeper than this is refused before parsing (json5 recurses).
const MAX_JSON_DEPTH: usize = 64;
/// PostHog truncates distinct ids to this many characters.
const MAX_DISTINCT_ID_CHARS: usize = 200;
/// Dashboard request bodies.
const MAX_API_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize, Default)]
struct FlagsQuery {
    v: Option<String>,
    compression: Option<String>,
}

#[derive(Clone)]
struct WireState {
    store: Arc<FlagStore>,
    persons: Arc<PersonStore>,
    authorizer: Arc<dyn CaptureAuthorizer>,
}

#[derive(Clone)]
struct ApiState {
    access: Arc<ProjectAccess>,
    store: Arc<FlagStore>,
    persons: Arc<PersonStore>,
}

/// Public SDK endpoints: `/flags` and `/decide`, token-authorized.
pub fn wire_router(
    store: Arc<FlagStore>,
    persons: Arc<PersonStore>,
    authorizer: Arc<dyn CaptureAuthorizer>,
) -> Router {
    Router::new()
        .route("/flags", post(flags_endpoint))
        .route("/flags/", post(flags_endpoint))
        .route("/decide", post(decide_endpoint))
        .route("/decide/", post(decide_endpoint))
        .layer(DefaultBodyLimit::max(MAX_FLAGS_BODY_BYTES))
        .with_state(WireState {
            store,
            persons,
            authorizer,
        })
}

/// Personal-key and session surfaces: local evaluation and the dashboard API.
pub fn api_router(
    access: Arc<ProjectAccess>,
    store: Arc<FlagStore>,
    persons: Arc<PersonStore>,
) -> Router {
    Router::new()
        .route("/flags/definitions", get(local_evaluation))
        .route("/flags/definitions/", get(local_evaluation))
        .route("/api/feature_flag/local_evaluation", get(local_evaluation))
        .route("/api/feature_flag/local_evaluation/", get(local_evaluation))
        .route(
            "/api/projects/{project_id}/feature_flags",
            get(list_flags).post(create_flag),
        )
        .route(
            "/api/projects/{project_id}/feature_flags/{id}",
            get(get_flag).patch(update_flag).delete(delete_flag),
        )
        .route(
            "/api/projects/{project_id}/feature_flags/{id}/evaluate",
            get(evaluate_flag),
        )
        .with_state(ApiState {
            access,
            store,
            persons,
        })
        .layer(middleware::from_fn(assign_request_id))
}

// ---------------------------------------------------------------------------
// Wire: /flags and /decide
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Flags,
    Decide,
}

/// Which response shape a request gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `{flags: {key: FlagDetails}}` — `/flags?v>=2`, `/decide?v>=4`.
    Detailed,
    /// `{featureFlags: map, featureFlagPayloads}` — `/flags` v1, `/decide?v=3`.
    Legacy,
    /// `{featureFlags: map}` — `/decide?v=2`.
    DecideMap,
    /// `{featureFlags: [enabled keys]}` — `/decide?v=1` (and no `v`).
    DecideList,
}

impl Shape {
    fn select(endpoint: Endpoint, version: Option<&str>) -> Self {
        let version = version.and_then(|v| v.trim().parse::<u32>().ok());
        match endpoint {
            Endpoint::Flags => match version {
                Some(v) if v >= 2 => Self::Detailed,
                _ => Self::Legacy,
            },
            Endpoint::Decide => match version {
                None | Some(0) | Some(1) => Self::DecideList,
                Some(2) => Self::DecideMap,
                Some(3) => Self::Legacy,
                Some(_) => Self::Detailed,
            },
        }
    }
}

/// One evaluated flag, with what the wire metadata needs.
struct Evaluated {
    key: String,
    id: i64,
    version: i64,
    name: String,
    evaluation: crate::flags::Evaluation,
}

async fn flags_endpoint(
    State(state): State<WireState>,
    Query(query): Query<FlagsQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    evaluate_wire(state, Endpoint::Flags, query, headers, body).await
}

async fn decide_endpoint(
    State(state): State<WireState>,
    Query(query): Query<FlagsQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    evaluate_wire(state, Endpoint::Decide, query, headers, body).await
}

async fn evaluate_wire(
    state: WireState,
    endpoint: Endpoint,
    query: FlagsQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match parse_request(&headers, query.compression.as_deref(), &body) {
        Ok(request) => request,
        Err(detail) => {
            return wire_error(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "invalid_payload",
                detail,
            );
        }
    };
    let Some(raw_token) = request
        .get("token")
        .or_else(|| request.get("api_key"))
        .and_then(Value::as_str)
    else {
        return wire_unauthorized();
    };
    if token::validate(raw_token).is_err() {
        return wire_unauthorized();
    }
    let project_id = match state.authorizer.authorize(raw_token).await {
        Ok(project) => project.project_id,
        Err(_) => return wire_unauthorized(),
    };
    let Some(distinct_id) = distinct_id(&request) else {
        return wire_error(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "missing_distinct_id",
            "The distinct_id field is missing from the request.",
        );
    };

    let shape = Shape::select(endpoint, query.v.as_deref());
    let disabled = request.get("disable_flags") == Some(&Value::Bool(true));
    // posthog-node sends `flag_keys_to_evaluate`, posthog-js `flag_keys`.
    let only_keys: Option<HashSet<String>> = request
        .get("flag_keys_to_evaluate")
        .or_else(|| request.get("flag_keys"))
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
    let person_properties = request
        .get("person_properties")
        .and_then(Value::as_object)
        .cloned();
    let anon_distinct_id = request
        .get("$anon_distinct_id")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let store = state.store;
    let persons = state.persons;
    let outcome = tokio::task::spawn_blocking(move || {
        if disabled {
            return Ok((Vec::new(), false));
        }
        let project = store.compiled(&project_id)?;
        let selected: Vec<_> = project
            .flags
            .iter()
            .filter(|flag| flag.flag.active)
            .filter(|flag| {
                only_keys
                    .as_ref()
                    .is_none_or(|keys| keys.contains(&flag.flag.key))
            })
            .collect();
        let hints = SubjectHints {
            anon_distinct_id: anon_distinct_id.as_deref(),
            person_properties: person_properties.as_ref(),
        };
        let (subject, person_failed) = resolve_subject(
            &persons,
            &project_id,
            &distinct_id,
            &hints,
            needs_person(selected.iter().copied()),
        );
        let evaluated = selected
            .into_iter()
            .map(|flag| Evaluated {
                key: flag.flag.key.clone(),
                id: flag.flag.id,
                version: flag.version,
                name: flag.flag.name.clone(),
                evaluation: subject.evaluate(flag),
            })
            .collect::<Vec<_>>();
        Ok::<_, FlagStoreError>((evaluated, person_failed))
    })
    .await;
    let (evaluated, errors) = match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            tracing::error!(%error, "flag definitions unavailable");
            return wire_unavailable();
        }
        Err(_) => return wire_unavailable(),
    };
    Json(Value::Object(render(shape, &evaluated, errors))).into_response()
}

/// Decode (form `data=`, base64, gzip, raw) and parse (JSON, then json5).
fn parse_request(
    headers: &HeaderMap,
    compression: Option<&str>,
    body: &[u8],
) -> Result<Map<String, Value>, &'static str> {
    let form_encoded = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/x-www-form-urlencoded"));
    let text = crate::capture::decompress::decode(body, form_encoded, compression)
        .map_err(|_| "The request body could not be decoded.")?;
    if text.len() > MAX_FLAGS_DECODED_BYTES {
        return Err("The request body is too large.");
    }
    if json_depth_exceeds(&text, MAX_JSON_DEPTH) {
        return Err("The request body is nested too deeply.");
    }
    // json5 tolerates what SDKs actually send (NaN, Infinity); plain JSON
    // takes the fast path.
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => json5::from_str(&text).map_err(|_| "The request body is not JSON.")?,
    };
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err("The request body must be a JSON object."),
    }
}

/// Bracket depth outside string literals. Conservative: a stray bracket in
/// an unterminated string only makes the check stricter.
fn json_depth_exceeds(text: &str, limit: usize) -> bool {
    let mut depth = 0_usize;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;
    for byte in text.bytes() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                in_string = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => in_string = Some(byte),
            b'[' | b'{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn distinct_id(request: &Map<String, Value>) -> Option<String> {
    let raw = match request.get("distinct_id")? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    Some(raw.chars().take(MAX_DISTINCT_ID_CHARS).collect())
}

/// The wire form of a payload: PostHog stores payloads as JSON strings and
/// sends them verbatim; structured payloads are encoded to that form.
fn payload_wire(payload: &Value) -> Value {
    match payload {
        Value::String(text) => Value::String(text.clone()),
        other => Value::String(other.to_string()),
    }
}

fn flag_value(evaluation: &crate::flags::Evaluation) -> Value {
    match &evaluation.variant {
        Some(variant) if evaluation.enabled => Value::String(variant.clone()),
        _ => Value::Bool(evaluation.enabled),
    }
}

fn render(shape: Shape, evaluated: &[Evaluated], errors: bool) -> Map<String, Value> {
    let mut body = crate::routes::config::remote_config_fields();
    body.insert("errorsWhileComputingFlags".into(), Value::Bool(errors));
    body.insert("requestId".into(), json!(Uuid::new_v4().to_string()));
    match shape {
        Shape::Detailed => {
            let mut flags = Map::new();
            for flag in evaluated {
                let evaluation = &flag.evaluation;
                let mut metadata = Map::new();
                metadata.insert("id".into(), json!(flag.id));
                metadata.insert("version".into(), json!(flag.version));
                metadata.insert(
                    "description".into(),
                    if flag.name.is_empty() {
                        Value::Null
                    } else {
                        Value::String(flag.name.clone())
                    },
                );
                if evaluation.enabled
                    && let Some(payload) = &evaluation.payload
                {
                    metadata.insert("payload".into(), payload_wire(payload));
                }
                flags.insert(
                    flag.key.clone(),
                    json!({
                        "key": flag.key,
                        "enabled": evaluation.enabled,
                        "variant": evaluation.variant,
                        "reason": {
                            "code": evaluation.reason.as_str(),
                            "condition_index": evaluation.condition_index,
                            "description": evaluation.reason.description(evaluation.condition_index),
                        },
                        "metadata": metadata,
                    }),
                );
            }
            body.insert("flags".into(), Value::Object(flags));
            body.insert("quotaLimited".into(), json!([]));
            body.insert(
                "evaluatedAt".into(),
                json!(chrono::Utc::now().timestamp_millis()),
            );
        }
        Shape::Legacy | Shape::DecideMap => {
            let mut values = Map::new();
            let mut payloads = Map::new();
            for flag in evaluated {
                values.insert(flag.key.clone(), flag_value(&flag.evaluation));
                if flag.evaluation.enabled
                    && let Some(payload) = &flag.evaluation.payload
                {
                    payloads.insert(flag.key.clone(), payload_wire(payload));
                }
            }
            body.insert("featureFlags".into(), Value::Object(values));
            if shape == Shape::Legacy {
                body.insert("featureFlagPayloads".into(), Value::Object(payloads));
            }
        }
        Shape::DecideList => {
            let keys: Vec<Value> = evaluated
                .iter()
                .filter(|flag| flag.evaluation.enabled)
                .map(|flag| Value::String(flag.key.clone()))
                .collect();
            body.insert("featureFlags".into(), Value::Array(keys));
        }
    }
    body
}

fn wire_error(status: StatusCode, kind: &str, code: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({"type": kind, "code": code, "detail": detail, "attr": null})),
    )
        .into_response()
}

fn wire_unauthorized() -> Response {
    wire_error(
        StatusCode::UNAUTHORIZED,
        "authentication_error",
        "invalid_api_key",
        "Project API key invalid. You can find your project API key in your Hoglet project settings.",
    )
}

fn wire_unavailable() -> Response {
    wire_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "server_error",
        "unavailable",
        "Feature flags are temporarily unavailable.",
    )
}

// ---------------------------------------------------------------------------
// Local evaluation for server SDKs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LocalEvaluationQuery {
    token: Option<String>,
}

async fn local_evaluation(
    State(state): State<ApiState>,
    Query(query): Query<LocalEvaluationQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(secret) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| value.starts_with("phx_"))
    else {
        return local_error(StatusCode::UNAUTHORIZED, "A personal API key is required.");
    };
    let Some(project_token) = query.token.filter(|value| token::validate(value).is_ok()) else {
        return local_error(
            StatusCode::UNAUTHORIZED,
            "A valid project token is required.",
        );
    };
    let project_id = match state.access.authorize_capture(&project_token).await {
        Ok(project) => project.project_id,
        Err(AccessError::Unavailable) => {
            return local_error(StatusCode::SERVICE_UNAVAILABLE, "Try again later.");
        }
        Err(_) => return local_error(StatusCode::UNAUTHORIZED, "Unknown project token."),
    };
    let principal = match state.access.validate_personal_key(secret).await {
        Ok(principal) => principal,
        Err(AccessError::Unavailable) => {
            return local_error(StatusCode::SERVICE_UNAVAILABLE, "Try again later.");
        }
        Err(_) => return local_error(StatusCode::UNAUTHORIZED, "Invalid personal API key."),
    };
    match state
        .access
        .authorize_project(&principal, &project_id)
        .await
    {
        Ok(_) => {}
        Err(AccessError::Unavailable) => {
            return local_error(StatusCode::SERVICE_UNAVAILABLE, "Try again later.");
        }
        Err(_) => {
            return local_error(
                StatusCode::FORBIDDEN,
                "This personal API key cannot access the project.",
            );
        }
    }

    let store = state.store;
    let definitions =
        match tokio::task::spawn_blocking(move || store.definitions(&project_id)).await {
            Ok(Ok(definitions)) => definitions,
            Ok(Err(error)) => {
                tracing::error!(%error, "flag definitions unavailable");
                return local_error(StatusCode::SERVICE_UNAVAILABLE, "Try again later.");
            }
            Err(_) => return local_error(StatusCode::SERVICE_UNAVAILABLE, "Try again later."),
        };
    let flags: Vec<Value> = definitions
        .iter()
        .map(|(flag, version)| posthog_definition(flag, *version))
        .collect();
    let body = json!({
        "flags": flags,
        "group_type_mapping": {},
        "cohorts": {},
    });
    let Ok(bytes) = serde_json::to_vec(&body) else {
        return local_error(StatusCode::INTERNAL_SERVER_ERROR, "Encoding failed.");
    };
    let etag = format!("\"{}\"", hex::encode(Sha1::digest(&bytes)));
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag));
    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            bytes,
        )
            .into_response()
    };
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// One flag in PostHog's local-evaluation format.
fn posthog_definition(flag: &FeatureFlag, version: i64) -> Value {
    let groups: Vec<Value> = flag
        .filters
        .groups
        .iter()
        .map(|group| {
            let properties: Vec<Value> = group
                .properties
                .iter()
                .map(|filter| {
                    json!({
                        "key": filter.key,
                        "type": "person",
                        "operator": filter.operator,
                        "value": filter.value,
                    })
                })
                .collect();
            json!({
                "properties": properties,
                "rollout_percentage": group.rollout_percentage,
                "variant": group.variant,
            })
        })
        .collect();
    let payloads: Map<String, Value> = flag
        .filters
        .payloads
        .iter()
        .map(|(key, payload)| (key.clone(), payload_wire(payload)))
        .collect();
    json!({
        "id": flag.id,
        "name": flag.name,
        "key": flag.key,
        "active": flag.active,
        "deleted": false,
        "ensure_experience_continuity": flag.ensure_experience_continuity,
        "version": version,
        "filters": {
            "groups": groups,
            "multivariate": flag.filters.multivariate,
            "payloads": payloads,
            "aggregation_group_type_index": null,
        },
    })
}

fn local_error(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail": detail}))).into_response()
}

// ---------------------------------------------------------------------------
// Dashboard API
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RequestId(String);

async fn assign_request_id(mut request: Request, next: Next) -> Response {
    let request_id = RequestId(Uuid::now_v7().to_string());
    request.extensions_mut().insert(request_id.clone());
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

/// Authenticate, then authorize the path project, before touching a body or
/// an object id. Mutations need an owner/admin session (dashboard policy).
async fn authorize(
    state: &ApiState,
    headers: &HeaderMap,
    project_id: &str,
    mutation: bool,
    request_id: &RequestId,
) -> Result<AuthorizedProject, Response> {
    let principal = crate::routes::workspace::authenticate(&state.access, headers)
        .await
        .map_err(|error| access_error(error, request_id))?;
    if Uuid::parse_str(project_id).is_err() {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.".into(),
            request_id,
        ));
    }
    let project = state
        .access
        .authorize_project(&principal, project_id)
        .await
        .map_err(|error| access_error(error, request_id))?;
    if mutation
        && (project.principal.authentication != Authentication::Session
            || !matches!(project.role, Role::Owner | Role::Admin))
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "A project owner or administrator session is required.".into(),
            request_id,
        ));
    }
    Ok(project)
}

async fn parse_body<T: DeserializeOwned>(
    request: Request,
    request_id: &RequestId,
) -> Result<T, Response> {
    let bytes = to_bytes(request.into_body(), MAX_API_BODY_BYTES)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                format!("The request body must be at most {MAX_API_BODY_BYTES} bytes."),
                request_id,
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("The request body is invalid: {error}"),
            request_id,
        )
    })
}

fn flag_not_found(request_id: &RequestId) -> Response {
    api_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "The requested feature flag was not found.".into(),
        request_id,
    )
}

async fn blocking<T, F>(operation: F) -> Result<T, FlagStoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, FlagStoreError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| FlagStoreError::Unavailable)?
}

async fn list_flags(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let project = match authorize(&state, &headers, &project_id, false, &request_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let store = state.store;
    match blocking(move || store.list(&project.project_id)).await {
        Ok(flags) => Json(flags).into_response(),
        Err(error) => store_error(error, &request_id),
    }
}

async fn create_flag(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    request: Request,
) -> Response {
    let project = match authorize(&state, request.headers(), &project_id, true, &request_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let input: FeatureFlagInput = match parse_body(request, &request_id).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let store = state.store;
    match blocking(move || store.create(&project.project_id, &input)).await {
        Ok(flag) => (StatusCode::CREATED, Json(flag)).into_response(),
        Err(error) => store_error(error, &request_id),
    }
}

async fn get_flag(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path((project_id, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let project = match authorize(&state, &headers, &project_id, false, &request_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let Ok(id) = id.parse::<i64>() else {
        return flag_not_found(&request_id);
    };
    let store = state.store;
    match blocking(move || store.get(&project.project_id, id)).await {
        Ok(flag) => Json(flag).into_response(),
        Err(error) => store_error(error, &request_id),
    }
}

async fn update_flag(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path((project_id, id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let project = match authorize(&state, request.headers(), &project_id, true, &request_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let Ok(id) = id.parse::<i64>() else {
        return flag_not_found(&request_id);
    };
    let patch: FeatureFlagPatch = match parse_body(request, &request_id).await {
        Ok(patch) => patch,
        Err(response) => return response,
    };
    let store = state.store;
    match blocking(move || store.update(&project.project_id, id, &patch)).await {
        Ok(flag) => Json(flag).into_response(),
        Err(error) => store_error(error, &request_id),
    }
}

async fn delete_flag(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path((project_id, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let project = match authorize(&state, &headers, &project_id, true, &request_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let Ok(id) = id.parse::<i64>() else {
        return flag_not_found(&request_id);
    };
    let store = state.store;
    match blocking(move || store.delete(&project.project_id, id)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => store_error(error, &request_id),
    }
}

#[derive(Debug, Deserialize)]
struct EvaluateQuery {
    distinct_id: Option<String>,
}

async fn evaluate_flag(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path((project_id, id)): Path<(String, String)>,
    Query(query): Query<EvaluateQuery>,
    headers: HeaderMap,
) -> Response {
    let project = match authorize(&state, &headers, &project_id, false, &request_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let Ok(id) = id.parse::<i64>() else {
        return flag_not_found(&request_id);
    };
    let Some(distinct_id) = query
        .distinct_id
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .chars()
                .take(MAX_DISTINCT_ID_CHARS)
                .collect::<String>()
        })
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "distinct_id is required.".into(),
            &request_id,
        );
    };
    let store = state.store;
    let persons = state.persons;
    let result = blocking(move || {
        let (flag, version) = store.get_with_version(&project.project_id, id)?;
        let compiled = crate::flags::CompiledFlag::compile(flag, version);
        let (subject, _) = resolve_subject(
            &persons,
            &project.project_id,
            &distinct_id,
            &SubjectHints::default(),
            true,
        );
        let evaluation = subject.evaluate(&compiled);
        Ok(FlagEvaluation {
            key: compiled.flag.key.clone(),
            enabled: evaluation.enabled,
            variant: evaluation.variant,
            reason: evaluation.reason.as_str().to_owned(),
            condition_index: evaluation.condition_index,
            payload: evaluation.payload,
        })
    })
    .await;
    match result {
        Ok(evaluation) => Json(evaluation).into_response(),
        Err(error) => store_error(error, &request_id),
    }
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
    api_error(status, code, message.into(), request_id)
}

fn store_error(error: FlagStoreError, request_id: &RequestId) -> Response {
    match error {
        FlagStoreError::NotFound => flag_not_found(request_id),
        FlagStoreError::Conflict => api_error(
            StatusCode::CONFLICT,
            "conflict",
            "A feature flag with this key already exists.".into(),
            request_id,
        ),
        FlagStoreError::Invalid(invalid) => api_error(
            StatusCode::BAD_REQUEST,
            "invalid_flag",
            invalid.to_string(),
            request_id,
        ),
        FlagStoreError::LimitExceeded(message) => api_error(
            StatusCode::BAD_REQUEST,
            "limit_exceeded",
            message,
            request_id,
        ),
        FlagStoreError::Unavailable | FlagStoreError::InvalidStorage => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "The service is temporarily unavailable.".into(),
            request_id,
        ),
        FlagStoreError::Database(error) => {
            tracing::error!(%error, "flag store database error");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred.".into(),
                request_id,
            )
        }
        FlagStoreError::Corrupt(message) => {
            tracing::error!(%message, "corrupt flag definition");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred.".into(),
                request_id,
            )
        }
    }
}

fn api_error(status: StatusCode, code: &str, message: String, request_id: &RequestId) -> Response {
    (
        status,
        Json(ApiError {
            error: ApiErrorBody {
                code: code.to_owned(),
                message,
                request_id: Some(request_id.0.clone()),
            },
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::ReasonCode;

    #[test]
    fn shapes_follow_endpoint_and_version() {
        use Endpoint::{Decide, Flags};
        assert_eq!(Shape::select(Flags, Some("2")), Shape::Detailed);
        assert_eq!(Shape::select(Flags, Some("3")), Shape::Detailed);
        assert_eq!(Shape::select(Flags, None), Shape::Legacy);
        assert_eq!(Shape::select(Flags, Some("1")), Shape::Legacy);
        assert_eq!(Shape::select(Flags, Some("junk")), Shape::Legacy);
        assert_eq!(Shape::select(Decide, None), Shape::DecideList);
        assert_eq!(Shape::select(Decide, Some("1")), Shape::DecideList);
        assert_eq!(Shape::select(Decide, Some("2")), Shape::DecideMap);
        assert_eq!(Shape::select(Decide, Some("3")), Shape::Legacy);
        assert_eq!(Shape::select(Decide, Some("4")), Shape::Detailed);
    }

    fn evaluated(
        key: &str,
        enabled: bool,
        variant: Option<&str>,
        payload: Option<Value>,
    ) -> Evaluated {
        Evaluated {
            key: key.into(),
            id: 7,
            version: 3,
            name: String::new(),
            evaluation: crate::flags::Evaluation {
                enabled,
                variant: variant.map(str::to_owned),
                reason: if enabled {
                    ReasonCode::ConditionMatch
                } else {
                    ReasonCode::OutOfRolloutBound
                },
                condition_index: Some(0),
                payload,
            },
        }
    }

    fn sample() -> Vec<Evaluated> {
        vec![
            evaluated("on", true, None, Some(json!({"a": 1}))),
            evaluated("off", false, None, None),
            evaluated("exp", true, Some("test"), Some(json!("plain"))),
        ]
    }

    #[test]
    fn detailed_shape_is_what_posthog_core_parses() {
        let body = render(Shape::Detailed, &sample(), false);
        let on = &body["flags"]["on"];
        assert_eq!(on["enabled"], true);
        assert_eq!(on["variant"], Value::Null);
        assert_eq!(on["reason"]["code"], "condition_match");
        assert_eq!(on["reason"]["condition_index"], 0);
        assert_eq!(on["reason"]["description"], "Matched condition set 1");
        assert_eq!(on["metadata"]["id"], 7);
        assert_eq!(on["metadata"]["version"], 3);
        assert_eq!(on["metadata"]["payload"], r#"{"a":1}"#);
        assert_eq!(body["flags"]["off"]["enabled"], false);
        assert!(body["flags"]["off"]["metadata"].get("payload").is_none());
        assert_eq!(body["flags"]["exp"]["variant"], "test");
        assert_eq!(body["flags"]["exp"]["metadata"]["payload"], "plain");
        assert_eq!(body["errorsWhileComputingFlags"], false);
        assert_eq!(body["quotaLimited"], json!([]));
        assert!(body["requestId"].is_string());
        assert!(body["evaluatedAt"].is_i64());
        // Remote config is flattened into every shape.
        assert_eq!(body["sessionRecording"], false);
        assert!(body.get("featureFlags").is_none());
        assert!(body.get("feature_flags").is_none());
    }

    #[test]
    fn legacy_shapes() {
        let legacy = render(Shape::Legacy, &sample(), true);
        assert_eq!(
            legacy["featureFlags"],
            json!({"on": true, "off": false, "exp": "test"})
        );
        assert_eq!(
            legacy["featureFlagPayloads"],
            json!({"on": r#"{"a":1}"#, "exp": "plain"})
        );
        assert_eq!(legacy["errorsWhileComputingFlags"], true);
        assert!(legacy.get("flags").is_none());
        assert_eq!(legacy["surveys"], false);

        let map = render(Shape::DecideMap, &sample(), false);
        assert_eq!(
            map["featureFlags"],
            json!({"on": true, "off": false, "exp": "test"})
        );
        assert!(map.get("featureFlagPayloads").is_none());

        let list = render(Shape::DecideList, &sample(), false);
        assert_eq!(list["featureFlags"], json!(["on", "exp"]));
        assert_eq!(list["heatmaps"], false);
    }

    #[test]
    fn depth_guard_ignores_brackets_in_strings() {
        assert!(!json_depth_exceeds(r#"{"a": "[[[[[[[["}"#, 2));
        assert!(json_depth_exceeds("[[[", 2));
        assert!(!json_depth_exceeds("[[]]", 2));
        assert!(!json_depth_exceeds(r#"{"a": "\"[[[["}"#, 2));
    }

    #[test]
    fn distinct_ids_are_strings_or_numbers_truncated_like_posthog() {
        let mut request = Map::new();
        assert_eq!(distinct_id(&request), None);
        request.insert("distinct_id".into(), json!(""));
        assert_eq!(distinct_id(&request), None);
        request.insert("distinct_id".into(), json!(42));
        assert_eq!(distinct_id(&request).as_deref(), Some("42"));
        request.insert("distinct_id".into(), json!("x".repeat(500)));
        assert_eq!(distinct_id(&request).map(|id| id.len()), Some(200));
    }

    #[test]
    fn local_definitions_use_posthog_format() {
        let flag: FeatureFlag = serde_json::from_value(json!({
            "id": 3, "key": "exp", "name": "Experiment", "active": true,
            "ensure_experience_continuity": false,
            "created_at": "", "updated_at": "",
            "filters": {
                "groups": [{"properties": [{"key": "plan", "operator": "exact", "value": ["pro"]}],
                            "rollout_percentage": 50, "variant": null}],
                "multivariate": {"variants": [{"key": "a", "rollout_percentage": 100}]},
                "payloads": {"a": {"x": 1}}
            }
        }))
        .unwrap();
        let definition = posthog_definition(&flag, 4);
        assert_eq!(definition["key"], "exp");
        assert_eq!(definition["active"], true);
        assert_eq!(definition["version"], 4);
        let property = &definition["filters"]["groups"][0]["properties"][0];
        assert_eq!(property["type"], "person");
        assert_eq!(property["operator"], "exact");
        assert_eq!(
            definition["filters"]["groups"][0]["rollout_percentage"],
            50.0
        );
        assert_eq!(
            definition["filters"]["multivariate"]["variants"][0]["key"],
            "a"
        );
        assert_eq!(definition["filters"]["payloads"]["a"], r#"{"x":1}"#);
    }
}
