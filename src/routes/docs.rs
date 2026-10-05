//! API documentation: an OpenAPI 3.1 spec at `/openapi.json` and the Scalar
//! reference UI at `/docs`.
//!
//! The Scalar bundle is vendored (`web/vendor/scalar.standalone.js`) and
//! embedded, so the docs page works offline with no CDN — consistent with the
//! single-binary promise. The spec is built in Rust so it stays close to the
//! routes it documents.

use axum::{
    Router,
    http::header,
    response::{Html, IntoResponse, Json, Response},
    routing::get,
};
use serde_json::{Map, Value, json};

const SCALAR_JS: &[u8] = include_bytes!("../../web/vendor/scalar.standalone.js");

pub fn router() -> Router {
    Router::new()
        .route("/openapi.json", get(|| async { Json(spec()) }))
        .route(
            "/docs",
            get(|| async {
                (
                    [(header::CONTENT_SECURITY_POLICY, crate::security::DOCS_CSP)],
                    docs_page().await,
                )
            }),
        )
        .route("/docs/scalar.js", get(scalar_js))
}

async fn scalar_js() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript")], SCALAR_JS).into_response()
}

async fn docs_page() -> Html<&'static str> {
    // Hoglet theme: the dashboard's palette (web/src/styles.css) mapped onto
    // Scalar's CSS variables. theme:'none' so ours is the only skin.
    Html(
        r##"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Hoglet API</title>
  <link rel="icon" href="data:image/svg+xml,<svg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 100 100%22><text y=%22.9em%22 font-size=%2290%22>🦔</text></svg>" />
  <style>
    :root, .dark-mode, .light-mode {
      /* surfaces */
      --scalar-background-1: #0b0d10;
      --scalar-background-2: #14181d;
      --scalar-background-3: #1a1f26;
      --scalar-background-accent: #ffb45414;
      /* text */
      --scalar-color-1: #e6edf3;
      --scalar-color-2: #8b98a5;
      --scalar-color-3: #6b7681;
      --scalar-color-accent: #ffb454;
      /* lines & controls */
      --scalar-border-color: #232a31;
      --scalar-button-1: #ffb454;
      --scalar-button-1-color: #0b0d10;
      --scalar-button-1-hover: #ffc678;
      /* semantic */
      --scalar-color-green: #3fb950;
      --scalar-color-red: #f47067;
      --scalar-color-yellow: #ffb454;
      --scalar-color-blue: #58a6ff;
      --scalar-color-orange: #ffb454;
      --scalar-color-purple: #bc8cff;
      /* sidebar */
      --scalar-sidebar-background-1: #0b0d10;
      --scalar-sidebar-color-1: #e6edf3;
      --scalar-sidebar-color-2: #8b98a5;
      --scalar-sidebar-color-active: #ffb454;
      --scalar-sidebar-item-active-background: #ffb45414;
      --scalar-sidebar-item-hover-background: #14181d;
      --scalar-sidebar-border-color: #232a31;
      --scalar-sidebar-search-background: #14181d;
      --scalar-sidebar-search-color: #8b98a5;
      --scalar-sidebar-search-border-color: #232a31;
      /* type — the dashboard is mono; docs get mono headings, readable body */
      --scalar-font: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      --scalar-font-code: ui-monospace, SFMono-Regular, Menlo, monospace;
    }
    /* mono, uppercase section headings — the dashboard's panel-title look */
    .section-header, .sidebar-heading, h2.t-editor__heading {
      font-family: var(--scalar-font-code) !important;
      letter-spacing: .03em;
    }
    /* amber method badges pop on the dark ground */
    .sidebar-heading-type, .http-verb { font-family: var(--scalar-font-code) !important; }
  </style>
</head>
<body>
  <div id="app"></div>
  <script src="/docs/scalar.js"></script>
  <script>
    Scalar.createApiReference('#app', {
      url: '/openapi.json',
      theme: 'none',
      darkMode: true,
      hideDarkModeToggle: true,
      // Self-hosted means no third-party requests: no web fonts, telemetry,
      // or agent/MCP registry lookups from the reference viewer.
      withDefaultFonts: false,
      telemetry: false,
      agent: { disabled: true },
      mcp: { disabled: true },
      metaData: { title: 'Hoglet API — one binary, PostHog-compatible' },
    })
  </script>
</body>
</html>"##,
    )
}
// ---------------------------------------------------------------------------
// OpenAPI document
// ---------------------------------------------------------------------------

const TAG_WIRE: &str = "SDK wire";
const TAG_WORKSPACE: &str = "Auth & workspace";
const TAG_TEAM: &str = "Team";
const TAG_QUERY: &str = "Insights & query";
const TAG_PERSONS: &str = "Persons & events";
const TAG_WEB: &str = "Web analytics";
const TAG_CATALOG: &str = "Catalog";
const TAG_FLAGS: &str = "Feature flags";
const TAG_RESOURCES: &str = "Dashboards, insights & shares";
const TAG_PROJECT: &str = "Project";
const TAG_OPS: &str = "Health & metrics";

/// Tag order is the order of the reference's sidebar.
const TAGS: &[(&str, &str)] = &[
    (
        TAG_WIRE,
        "PostHog wire protocol. Stock PostHog SDKs call these; shapes and status codes match PostHog. Authenticated by the project token (`phc_…`) in the URL or body, except local evaluation, which needs a personal API key.",
    ),
    (
        TAG_WORKSPACE,
        "First-run setup, login sessions, personal API keys, organizations and projects.",
    ),
    (
        TAG_TEAM,
        "Members, roles and invite links. No email server needed: creating an invite returns a one-time link to hand over.",
    ),
    (
        TAG_QUERY,
        "Insight queries (trends, funnels, retention, lifecycle, stickiness, paths, SQL) and the persons behind a result.",
    ),
    (TAG_PERSONS, "Persons and the stored event feed."),
    (
        TAG_WEB,
        "Visitors, pageviews, sessions, bounce rate and top pages/sources over `$pageview` events.",
    ),
    (
        TAG_CATALOG,
        "Event names, property keys and property values seen in a project, for autocomplete.",
    ),
    (
        TAG_FLAGS,
        "Feature flag definitions (PostHog's flag model) and per-person evaluation.",
    ),
    (
        TAG_RESOURCES,
        "Saved insights, dashboards, and public share links.",
    ),
    (
        TAG_PROJECT,
        "Data freshness, demo data, GDPR erasure, and shadow-mode forwarding to PostHog.",
    ),
    (
        TAG_OPS,
        "Liveness, readiness, Prometheus metrics, and this reference.",
    ),
];

const INFO_DESCRIPTION: &str = "\
PostHog-compatible product analytics in one binary.

**SDK wire** endpoints follow PostHog's protocol: point a PostHog SDK's `api_host` at this server. \
Everything else is Hoglet's own JSON API, used by the bundled dashboard.

**Authentication.** Project-scoped API routes accept either the `hoglet_sid` session cookie \
(set by `/api/auth/login` and `/api/auth/setup`) or a personal API key sent as \
`Authorization: Bearer phx_…`. Reads need project membership. Writes need a session or a \
`write`-scoped key, plus the `owner` or `admin` role in the project's organization. \
Account management (keys, organizations, projects, members, invites) needs a session. \
Roles are per organization: `owner` (everything), `admin` (everything but owners), `member` (read-only). \
State-changing requests that carry the session cookie are refused (403) when a browser marks them \
cross-site (`Sec-Fetch-Site`, or an `Origin` that is not this host); bearer-key requests are not affected.

**Errors.** API errors are JSON: `{\"error\": {\"code\", \"message\", \"request_id\"?, \"field\"?}}`. \
Most API responses carry an `x-request-id` header that matches `request_id`.

**Paths.** SDK wire paths are served with and without a trailing slash. \
The dashboard UI (`/`, `/dashboard`, `/assets/…`, and client-side routes) is not listed here.";

fn schema(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

fn param(name: &str) -> Value {
    json!({ "$ref": format!("#/components/parameters/{name}") })
}

fn array_of(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

fn nullable(type_name: &str) -> Value {
    json!({ "type": [type_name, "null"] })
}

fn nullable_ref(name: &str) -> Value {
    json!({ "anyOf": [schema(name), { "type": "null" }] })
}

fn json_content(body_schema: Value) -> Value {
    json!({ "application/json": { "schema": body_schema } })
}

fn json_request(body_schema: Value) -> Value {
    json!({ "required": true, "content": json_content(body_schema) })
}

fn ok(description: &str, body_schema: Value) -> Value {
    json!({ "description": description, "content": json_content(body_schema) })
}

fn query_param(name: &str, description: &str, param_schema: Value, required: bool) -> Value {
    json!({
        "name": name,
        "in": "query",
        "required": required,
        "description": description,
        "schema": param_schema,
    })
}

fn path_param(name: &str, description: &str, param_schema: Value) -> Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "description": description,
        "schema": param_schema,
    })
}

/// Session cookie or personal API key.
fn auth_any() -> Value {
    json!([{ "sessionCookie": [] }, { "personalApiKey": [] }])
}

/// Session cookie only: personal API keys get 403.
fn auth_session() -> Value {
    json!([{ "sessionCookie": [] }])
}

/// No Hoglet credential (wire endpoints carry the project token instead).
fn auth_none() -> Value {
    json!([])
}

/// Responses: the success entries plus shared error responses by status.
fn responses(success: Value, errors: &[&str]) -> Value {
    let mut map = match success {
        Value::Object(map) => map,
        _ => unreachable!("success responses are an object"),
    };
    for status in errors {
        let name = match *status {
            "400" => "BadRequest",
            "401" => "Unauthorized",
            "403" => "Forbidden",
            "404" => "NotFound",
            "409" => "Conflict",
            "422" => "UnprocessableEntity",
            "429" => "TooManyRequests",
            "500" => "InternalError",
            "503" => "Unavailable",
            "504" => "Timeout",
            other => unreachable!("no shared response for {other}"),
        };
        map.insert(
            (*status).to_owned(),
            json!({ "$ref": format!("#/components/responses/{name}") }),
        );
    }
    Value::Object(map)
}

/// Errors every project-scoped explore read can return.
const EXPLORE_ERRORS: &[&str] = &["400", "401", "403", "404", "500", "503", "504"];
/// Errors every project-scoped resource read can return.
const READ_ERRORS: &[&str] = &["401", "403", "404", "500", "503"];
/// Errors every project-scoped resource write can return.
const WRITE_ERRORS: &[&str] = &["400", "401", "403", "404", "409", "422", "500", "503"];

#[derive(Default)]
struct Paths(Map<String, Value>);

impl Paths {
    fn add(&mut self, path: &str, method: &str, operation: Value) {
        let item = self
            .0
            .entry(path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let previous = item
            .as_object_mut()
            .expect("path items are objects")
            .insert(method.to_owned(), operation);
        assert!(previous.is_none(), "{method} {path} documented twice");
    }
}

/// The OpenAPI 3.1 document describing every route the production
/// application mounts (see `crate::application`).
pub fn spec() -> Value {
    let mut paths = Paths::default();
    wire_paths(&mut paths);
    workspace_paths(&mut paths);
    team_paths(&mut paths);
    query_paths(&mut paths);
    persons_paths(&mut paths);
    web_paths(&mut paths);
    catalog_paths(&mut paths);
    flag_paths(&mut paths);
    resource_paths(&mut paths);
    project_paths(&mut paths);
    ops_paths(&mut paths);

    let mut schemas = Map::new();
    for group in [
        wire_schemas(),
        common_schemas(),
        query_schemas(),
        result_schemas(),
        explore_schemas(),
        flag_schemas(),
        workspace_schemas(),
        team_schemas(),
        resource_schemas(),
        project_schemas(),
    ] {
        for (name, value) in group {
            let previous = schemas.insert(name.clone(), value);
            assert!(previous.is_none(), "schema {name} defined twice");
        }
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Hoglet API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": INFO_DESCRIPTION,
            "license": { "name": "AGPL-3.0-only", "identifier": "AGPL-3.0-only" },
        },
        "servers": [{ "url": "/", "description": "This Hoglet instance" }],
        "tags": TAGS
            .iter()
            .map(|(name, description)| json!({ "name": name, "description": description }))
            .collect::<Vec<_>>(),
        "paths": Value::Object(paths.0),
        "components": {
            "securitySchemes": security_schemes(),
            "parameters": shared_parameters(),
            "responses": shared_responses(),
            "schemas": Value::Object(schemas),
        },
    })
}

fn security_schemes() -> Value {
    json!({
        "sessionCookie": {
            "type": "apiKey",
            "in": "cookie",
            "name": "hoglet_sid",
            "description": "Dashboard session. Set by `/api/auth/setup` and `/api/auth/login` (HttpOnly, SameSite=Lax, 7 days); cleared by `/api/auth/logout`. Sessions may write.",
        },
        "personalApiKey": {
            "type": "http",
            "scheme": "bearer",
            "bearerFormat": "phx_…",
            "description": "Personal API key: `Authorization: Bearer phx_…`, created at `POST /api/auth/keys`. Scope `read` reads; scope `write` may also mutate, subject to the user's project role. Never accepted where a project token belongs.",
        },
    })
}

fn shared_parameters() -> Value {
    json!({
        "ProjectId": path_param("project_id", "Project id (UUID). A malformed id is 404; a project the caller cannot access is 403 (404 on the Project routes).", json!({ "type": "string", "format": "uuid" })),
        "PersonId": path_param("person_id", "Person id.", json!({ "type": "string", "maxLength": 1024 })),
        "InsightId": path_param("insight_id", "Saved insight id.", json!({ "type": "string" })),
        "DashboardId": path_param("dashboard_id", "Dashboard id.", json!({ "type": "string" })),
        "FlagId": path_param("id", "Feature flag id.", json!({ "type": "integer", "format": "int64" })),
        "WebDateFrom": query_param("date_from", "Range start: `-24h`, `-7d`, `-4w`, `-3m`, `-1y`, `dStart`, `mStart`, `yStart`, `all`, or an ISO 8601 date/datetime.", json!({ "type": "string", "default": "-7d" }), false),
        "WebDateTo": query_param("date_to", "Range end, same forms as `date_from`. Omitted means now.", json!({ "type": "string" }), false),
        "WebInterval": query_param("interval", "Bucket size of the time series. Chosen from the range when omitted.", schema("Interval"), false),
        "PropertiesFilter": query_param("properties", "URL-encoded JSON array of `PropertyFilter` (at most 20).", json!({ "type": "string" }), false),
    })
}

fn shared_responses() -> Value {
    let error = |description: &str| ok(description, schema("ApiError"));
    let mut unavailable = error(
        "Temporarily unavailable; safe to retry. When a bounded query queue is full the code is `query_busy` and `Retry-After` is set.",
    );
    unavailable["headers"] = json!({
        "Retry-After": {
            "description": "Seconds to wait before retrying.",
            "schema": { "type": "integer" },
        }
    });
    json!({
        "BadRequest": error("The request is invalid: body, query string or field validation. `field` names the offending field when known."),
        "Unauthorized": error("No valid session cookie or personal API key."),
        "Forbidden": error("Authenticated, but not allowed: not a member of the project, missing `write` scope or owner/admin role, or a personal API key on a session-only route."),
        "NotFound": error("The resource does not exist or the id is malformed."),
        "Conflict": error("The request conflicts with existing state."),
        "TooManyRequests": error("Too many failed attempts from this address or for this account; `Retry-After` says how long to wait."),
        "UnprocessableEntity": error("Well-formed but semantically invalid. `field` points at the offending part."),
        "InternalError": error("Unexpected server error."),
        "Unavailable": unavailable,
        "Timeout": error("The query exceeded its execution deadline (`query_timeout`)."),
    })
}

fn merge(target: &mut Value, extra: Value) {
    let (Some(target), Value::Object(extra)) = (target.as_object_mut(), extra) else {
        unreachable!("merge operands are objects");
    };
    target.extend(extra);
}

// ── SDK wire ──────────────────────────────────────────────────────

fn capture_operation(operation_id: &str, summary: &str, intro: &str, limit: &str) -> Value {
    let description = format!(
        "{intro} Also served with a trailing slash, which is what the SDKs use.\n\n\
         The body is decoded in this order, whatever the `compression` hint says: gzip (by magic \
         bytes), form field `data=`, base64, plain JSON. `compression=lz64` is honoured for legacy \
         posthog-js. Body limit: {limit}; decompressed limit: 64 MiB.\n\n\
         Every distinct token in the request must belong to a project, or the whole request is \
         rejected with 401 before anything is stored. `$performance_event` events are dropped. \
         An event without a `uuid` gets a UUIDv7 derived from its timestamp.\n\n\
         Status codes follow the PostHog SDK retry contract: 4xx is never retried, 5xx is. `200` \
         means the events are durably written to the write-ahead log."
    );
    json!({
        "tags": [TAG_WIRE],
        "operationId": operation_id,
        "summary": summary,
        "description": description,
        "security": auth_none(),
        "parameters": [
            query_param("beacon", "`1` when sent with `navigator.sendBeacon`; success is then `204` with no body.", json!({ "type": "string", "enum": ["1"] }), false),
            query_param("compression", "Client hint. Ignored except `lz64`; gzip is detected from the bytes.", json!({ "type": "string", "examples": ["gzip-js", "base64", "lz64"] }), false),
            query_param("_", "Client send time in ms since the epoch. Corrects client clock skew when the body has no `sent_at`.", json!({ "type": "string", "pattern": "^[0-9]+$" }), false),
        ],
        "requestBody": {
            "required": true,
            "content": {
                "application/json": {
                    "schema": schema("CaptureBody"),
                    "examples": {
                        "events": {
                            "summary": "Array of events (posthog-js)",
                            "value": [{
                                "event": "$pageview",
                                "distinct_id": "u1",
                                "properties": { "token": "phc_example", "$current_url": "https://example.com/" }
                            }]
                        },
                        "batch": {
                            "summary": "Batch (server SDKs)",
                            "value": {
                                "api_key": "phc_example",
                                "batch": [
                                    { "event": "signed_up", "distinct_id": "u1", "timestamp": "2026-10-04T12:00:00Z" },
                                    { "event": "$identify", "distinct_id": "user@example.com", "properties": { "$anon_distinct_id": "u1", "$set": { "plan": "pro" } } }
                                ]
                            }
                        }
                    }
                },
                "text/plain": {
                    "schema": { "type": "string", "description": "Gzip bytes, base64 of the JSON body, or JSON." }
                },
                "application/x-www-form-urlencoded": {
                    "schema": {
                        "type": "object",
                        "required": ["data"],
                        "properties": { "data": { "type": "string", "description": "Base64 of the JSON body." } }
                    }
                }
            }
        },
        "responses": {
            "200": ok("Accepted and durably written.", schema("CaptureOk")),
            "204": { "description": "Accepted (`beacon=1`). No body." },
            "400": { "description": "Undecodable or malformed body: invalid JSON, missing `event` or `distinct_id`, bad `timestamp` or `uuid`. Empty body. Not retried." },
            "401": { "description": "Missing, malformed or unknown project token, or a personal API key (`phx_…`) used as a token. Empty body. Not retried." },
            "413": { "description": "Body or decompressed body over the limit. Not retried." },
            "429": { "description": "Per-project rate limit exceeded (`HOGLET_MAX_EVENTS_PER_SEC`, default 10000 events/s). Empty body." },
            "503": { "description": "The write-ahead log could not accept the events. Empty body. Retried by SDKs." }
        }
    })
}

fn wire_paths(paths: &mut Paths) {
    for (path, operation_id, summary, intro, limit) in [
        (
            "/e",
            "captureE",
            "Capture events (/e)",
            "Event ingestion used by posthog-js. Body: an array of events, one event, or a batch.",
            "2 MiB",
        ),
        (
            "/i/v0/e",
            "captureIV0E",
            "Capture events (/i/v0/e)",
            "The ingestion endpoint advertised in remote config (`analytics.endpoint`). Same handler as `/e`.",
            "2 MiB",
        ),
        (
            "/capture",
            "captureCapture",
            "Capture events (/capture)",
            "Alias of `/e`.",
            "2 MiB",
        ),
        (
            "/track",
            "captureTrack",
            "Capture events (/track)",
            "Alias of `/e`.",
            "2 MiB",
        ),
        (
            "/engage",
            "captureEngage",
            "Capture person updates (/engage)",
            "Alias of `/e`. An object without `event` is stored as `$identify`; top-level `$set`/`$set_once` move into `properties`.",
            "2 MiB",
        ),
        (
            "/batch",
            "captureBatch",
            "Capture a batch (/batch)",
            "Batch ingestion used by server SDKs. A batch-level `api_key`/`token` applies to every event and wins over per-event tokens; `sent_at` corrects client clock skew. `historical_migration` is ignored.",
            "20 MiB",
        ),
    ] {
        paths.add(
            path,
            "post",
            capture_operation(operation_id, summary, intro, limit),
        );
    }

    let token = path_param(
        "token",
        "Project token (`phc_…`).",
        json!({ "type": "string", "maxLength": 64 }),
    );
    paths.add(
        "/array/{token}/config",
        "get",
        json!({
            "tags": [TAG_WIRE],
            "operationId": "getRemoteConfig",
            "summary": "SDK remote config",
            "description": "The first request posthog-js makes; also served with a trailing slash. Features Hoglet does not implement are declared off (`sessionRecording`, `surveys`, `heatmaps`, `capturePerformance`, `autocaptureExceptions`) so the SDK never calls endpoints that do not exist.",
            "security": auth_none(),
            "parameters": [token.clone()],
            "responses": {
                "200": ok("Remote config.", schema("RemoteConfig")),
                "401": { "description": "Malformed or unknown project token. Empty body. Not retried." }
            }
        }),
    );
    paths.add(
        "/array/{token}/config.js",
        "get",
        json!({
            "tags": [TAG_WIRE],
            "operationId": "getRemoteConfigScript",
            "summary": "SDK remote config (script)",
            "description": "The script form posthog-js tries first. Sets `window._POSTHOG_REMOTE_CONFIG[token] = {config, siteApps: []}` with the same config as the JSON endpoint, which the SDK falls back to on failure.",
            "security": auth_none(),
            "parameters": [token],
            "responses": {
                "200": {
                    "description": "JavaScript.",
                    "content": { "application/javascript": { "schema": { "type": "string" } } }
                },
                "401": { "description": "Malformed or unknown project token. Empty body." }
            }
        }),
    );
    for (path, operation_id, versioned) in [
        ("/static/surveys.js", "getSurveysScript", false),
        (
            "/static/{version}/surveys.js",
            "getVersionedSurveysScript",
            true,
        ),
    ] {
        let mut operation = json!({
            "tags": [TAG_WIRE],
            "operationId": operation_id,
            "summary": if versioned { "Surveys extension (versioned path)" } else { "Surveys extension" },
            "description": "posthog-js loads its surveys extension even when remote config says `surveys: false`. This no-op script satisfies the loader and never shows a survey. Any other file name under `/static/` is 404.",
            "security": auth_none(),
            "responses": {
                "200": {
                    "description": "JavaScript, cacheable for one hour.",
                    "content": { "application/javascript": { "schema": { "type": "string" } } }
                }
            }
        });
        if versioned {
            operation["parameters"] = json!([path_param(
                "version",
                "SDK version segment. Ignored.",
                json!({ "type": "string" })
            )]);
        }
        paths.add(path, "get", operation);
    }

    flags_wire_paths(paths);
}

fn flags_wire_paths(paths: &mut Paths) {
    let flags_errors = || {
        json!({
            "400": ok("Undecodable body, not a JSON object, nested deeper than 64 levels, or missing `distinct_id` (`code`: `invalid_payload` or `missing_distinct_id`). Not retried.", schema("WireError")),
            "401": ok("Missing, malformed or unknown project token (`invalid_api_key`). Not retried.", schema("WireError")),
            "413": { "description": "Body over 1 MiB." },
            "503": ok("Flag definitions temporarily unavailable. Retried by SDKs.", schema("WireError"))
        })
    };
    let flags_body = json!({
        "required": true,
        "content": {
            "application/json": {
                "schema": schema("FlagsRequest"),
                "example": { "token": "phc_example", "distinct_id": "u1", "person_properties": { "plan": "pro" } }
            },
            "text/plain": {
                "schema": { "type": "string", "description": "Gzip bytes, base64 of the JSON body, or JSON/JSON5." }
            },
            "application/x-www-form-urlencoded": {
                "schema": {
                    "type": "object",
                    "properties": { "data": { "type": "string", "description": "Base64 of the JSON body." } }
                }
            }
        }
    });
    let compression = query_param(
        "compression",
        "Client hint. Gzip is detected from the bytes.",
        json!({ "type": "string" }),
        false,
    );

    let mut flags_responses = json!({
        "200": ok(
            "Evaluated flags: `FlagsResponseV2` for `v=2` or higher, otherwise `FlagsResponseV1`.",
            json!({ "oneOf": [schema("FlagsResponseV2"), schema("FlagsResponseV1")] })
        )
    });
    merge(&mut flags_responses, flags_errors());
    paths.add(
        "/flags",
        "post",
        json!({
            "tags": [TAG_WIRE],
            "operationId": "evaluateFlags",
            "summary": "Evaluate feature flags",
            "description": "Evaluates every active flag, or only `flag_keys`/`flag_keys_to_evaluate`, for one `distinct_id`. Also served at `/flags/`.\n\n\
                Person properties come from the stored person, overridden by `person_properties` in the body. Rollouts bucket deterministically by `distinct_id`. \
                Payloads are sent as JSON strings, as PostHog does. Every shape also carries the remote-config fields.\n\n\
                `errorsWhileComputingFlags` is `true` when stored person properties could not be read.",
            "security": auth_none(),
            "parameters": [
                query_param("v", "Response shape: `2` or higher gives the detailed `flags` map; absent or `1` gives `featureFlags` + `featureFlagPayloads`.", json!({ "type": "string", "examples": ["2"] }), false),
                compression.clone(),
            ],
            "requestBody": flags_body.clone(),
            "responses": flags_responses,
        }),
    );

    let mut decide_responses = json!({
        "200": ok(
            "Evaluated flags in the shape selected by `v`.",
            json!({ "oneOf": [
                schema("FlagsResponseV2"),
                schema("FlagsResponseV1"),
                schema("DecideResponseV2"),
                schema("DecideResponseV1")
            ] })
        )
    });
    merge(&mut decide_responses, flags_errors());
    paths.add(
        "/decide",
        "post",
        json!({
            "tags": [TAG_WIRE],
            "operationId": "decide",
            "summary": "Evaluate feature flags (legacy /decide)",
            "description": "The older flags endpoint; same evaluation as `/flags`. Also served at `/decide/`. Shape by `v`: absent or `1`: `DecideResponseV1` (enabled keys); `2`: `DecideResponseV2` (key to value); `3`: `FlagsResponseV1`; `4` or higher: `FlagsResponseV2`.",
            "security": auth_none(),
            "parameters": [
                query_param("v", "Response shape version.", json!({ "type": "string", "examples": ["3", "4"] }), false),
                compression,
            ],
            "requestBody": flags_body,
            "responses": decide_responses,
        }),
    );

    for (path, operation_id) in [
        ("/flags/definitions", "getFlagDefinitions"),
        ("/api/feature_flag/local_evaluation", "getLocalEvaluation"),
    ] {
        paths.add(
            path,
            "get",
            json!({
                "tags": [TAG_WIRE],
                "operationId": operation_id,
                "summary": format!("Flag definitions for local evaluation ({path})"),
                "description": "PostHog-format flag definitions for server SDKs that evaluate flags locally (the SDK's `personalApiKey` option). \
                    Requires a personal API key whose user can access the token's project. \
                    The response has a strong `ETag`; a matching `If-None-Match` returns `304`. Errors are `{\"detail\": …}`. \
                    Served at `/flags/definitions` and `/api/feature_flag/local_evaluation`, with and without a trailing slash.",
                "security": [{ "personalApiKey": [] }],
                "parameters": [
                    query_param("token", "Project token (`phc_…`).", json!({ "type": "string" }), true),
                    {
                        "name": "If-None-Match",
                        "in": "header",
                        "required": false,
                        "description": "`ETag` of a previous response.",
                        "schema": { "type": "string" }
                    }
                ],
                "responses": {
                    "200": {
                        "description": "Flag definitions.",
                        "headers": {
                            "ETag": { "description": "Quoted SHA-1 of the body.", "schema": { "type": "string" } }
                        },
                        "content": json_content(schema("LocalEvaluationResponse"))
                    },
                    "304": { "description": "Unchanged since `If-None-Match`." },
                    "401": ok("Missing or invalid personal API key, or missing or unknown project token.", schema("DetailError")),
                    "403": ok("The key's user cannot access the project.", schema("DetailError")),
                    "503": ok("Temporarily unavailable.", schema("DetailError"))
                }
            }),
        );
    }
}

// ── Auth & workspace ──────────────────────────────────────────────

fn session_cookie_header() -> Value {
    json!({
        "Set-Cookie": {
            "description": "`hoglet_sid` session cookie (HttpOnly, SameSite=Lax, Path=/, Max-Age 7 days).",
            "schema": { "type": "string" }
        }
    })
}

fn workspace_paths(paths: &mut Paths) {
    paths.add(
        "/api/auth/bootstrap",
        "get",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "getBootstrap",
            "summary": "Is first-run setup required?",
            "description": "`setup_required` is `true` until the first user exists.",
            "security": auth_none(),
            "responses": responses(
                json!({ "200": ok("Setup state.", schema("BootstrapResponse")) }),
                &["500", "503"]
            )
        }),
    );

    let mut setup_ok = ok(
        "Workspace of the new owner; the session cookie is set.",
        schema("Workspace"),
    );
    setup_ok["headers"] = session_cookie_header();
    paths.add(
        "/api/auth/setup",
        "post",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "setup",
            "summary": "First-run setup",
            "description": "Creates the first user as owner of a new organization and project, and logs them in. Only possible while no user exists. \
                With `existing_project_token`, the user instead becomes owner of the organization that already holds a project with that capture token \
                (for data captured before setup); if the token is unknown and projects already exist, the response is 404.",
            "security": auth_none(),
            "requestBody": json_request(schema("SetupRequest")),
            "responses": responses(
                json!({
                    "200": setup_ok,
                    "403": ok("`HOGLET_SETUP_TOKEN` is set and the `X-Hoglet-Setup-Token` header (or `setup_token` field) is missing or wrong.", schema("ApiError")),
                    "409": ok("Setup was already completed.", schema("ApiError"))
                }),
                &["400", "404", "500", "503"]
            )
        }),
    );

    let mut login_ok = ok(
        "Workspace of the user; the session cookie is set.",
        schema("Workspace"),
    );
    login_ok["headers"] = session_cookie_header();
    paths.add(
        "/api/auth/login",
        "post",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "login",
            "summary": "Log in",
            "security": auth_none(),
            "requestBody": json_request(schema("LoginRequest")),
            "responses": responses(
                json!({
                    "200": login_ok,
                    "429": ok("Too many failed attempts for this email or address; wait `Retry-After` seconds.", schema("ApiError"))
                }),
                &["400", "401", "500", "503"]
            )
        }),
    );

    let mut logout_ok = ok(
        "Logged out; the session cookie is cleared.",
        schema("StatusOk"),
    );
    logout_ok["headers"] = session_cookie_header();
    paths.add(
        "/api/auth/logout",
        "post",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "logout",
            "summary": "Log out",
            "description": "Ends the session named by the cookie, if any.",
            "security": auth_any(),
            "responses": responses(json!({ "200": logout_ok }), &["401", "500", "503"])
        }),
    );
    paths.add(
        "/api/auth/me",
        "get",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "getMe",
            "summary": "Current user and workspace",
            "security": auth_any(),
            "responses": responses(
                json!({ "200": ok("The user, their organizations, roles and projects.", schema("Workspace")) }),
                &["401", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/auth/keys",
        "get",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "listPersonalApiKeys",
            "summary": "List personal API keys",
            "description": "The current user's keys, newest first. Secrets are never returned again. Session only.",
            "security": auth_session(),
            "responses": responses(
                json!({ "200": ok("Keys.", array_of(schema("PersonalApiKey"))) }),
                &["401", "403", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/auth/keys",
        "post",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "createPersonalApiKey",
            "summary": "Create a personal API key",
            "description": "Returns the secret (`phx_…`) once. Session only.",
            "security": auth_session(),
            "requestBody": json_request(schema("CreateKeyRequest")),
            "responses": responses(
                json!({ "201": ok("Created. Store `secret` now; it is not shown again.", schema("CreatedPersonalApiKey")) }),
                &["400", "401", "403", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/auth/keys/{key_id}",
        "delete",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "revokePersonalApiKey",
            "summary": "Revoke a personal API key",
            "description": "Session only. Only the key's owner can revoke it.",
            "security": auth_session(),
            "parameters": [path_param("key_id", "Key id.", json!({ "type": "string" }))],
            "responses": responses(
                json!({ "204": { "description": "Revoked." } }),
                &["401", "403", "404", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations",
        "get",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "listOrganizations",
            "summary": "List organizations",
            "description": "Organizations the user belongs to, with their role and projects.",
            "security": auth_any(),
            "responses": responses(
                json!({ "200": ok("Organizations.", array_of(schema("Organization"))) }),
                &["401", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations",
        "post",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "createOrganization",
            "summary": "Create an organization",
            "description": "The caller becomes its owner. Session only.",
            "security": auth_session(),
            "requestBody": json_request(schema("NameRequest")),
            "responses": responses(
                json!({ "201": ok("Created.", schema("Organization")) }),
                &["400", "401", "403", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations/{organization_id}/projects",
        "post",
        json!({
            "tags": [TAG_WORKSPACE],
            "operationId": "createProject",
            "summary": "Create a project",
            "description": "Creates a project with a new capture token (`phc_…`). Session only; requires the `owner` or `admin` role in the organization.",
            "security": auth_session(),
            "parameters": [path_param("organization_id", "Organization id.", json!({ "type": "string" }))],
            "requestBody": json_request(schema("NameRequest")),
            "responses": responses(
                json!({ "201": ok("Created.", schema("Project")) }),
                &["400", "401", "403", "500", "503"]
            )
        }),
    );
}

// ── Team ──────────────────────────────────────────────────────────

fn team_paths(paths: &mut Paths) {
    let org = || path_param("organization_id", "Organization id.", json!({ "type": "string", "format": "uuid" }));
    let user = || path_param("user_id", "Member's user id.", json!({ "type": "string", "format": "uuid" }));
    paths.add(
        "/api/organizations/{organization_id}/members",
        "get",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "listMembers",
            "summary": "List members",
            "description": "Everyone in the organization with their role. Any member may read it.",
            "security": auth_any(),
            "parameters": [org()],
            "responses": responses(
                json!({ "200": ok("Members, owners first.", array_of(schema("Member"))) }),
                &["401", "403", "404", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations/{organization_id}/members/{user_id}",
        "patch",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "updateMember",
            "summary": "Change a member's role",
            "description": "Session only; `owner` or `admin`. Admins cannot change owners or grant `owner`. The last owner cannot be demoted (`409 last_owner`).",
            "security": auth_session(),
            "parameters": [org(), user()],
            "requestBody": json_request(schema("UpdateMemberRequest")),
            "responses": responses(
                json!({ "200": ok("The member with the new role.", schema("Member")) }),
                &["400", "401", "403", "404", "409", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations/{organization_id}/members/{user_id}",
        "delete",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "removeMember",
            "summary": "Remove a member (or leave)",
            "description": "Session only. Owners and admins remove members (admins never owners); anyone may remove themselves. The last owner cannot be removed (`409 last_owner`). The person's sessions end, and when no organization is left their personal API keys are revoked.",
            "security": auth_session(),
            "parameters": [org(), user()],
            "responses": responses(
                json!({ "204": { "description": "Removed." } }),
                &["401", "403", "404", "409", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations/{organization_id}/invites",
        "get",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "listInvites",
            "summary": "List pending invites",
            "description": "Unused, unexpired invites. `owner` or `admin`. Tokens are never listed.",
            "security": auth_any(),
            "parameters": [org()],
            "responses": responses(
                json!({ "200": ok("Pending invites.", array_of(schema("Invite"))) }),
                &["401", "403", "404", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations/{organization_id}/invites",
        "post",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "createInvite",
            "summary": "Invite someone",
            "description": "Session only; `owner` or `admin` (admins cannot invite owners). Returns a one-time token and the dashboard path `/invite/{token}` to hand to the invitee. It is shown once, stored only as a hash, single use, and expires after 7 days. Inviting the same address again replaces the earlier link. At most 100 pending invites per organization.",
            "security": auth_session(),
            "parameters": [org()],
            "requestBody": json_request(schema("CreateInviteRequest")),
            "responses": responses(
                json!({ "201": ok("Created. Store `token` now; it is not shown again.", schema("CreatedInvite")) }),
                &["400", "401", "403", "404", "409", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/organizations/{organization_id}/invites/{invite_id}",
        "delete",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "revokeInvite",
            "summary": "Revoke an invite",
            "description": "Session only; `owner` or `admin` (admins cannot revoke owner invites). The link stops working at once.",
            "security": auth_session(),
            "parameters": [org(), path_param("invite_id", "Invite id.", json!({ "type": "string", "format": "uuid" }))],
            "responses": responses(
                json!({ "204": { "description": "Revoked." } }),
                &["401", "403", "404", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/invites/preview",
        "post",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "previewInvite",
            "summary": "Look at an invite",
            "description": "No session: the token is the credential. Unknown, expired, used and revoked tokens all answer `404 invite_invalid`. Failed guesses are throttled per source address (`429`).",
            "security": auth_none(),
            "requestBody": json_request(schema("InviteTokenRequest")),
            "responses": responses(
                json!({ "200": ok("What the invite is for.", schema("InvitePreview")) }),
                &["400", "404", "429", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/invites/accept",
        "post",
        json!({
            "tags": [TAG_TEAM],
            "operationId": "acceptInvite",
            "summary": "Accept an invite",
            "description": "No session. A new address gets an account with the given `name` and `password` (at least 12 characters); an address that already has an account must present its current password, like a sign-in, and shares the sign-in throttle. Single use. Sets the `hoglet_sid` session cookie.",
            "security": auth_none(),
            "requestBody": json_request(schema("AcceptInviteRequest")),
            "responses": responses(
                json!({ "200": {
                    "description": "Joined. The invitee's workspace; the response sets the session cookie.",
                    "headers": session_cookie_header(),
                    "content": { "application/json": { "schema": schema("Workspace") } }
                } }),
                &["400", "401", "404", "409", "429", "500", "503"]
            )
        }),
    );
}

// ── Insights & query ──────────────────────────────────────────────

fn query_paths(paths: &mut Paths) {
    paths.add(
        "/api/projects/{project_id}/query",
        "post",
        json!({
            "tags": [TAG_QUERY],
            "operationId": "runQuery",
            "summary": "Run an insight query",
            "description": "Runs one `InsightQuery` over the project's events. Person counts are counts of persons after identity merges. \
                Results are cached per project until its stored events change (`meta.data_version`); `refresh: true` bypasses the cache.\n\n\
                Authentication and project access are checked before the body is read. Queries run in a bounded queue \
                (503 `query_busy` with `Retry-After` when full) with a 10 s deadline (504).",
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": schema("QueryRequest"),
                        "example": {
                            "query": {
                                "kind": "TrendsQuery",
                                "series": [{ "event": "$pageview", "math": "dau" }],
                                "date_range": { "date_from": "-30d" },
                                "interval": "day"
                            }
                        }
                    }
                }
            },
            "responses": responses(
                json!({ "200": ok("Query result.", schema("QueryResponse")) }),
                &["400", "401", "403", "404", "422", "500", "503", "504"]
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/query/actors",
        "post",
        json!({
            "tags": [TAG_QUERY],
            "operationId": "queryActors",
            "summary": "Persons behind a result",
            "description": "The persons counted in one number of an insight result: a trends point, a funnel step (converted or dropped off), \
                a retention cell, a lifecycle cell or a stickiness bar. Pages with `offset`/`limit`.",
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "requestBody": json_request(schema("ActorsRequest")),
            "responses": responses(
                json!({ "200": ok("A page of persons.", schema("ActorsResponse")) }),
                &["400", "401", "403", "404", "422", "500", "503", "504"]
            )
        }),
    );
}

// ── Persons & events ──────────────────────────────────────────────

fn limit_param(description: &str, default: u32, maximum: u32) -> Value {
    query_param(
        "limit",
        description,
        json!({ "type": "integer", "minimum": 1, "maximum": maximum, "default": default }),
        false,
    )
}

fn before_param() -> Value {
    query_param(
        "before",
        "RFC 3339 cursor: only events strictly older. Pass back `next_before`.",
        json!({ "type": "string", "format": "date-time" }),
        false,
    )
}

fn persons_paths(paths: &mut Paths) {
    paths.add(
        "/api/projects/{project_id}/persons",
        "get",
        json!({
            "tags": [TAG_PERSONS],
            "operationId": "listPersons",
            "summary": "List persons",
            "description": "Newest persons first.",
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                query_param("search", "Distinct-id prefix, or a substring of the `email` or `name` property (case-insensitive).", json!({ "type": "string" }), false),
                query_param("cursor", "Opaque cursor from `next_cursor`.", json!({ "type": "string" }), false),
                limit_param("Page size; values above 100 are clamped.", 50, 100),
            ],
            "responses": responses(
                json!({ "200": ok("A page of persons.", schema("PersonListResponse")) }),
                EXPLORE_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/persons/{person_id}",
        "get",
        json!({
            "tags": [TAG_PERSONS],
            "operationId": "getPerson",
            "summary": "Get a person",
            "description": "The person with every distinct id, event and session counts, and first/last seen.",
            "security": auth_any(),
            "parameters": [param("ProjectId"), param("PersonId")],
            "responses": responses(
                json!({ "200": ok("The person.", schema("PersonDetail")) }),
                EXPLORE_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/persons/{person_id}/events",
        "get",
        json!({
            "tags": [TAG_PERSONS],
            "operationId": "listPersonEvents",
            "summary": "A person's events",
            "description": "Events of every distinct id of the person, newest first.",
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                param("PersonId"),
                before_param(),
                limit_param("Page size; values above 200 are clamped.", 100, 200),
            ],
            "responses": responses(
                json!({ "200": ok("A page of events.", schema("EventListResponse")) }),
                EXPLORE_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/events",
        "get",
        json!({
            "tags": [TAG_PERSONS],
            "operationId": "listEvents",
            "summary": "Event feed",
            "description": "Stored events, newest first. Filters combine with AND. An unknown `person_id` yields no events.",
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                query_param("event", "Exact event name (at most 400 bytes).", json!({ "type": "string", "maxLength": 400 }), false),
                query_param("person_id", "Only events of this person's distinct ids.", json!({ "type": "string", "maxLength": 1024 }), false),
                query_param("distinct_id", "Only events of this distinct id.", json!({ "type": "string", "maxLength": 1024 }), false),
                param("PropertiesFilter"),
                before_param(),
                limit_param("Page size; values above 200 are clamped.", 100, 200),
            ],
            "responses": responses(
                json!({ "200": ok("A page of events.", schema("EventListResponse")) }),
                EXPLORE_ERRORS
            )
        }),
    );
}

// ── Web analytics ─────────────────────────────────────────────────

fn web_paths(paths: &mut Paths) {
    let description = "Visitors are persons; sessions are `$session_id` values (events without one are sessionized by 30 minutes of inactivity). \
        A bounce is a session with exactly one pageview and no other event. `previous` compares with the preceding period of equal length.";
    paths.add(
        "/api/projects/{project_id}/web/overview",
        "get",
        json!({
            "tags": [TAG_WEB],
            "operationId": "getWebOverview",
            "summary": "Web overview",
            "description": format!("Headline metrics and visitor/pageview series. {description}"),
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                param("WebDateFrom"),
                param("WebDateTo"),
                param("WebInterval"),
                param("PropertiesFilter"),
            ],
            "responses": responses(
                json!({ "200": ok("Overview.", schema("WebOverview")) }),
                EXPLORE_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/web/breakdown",
        "get",
        json!({
            "tags": [TAG_WEB],
            "operationId": "getWebBreakdown",
            "summary": "Web breakdown",
            "description": format!("Top values of one dimension by visitors. {description}"),
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                query_param("dimension", "What to break down by.", schema("WebDimension"), true),
                limit_param("Rows; values above 100 are clamped.", 10, 100),
                param("WebDateFrom"),
                param("WebDateTo"),
                param("WebInterval"),
                param("PropertiesFilter"),
            ],
            "responses": responses(
                json!({ "200": ok("Breakdown.", schema("WebBreakdown")) }),
                EXPLORE_ERRORS
            )
        }),
    );
}

// ── Catalog ───────────────────────────────────────────────────────

fn catalog_paths(paths: &mut Paths) {
    let search = query_param(
        "search",
        "Case-insensitive substring (alias `prefix`).",
        json!({ "type": "string", "maxLength": 200 }),
        false,
    );
    let source = query_param(
        "type",
        "Event or person properties (alias `source`).",
        schema("PropertySource"),
        false,
    );
    let errors: &[&str] = &["400", "401", "403", "404", "500"];
    paths.add(
        "/api/projects/{project_id}/catalog/events",
        "get",
        json!({
            "tags": [TAG_CATALOG],
            "operationId": "listCatalogEvents",
            "summary": "Event names",
            "description": "Event names seen in the project, most frequent first.",
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                search.clone(),
                limit_param("Values above 200 are clamped.", 200, 200),
            ],
            "responses": responses(
                json!({ "200": ok("Event names.", array_of(schema("CatalogEvent"))) }),
                errors
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/catalog/properties",
        "get",
        json!({
            "tags": [TAG_CATALOG],
            "operationId": "listCatalogProperties",
            "summary": "Property keys",
            "description": "Property keys of events or persons, most frequent first (at most 500).",
            "security": auth_any(),
            "parameters": [param("ProjectId"), source.clone(), search.clone()],
            "responses": responses(
                json!({ "200": ok("Property keys.", array_of(schema("CatalogProperty"))) }),
                errors
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/catalog/values",
        "get",
        json!({
            "tags": [TAG_CATALOG],
            "operationId": "listCatalogValues",
            "summary": "Property values",
            "description": "Values of one property, most frequent first.",
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                query_param("key", "Property key.", json!({ "type": "string", "minLength": 1, "maxLength": 1000 }), true),
                source,
                search,
                limit_param("Values above 100 are clamped.", 50, 100),
            ],
            "responses": responses(
                json!({ "200": ok("Property values.", array_of(schema("CatalogValue"))) }),
                errors
            )
        }),
    );
}

// ── Feature flags ─────────────────────────────────────────────────

fn flag_paths(paths: &mut Paths) {
    let write_note = "Requires a session or a `write`-scoped key, and the `owner` or `admin` role.";
    let errors: &[&str] = &["400", "401", "403", "404", "409", "500", "503"];
    paths.add(
        "/api/projects/{project_id}/feature_flags",
        "get",
        json!({
            "tags": [TAG_FLAGS],
            "operationId": "listFeatureFlags",
            "summary": "List feature flags",
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "responses": responses(
                json!({ "200": ok("Flags.", array_of(schema("FeatureFlag"))) }),
                READ_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/feature_flags",
        "post",
        json!({
            "tags": [TAG_FLAGS],
            "operationId": "createFeatureFlag",
            "summary": "Create a feature flag",
            "description": format!("Keys are unique per project (409 otherwise); at most 2000 flags per project. {write_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "requestBody": json_request(schema("FeatureFlagInput")),
            "responses": responses(
                json!({ "201": ok("Created.", schema("FeatureFlag")) }),
                errors
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/feature_flags/{id}",
        "get",
        json!({
            "tags": [TAG_FLAGS],
            "operationId": "getFeatureFlag",
            "summary": "Get a feature flag",
            "security": auth_any(),
            "parameters": [param("ProjectId"), param("FlagId")],
            "responses": responses(
                json!({ "200": ok("The flag.", schema("FeatureFlag")) }),
                READ_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/feature_flags/{id}",
        "patch",
        json!({
            "tags": [TAG_FLAGS],
            "operationId": "updateFeatureFlag",
            "summary": "Update a feature flag",
            "description": format!("Changes only the fields present. {write_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId"), param("FlagId")],
            "requestBody": json_request(schema("FeatureFlagPatch")),
            "responses": responses(
                json!({ "200": ok("The updated flag.", schema("FeatureFlag")) }),
                errors
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/feature_flags/{id}",
        "delete",
        json!({
            "tags": [TAG_FLAGS],
            "operationId": "deleteFeatureFlag",
            "summary": "Delete a feature flag",
            "description": write_note,
            "security": auth_any(),
            "parameters": [param("ProjectId"), param("FlagId")],
            "responses": responses(
                json!({ "204": { "description": "Deleted." } }),
                &["401", "403", "404", "500", "503"]
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/feature_flags/{id}/evaluate",
        "get",
        json!({
            "tags": [TAG_FLAGS],
            "operationId": "evaluateFeatureFlag",
            "summary": "Evaluate a flag for one person",
            "description": "What `distinct_id` gets and why, using the stored person's properties. An inactive flag evaluates to `enabled: false` with reason `disabled`.",
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                param("FlagId"),
                query_param("distinct_id", "Distinct id to evaluate for (truncated to 200 characters).", json!({ "type": "string", "minLength": 1 }), true),
            ],
            "responses": responses(
                json!({ "200": ok("Evaluation.", schema("FlagEvaluation")) }),
                &["400", "401", "403", "404", "500", "503"]
            )
        }),
    );
}

// ── Dashboards, insights & shares ─────────────────────────────────

fn resource_paths(paths: &mut Paths) {
    let write_note = "Requires a session or a `write`-scoped key, and the `owner` or `admin` role.";
    let crud = [
        (
            "/api/projects/{project_id}/insights",
            "/api/projects/{project_id}/insights/{insight_id}",
            "InsightId",
            "Insight",
            "insight",
            "SavedInsight",
            "InsightDraft",
        ),
        (
            "/api/projects/{project_id}/dashboards",
            "/api/projects/{project_id}/dashboards/{dashboard_id}",
            "DashboardId",
            "Dashboard",
            "dashboard",
            "Dashboard",
            "DashboardDraft",
        ),
    ];
    for (collection, item, id_param, noun, lower, model, draft) in crud {
        let draft_note = if noun == "Insight" {
            " `query_ir` is validated against the supported query subset; an invalid query is 422 with `field` under `query_ir`."
        } else {
            " Tiles are set with `PUT …/tiles`."
        };
        paths.add(
            collection,
            "get",
            json!({
                "tags": [TAG_RESOURCES],
                "operationId": format!("list{noun}s"),
                "summary": format!("List {lower}s"),
                "security": auth_any(),
                "parameters": [param("ProjectId")],
                "responses": responses(
                    json!({ "200": ok(&format!("{noun}s."), array_of(schema(model))) }),
                    READ_ERRORS
                )
            }),
        );
        paths.add(
            collection,
            "post",
            json!({
                "tags": [TAG_RESOURCES],
                "operationId": format!("create{noun}"),
                "summary": format!("Create a {lower}"),
                "description": format!("{write_note}{draft_note}"),
                "security": auth_any(),
                "parameters": [param("ProjectId")],
                "requestBody": json_request(schema(draft)),
                "responses": responses(
                    json!({ "201": ok("Created.", schema(model)) }),
                    WRITE_ERRORS
                )
            }),
        );
        paths.add(
            item,
            "get",
            json!({
                "tags": [TAG_RESOURCES],
                "operationId": format!("get{noun}"),
                "summary": format!("Get a {lower}"),
                "security": auth_any(),
                "parameters": [param("ProjectId"), param(id_param)],
                "responses": responses(
                    json!({ "200": ok(&format!("The {lower}."), schema(model)) }),
                    READ_ERRORS
                )
            }),
        );
        paths.add(
            item,
            "put",
            json!({
                "tags": [TAG_RESOURCES],
                "operationId": format!("update{noun}"),
                "summary": format!("Replace a {lower}"),
                "description": format!("{write_note}{draft_note}"),
                "security": auth_any(),
                "parameters": [param("ProjectId"), param(id_param)],
                "requestBody": json_request(schema(draft)),
                "responses": responses(
                    json!({ "200": ok(&format!("The updated {lower}."), schema(model)) }),
                    WRITE_ERRORS
                )
            }),
        );
        paths.add(
            item,
            "delete",
            json!({
                "tags": [TAG_RESOURCES],
                "operationId": format!("delete{noun}"),
                "summary": format!("Delete a {lower}"),
                "description": write_note,
                "security": auth_any(),
                "parameters": [param("ProjectId"), param(id_param)],
                "responses": responses(
                    json!({ "204": { "description": "Deleted." } }),
                    &["401", "403", "404", "500", "503"]
                )
            }),
        );
    }
    paths.add(
        "/api/projects/{project_id}/dashboards/{dashboard_id}/tiles",
        "put",
        json!({
            "tags": [TAG_RESOURCES],
            "operationId": "replaceDashboardTiles",
            "summary": "Replace a dashboard's tiles",
            "description": format!("The body is the complete tile list. Every `insight_id` must be an insight of the same project. {write_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId"), param("DashboardId")],
            "requestBody": json_request(array_of(schema("DashboardTileInput"))),
            "responses": responses(
                json!({ "200": ok("The dashboard with its new tiles.", schema("Dashboard")) }),
                WRITE_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/shares",
        "get",
        json!({
            "tags": [TAG_RESOURCES],
            "operationId": "listShares",
            "summary": "List share links",
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "responses": responses(
                json!({ "200": ok("Share links.", array_of(schema("ShareLink"))) }),
                READ_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/shares",
        "post",
        json!({
            "tags": [TAG_RESOURCES],
            "operationId": "createShare",
            "summary": "Create a share link",
            "description": format!("Creates a public, read-only link (`token`, `phs_…`) to one insight or dashboard of the project. {write_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "requestBody": json_request(schema("ShareDraft")),
            "responses": responses(
                json!({ "201": ok("Created.", schema("ShareLink")) }),
                WRITE_ERRORS
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/shares/{share_id}",
        "delete",
        json!({
            "tags": [TAG_RESOURCES],
            "operationId": "deleteShare",
            "summary": "Revoke a share link",
            "description": write_note,
            "security": auth_any(),
            "parameters": [
                param("ProjectId"),
                path_param("share_id", "Share link id.", json!({ "type": "string" })),
            ],
            "responses": responses(
                json!({ "204": { "description": "Revoked." } }),
                &["401", "403", "404", "500", "503"]
            )
        }),
    );
    for (path, operation_id) in [
        ("/shared/{token}", "resolveShare"),
        ("/api/shares/{token}", "resolveShareApi"),
    ] {
        paths.add(
            path,
            "get",
            json!({
                "tags": [TAG_RESOURCES],
                "operationId": operation_id,
                "summary": format!("Open a share link ({})", path.trim_end_matches("/{token}")),
                "description": "No authentication: the share token is the credential. Returns the shared insight or dashboard. Unknown, revoked and expired tokens are 404. Served at `/shared/{token}` and `/api/shares/{token}`.",
                "security": auth_none(),
                "parameters": [path_param("token", "Share token (`phs_…`).", json!({ "type": "string" }))],
                "responses": responses(
                    json!({ "200": ok("The shared object.", schema("PublicShare")) }),
                    &["404", "500", "503"]
                )
            }),
        );
    }
}

// ── Project ───────────────────────────────────────────────────────

fn project_paths(paths: &mut Paths) {
    let access_note = "A project the caller cannot access is 404.";
    paths.add(
        "/api/projects/{project_id}/status",
        "get",
        json!({
            "tags": [TAG_PROJECT],
            "operationId": "getProjectStatus",
            "summary": "Data freshness",
            "description": format!("Whether events exist, the newest stored event, ingestion lag, and stored size. {access_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "responses": responses(
                json!({ "200": ok("Status.", schema("ProjectStatus")) }),
                &["401", "404", "503"]
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/demo",
        "post",
        json!({
            "tags": [TAG_PROJECT],
            "operationId": "seedDemoData",
            "summary": "Fill the project with demo data",
            "description": format!("Generates a demo dataset and writes it through the normal durable pipeline. Any project member. {access_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "responses": responses(
                json!({ "200": ok("Number of events written.", schema("DemoResult")) }),
                &["401", "404", "503"]
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/persons/{person_id}/erase",
        "post",
        json!({
            "tags": [TAG_PROJECT],
            "operationId": "erasePerson",
            "summary": "Erase a person (GDPR)",
            "description": format!("Physically deletes the person, their distinct ids, and every stored event of those ids before responding. Not reversible. Requires the `owner` or `admin` role. An unknown person is 404. {access_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId"), param("PersonId")],
            "responses": responses(
                json!({ "200": ok("What was removed.", schema("ErasureReport")) }),
                &["401", "403", "404", "503"]
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/forwarding",
        "get",
        json!({
            "tags": [TAG_PROJECT],
            "operationId": "getForwarding",
            "summary": "Shadow-mode forwarding status",
            "description": format!("Forwarding sends every acknowledged event on to a PostHog project as well, for side-by-side comparison. Counters are since process start. {access_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "responses": responses(
                json!({ "200": ok("Settings and counters.", schema("ForwardingStatus")) }),
                &["401", "404", "503"]
            )
        }),
    );
    paths.add(
        "/api/projects/{project_id}/forwarding",
        "put",
        json!({
            "tags": [TAG_PROJECT],
            "operationId": "setForwarding",
            "summary": "Configure shadow-mode forwarding",
            "description": format!("When `enabled`, `host` must be an http(s) URL (at most 256 bytes) and `posthog_token` a `phc_` key (at most 128 bytes). Requires a session or a `write`-scoped key, and the `owner` or `admin` role. {access_note}"),
            "security": auth_any(),
            "parameters": [param("ProjectId")],
            "requestBody": json_request(schema("ForwardingConfig")),
            "responses": responses(
                json!({ "200": ok("Settings and counters.", schema("ForwardingStatus")) }),
                &["400", "401", "403", "404", "503"]
            )
        }),
    );
}

// ── Health & metrics ──────────────────────────────────────────────

fn ops_paths(paths: &mut Paths) {
    paths.add(
        "/health",
        "get",
        json!({
            "tags": [TAG_OPS],
            "operationId": "health",
            "summary": "Liveness",
            "description": "`200` while the process runs. Says nothing about readiness.",
            "security": auth_none(),
            "responses": { "200": { "description": "Alive. Empty body." } }
        }),
    );
    paths.add(
        "/ready",
        "get",
        json!({
            "tags": [TAG_OPS],
            "operationId": "ready",
            "summary": "Readiness",
            "description": "`200` once write-ahead-log recovery has finished and every store is open; `503` before that and during shutdown. Point load balancers here.",
            "security": auth_none(),
            "responses": {
                "200": { "description": "Ready. Empty body." },
                "503": { "description": "Not ready. Empty body." }
            }
        }),
    );
    paths.add(
        "/metrics",
        "get",
        json!({
            "tags": [TAG_OPS],
            "operationId": "metrics",
            "summary": "Prometheus metrics",
            "description": "Process counters in Prometheus text format: `hoglet_events_captured_total`, `hoglet_events_acked_total`, `hoglet_requests_rejected_total`, `hoglet_sink_errors_total`, `hoglet_uptime_seconds`.",
            "security": auth_none(),
            "responses": {
                "200": {
                    "description": "Prometheus text exposition 0.0.4.",
                    "content": { "text/plain": { "schema": { "type": "string" } } }
                }
            }
        }),
    );
    paths.add(
        "/openapi.json",
        "get",
        json!({
            "tags": [TAG_OPS],
            "operationId": "getOpenApi",
            "summary": "This OpenAPI document",
            "security": auth_none(),
            "responses": {
                "200": ok("OpenAPI 3.1 document.", json!({ "type": "object" }))
            }
        }),
    );
    paths.add(
        "/docs",
        "get",
        json!({
            "tags": [TAG_OPS],
            "operationId": "getDocs",
            "summary": "API reference UI",
            "description": "This reference, rendered by a bundled copy of Scalar (no CDN).",
            "security": auth_none(),
            "responses": {
                "200": {
                    "description": "HTML page.",
                    "content": { "text/html": { "schema": { "type": "string" } } }
                }
            }
        }),
    );
}

// ── Schemas ───────────────────────────────────────────────────────

fn object(required: &[&str], properties: Value) -> Value {
    json!({ "type": "object", "required": required, "properties": properties })
}

/// An object schema that rejects unknown fields (`#[serde(deny_unknown_fields)]`).
fn closed_object(required: &[&str], properties: Value) -> Value {
    let mut value = object(required, properties);
    value["additionalProperties"] = Value::Bool(false);
    value
}

fn into_map(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("schema groups are objects"),
    }
}

/// `{kind: "<kind>"}` plus the variant's own fields: one internally tagged
/// enum variant.
fn tagged(tag: &str, kind: &str, required: &[&str], properties: Value) -> Value {
    let mut all_required = vec![tag];
    all_required.extend_from_slice(required);
    let mut value = object(&all_required, properties);
    value["properties"][tag] = json!({ "const": kind });
    value
}

fn discriminated(tag: &str, variants: &[(&str, &str)]) -> Value {
    let mapping: Map<String, Value> = variants
        .iter()
        .map(|(kind, name)| {
            (
                (*kind).to_owned(),
                Value::String(format!("#/components/schemas/{name}")),
            )
        })
        .collect();
    json!({
        "oneOf": variants.iter().map(|(_, name)| schema(name)).collect::<Vec<_>>(),
        "discriminator": { "propertyName": tag, "mapping": mapping },
    })
}

fn wire_schemas() -> Map<String, Value> {
    into_map(json!({
        "CaptureEvent": {
            "type": "object",
            "description": "One event. `event` and `distinct_id` are required (except on `/engage`, where a missing `event` means `$identify`). Unknown fields are ignored.",
            "required": ["event", "distinct_id"],
            "properties": {
                "event": { "type": "string", "minLength": 1 },
                "distinct_id": { "description": "String, or any JSON value (stringified). Truncated to 200 characters. Alias `$distinct_id`; falls back to `properties.distinct_id`." },
                "token": { "type": "string", "description": "Project token. Alternatives, in order: `$token`, `api_key`, `properties.token`. A batch-level token wins." },
                "uuid": { "type": "string", "format": "uuid", "description": "Idempotency key. Generated (UUIDv7) when absent." },
                "timestamp": { "type": "string", "description": "RFC 3339 (naive datetimes are UTC). Corrected for clock skew with `sent_at` unless `properties.$ignore_sent_at`; more than 23 h in the future becomes now; absent means now." },
                "offset": { "type": "integer", "description": "Milliseconds ago. Wins over `timestamp`." },
                "properties": { "type": "object", "additionalProperties": true },
                "$set": { "type": "object", "description": "Person properties to set. Moved into `properties.$set`." },
                "$set_once": { "type": "object", "description": "Person properties to set if absent. Moved into `properties.$set_once`." }
            }
        },
        "CaptureBatch": {
            "type": "object",
            "required": ["batch"],
            "properties": {
                "api_key": { "type": "string", "description": "Project token for every event. Alias `token`." },
                "token": { "type": "string" },
                "sent_at": { "type": "string", "description": "Client send time (RFC 3339), for clock-skew correction." },
                "historical_migration": { "type": "boolean", "description": "Ignored." },
                "batch": array_of(schema("CaptureEvent"))
            }
        },
        "CaptureBody": {
            "description": "An array of events, a batch, or a single event.",
            "oneOf": [
                array_of(schema("CaptureEvent")),
                schema("CaptureBatch"),
                schema("CaptureEvent")
            ]
        },
        "CaptureOk": object(&["status"], json!({ "status": { "const": 1 } })),
        "RemoteConfigFields": {
            "type": "object",
            "description": "Remote-config fields. Present in `/array/{token}/config` and in every `/flags` and `/decide` response.",
            "required": ["supportedCompression", "hasFeatureFlags", "sessionRecording", "surveys", "heatmaps", "capturePerformance", "autocaptureExceptions", "isAuthenticated", "toolbarParams", "analytics", "defaultIdentifiedOnly", "siteApps", "config"],
            "properties": {
                "supportedCompression": { "type": "array", "items": { "type": "string" }, "examples": [["gzip", "gzip-js"]] },
                "hasFeatureFlags": { "const": true },
                "sessionRecording": { "const": false },
                "surveys": { "const": false },
                "heatmaps": { "const": false },
                "capturePerformance": { "const": false },
                "autocaptureExceptions": { "const": false },
                "isAuthenticated": { "const": false },
                "toolbarParams": { "type": "object" },
                "analytics": object(&["endpoint"], json!({ "endpoint": { "const": "/i/v0/e/" } })),
                "defaultIdentifiedOnly": { "const": true },
                "siteApps": { "type": "array", "maxItems": 0 },
                "config": object(&["enable_collect_everything"], json!({ "enable_collect_everything": { "type": "boolean" } }))
            }
        },
        "RemoteConfig": {
            "allOf": [
                schema("RemoteConfigFields"),
                object(&["token"], json!({ "token": { "type": "string" } }))
            ]
        },
        "FlagsRequest": {
            "type": "object",
            "description": "Unknown fields (`groups`, `group_properties`, …) are ignored.",
            "required": ["distinct_id"],
            "properties": {
                "token": { "type": "string", "description": "Project token. Alias `api_key`." },
                "api_key": { "type": "string" },
                "distinct_id": { "type": ["string", "number"], "description": "Non-empty. Truncated to 200 characters." },
                "$anon_distinct_id": { "type": "string", "description": "Pre-identify id, used for experience continuity." },
                "person_properties": { "type": "object", "description": "Override stored person properties for this evaluation." },
                "flag_keys": { "type": "array", "items": { "type": "string" }, "description": "Evaluate only these flags (posthog-js)." },
                "flag_keys_to_evaluate": { "type": "array", "items": { "type": "string" }, "description": "Evaluate only these flags (posthog-node). Wins over `flag_keys`." },
                "disable_flags": { "type": "boolean", "description": "`true` returns no flags, only config." }
            }
        },
        "FlagsCommon": {
            "allOf": [
                schema("RemoteConfigFields"),
                object(&["errorsWhileComputingFlags", "requestId"], json!({
                    "errorsWhileComputingFlags": { "type": "boolean" },
                    "requestId": { "type": "string", "format": "uuid" }
                }))
            ]
        },
        "FlagDetails": object(&["key", "enabled", "variant", "reason", "metadata"], json!({
            "key": { "type": "string" },
            "enabled": { "type": "boolean" },
            "variant": nullable("string"),
            "reason": object(&["code", "condition_index", "description"], json!({
                "code": { "type": "string", "enum": ["condition_match", "no_condition_match", "out_of_rollout_bound", "disabled"] },
                "condition_index": { "type": ["integer", "null"] },
                "description": { "type": "string" }
            })),
            "metadata": object(&["id", "version", "description"], json!({
                "id": { "type": "integer" },
                "version": { "type": "integer" },
                "description": nullable("string"),
                "payload": { "type": "string", "description": "JSON-encoded payload; present only when enabled and a payload is set." }
            }))
        })),
        "FlagValue": {
            "description": "`true`/`false`, or the variant key of an enabled multivariate flag.",
            "type": ["boolean", "string"]
        },
        "FlagsResponseV2": {
            "description": "`/flags?v=2` and `/decide?v=4`.",
            "allOf": [
                schema("FlagsCommon"),
                object(&["flags", "quotaLimited", "evaluatedAt"], json!({
                    "flags": { "type": "object", "additionalProperties": schema("FlagDetails") },
                    "quotaLimited": { "type": "array", "maxItems": 0 },
                    "evaluatedAt": { "type": "integer", "description": "Milliseconds since the epoch." }
                }))
            ]
        },
        "FlagsResponseV1": {
            "description": "`/flags` (no `v` or `v=1`) and `/decide?v=3`.",
            "allOf": [
                schema("FlagsCommon"),
                object(&["featureFlags", "featureFlagPayloads"], json!({
                    "featureFlags": { "type": "object", "additionalProperties": schema("FlagValue") },
                    "featureFlagPayloads": { "type": "object", "additionalProperties": { "type": "string" }, "description": "JSON-encoded payloads of enabled flags." }
                }))
            ]
        },
        "DecideResponseV2": {
            "description": "`/decide?v=2`.",
            "allOf": [
                schema("FlagsCommon"),
                object(&["featureFlags"], json!({
                    "featureFlags": { "type": "object", "additionalProperties": schema("FlagValue") }
                }))
            ]
        },
        "DecideResponseV1": {
            "description": "`/decide` with no `v` or `v=1`.",
            "allOf": [
                schema("FlagsCommon"),
                object(&["featureFlags"], json!({
                    "featureFlags": { "type": "array", "items": { "type": "string" }, "description": "Keys of enabled flags." }
                }))
            ]
        },
        "WireError": object(&["type", "code", "detail", "attr"], json!({
            "type": { "type": "string", "examples": ["validation_error", "authentication_error", "server_error"] },
            "code": { "type": "string", "examples": ["invalid_payload", "missing_distinct_id", "invalid_api_key", "unavailable"] },
            "detail": { "type": "string" },
            "attr": { "type": "null" }
        })),
        "DetailError": object(&["detail"], json!({ "detail": { "type": "string" } })),
        "LocalEvaluationResponse": object(&["flags", "group_type_mapping", "cohorts"], json!({
            "flags": array_of(schema("LocalFlagDefinition")),
            "group_type_mapping": { "type": "object" },
            "cohorts": { "type": "object" }
        })),
        "LocalFlagDefinition": object(
            &["id", "name", "key", "active", "deleted", "ensure_experience_continuity", "version", "filters"],
            json!({
                "id": { "type": "integer" },
                "name": { "type": "string" },
                "key": { "type": "string" },
                "active": { "type": "boolean" },
                "deleted": { "const": false },
                "ensure_experience_continuity": { "type": "boolean" },
                "version": { "type": "integer" },
                "filters": object(&["groups", "multivariate", "payloads", "aggregation_group_type_index"], json!({
                    "groups": array_of(object(&["properties", "rollout_percentage", "variant"], json!({
                        "properties": array_of(object(&["key", "type", "operator", "value"], json!({
                            "key": { "type": "string" },
                            "type": { "const": "person" },
                            "operator": schema("PropertyOperator"),
                            "value": {}
                        }))),
                        "rollout_percentage": nullable("number"),
                        "variant": nullable("string")
                    }))),
                    "multivariate": nullable_ref("Multivariate"),
                    "payloads": { "type": "object", "additionalProperties": { "type": "string" } },
                    "aggregation_group_type_index": { "type": "null" }
                }))
            })
        )
    }))
}

fn common_schemas() -> Map<String, Value> {
    into_map(json!({
        "ApiError": {
            "type": "object",
            "required": ["error"],
            "properties": {
                "error": object(&["code", "message"], json!({
                    "code": { "type": "string", "examples": ["invalid_request", "unauthorized", "forbidden", "not_found", "conflict", "invalid_query", "query_busy", "query_timeout", "unavailable", "internal_error"] },
                    "message": { "type": "string" },
                    "request_id": { "type": "string", "description": "Matches the `x-request-id` response header." },
                    "field": { "type": "string", "description": "Offending field, when known." }
                }))
            }
        },
        "PropertySource": { "type": "string", "enum": ["event", "person"], "default": "event" },
        "PropertyOperator": {
            "type": "string",
            "enum": ["exact", "is_not", "icontains", "not_icontains", "regex", "not_regex", "gt", "gte", "lt", "lte", "is_set", "is_not_set", "is_date_before", "is_date_after"],
            "default": "exact",
            "description": "PostHog's operators. `exact`/`is_not` take a scalar or an array (any of / none of)."
        },
        "PropertyFilter": object(&["key"], json!({
            "key": { "type": "string" },
            "type": schema("PropertySource"),
            "operator": schema("PropertyOperator"),
            "value": { "description": "String, number, boolean, or an array of those. Ignored by `is_set`/`is_not_set`." }
        })),
        "DateRange": object(&["date_from"], json!({
            "date_from": { "type": "string", "default": "-7d", "description": "`-24h`, `-7d`, `-4w`, `-3m`, `-1y`, `dStart`, `mStart`, `yStart`, `all`, or an ISO 8601 date/datetime." },
            "date_to": { "type": ["string", "null"], "description": "Same forms; `null` means now." }
        })),
        "Interval": { "type": "string", "enum": ["hour", "day", "week", "month"], "default": "day" },
        "Breakdown": object(&["property"], json!({
            "property": { "type": "string" },
            "type": schema("PropertySource"),
            "limit": { "type": "integer", "default": 10, "description": "Top-N values kept; the rest fold into `$$_other`. Events without the property are `$$_none`." }
        }))
    }))
}

fn query_schemas() -> Map<String, Value> {
    let properties = || array_of(schema("PropertyFilter"));
    into_map(json!({
        "Math": {
            "type": "string",
            "enum": ["total", "dau", "weekly_active", "monthly_active", "unique_session", "sum", "avg", "min", "max", "median", "p90", "p95", "p99"],
            "default": "total",
            "description": "`total`: events. `dau`: unique persons per bucket. `weekly_active`/`monthly_active`: unique persons in the trailing 7/30 days. `unique_session`: unique `$session_id`. The rest aggregate `math_property`."
        },
        "EventNode": object(&[], json!({
            "event": { "type": ["string", "null"], "description": "Event name; `null` means all events." },
            "custom_name": { "type": ["string", "null"], "description": "Display label." },
            "properties": properties(),
            "math": schema("Math"),
            "math_property": { "type": ["string", "null"], "description": "Event property for `sum`, `avg`, `min`, `max`, `median`, `p90`, `p95`, `p99`." }
        })),
        "ChartDisplay": {
            "type": "string",
            "enum": ["ActionsLineGraph", "ActionsAreaGraph", "ActionsBar", "ActionsBarValue", "ActionsTable", "ActionsPie", "BoldNumber"],
            "default": "ActionsLineGraph"
        },
        "TrendsQuery": tagged("kind", "TrendsQuery", &["series"], json!({
            "series": array_of(schema("EventNode")),
            "date_range": schema("DateRange"),
            "interval": schema("Interval"),
            "properties": properties(),
            "breakdown": nullable_ref("Breakdown"),
            "formula": { "type": ["string", "null"], "description": "Arithmetic over series letters, e.g. `A / B * 100`. The result then holds the formula series instead of the raw ones." },
            "compare": { "type": "boolean", "default": false, "description": "Also return the previous period." },
            "display": schema("ChartDisplay")
        })),
        "FunnelWindow": object(&["interval", "unit"], json!({
            "interval": { "type": "integer", "minimum": 0, "default": 14 },
            "unit": { "type": "string", "enum": ["minute", "hour", "day", "week"], "default": "day" }
        })),
        "FunnelExclusion": object(&["event", "from_step", "to_step"], json!({
            "event": { "type": "string" },
            "from_step": { "type": "integer", "minimum": 0 },
            "to_step": { "type": "integer", "minimum": 0 }
        })),
        "FunnelsQuery": tagged("kind", "FunnelsQuery", &["series"], json!({
            "series": { "type": "array", "items": schema("EventNode"), "description": "Steps in order; `math` is ignored." },
            "date_range": schema("DateRange"),
            "properties": properties(),
            "breakdown": nullable_ref("Breakdown"),
            "funnel_window": schema("FunnelWindow"),
            "funnel_order": { "type": "string", "enum": ["ordered", "strict", "unordered"], "default": "ordered" },
            "exclusions": array_of(schema("FunnelExclusion"))
        })),
        "RetentionQuery": tagged("kind", "RetentionQuery", &["target", "returning"], json!({
            "target": schema("EventNode"),
            "returning": schema("EventNode"),
            "period": { "type": "string", "enum": ["day", "week", "month"], "default": "day" },
            "total_intervals": { "type": "integer", "minimum": 0, "default": 8 },
            "retention_type": { "type": "string", "enum": ["retention_recurring", "retention_first_time"], "default": "retention_recurring" },
            "properties": properties()
        })),
        "LifecycleQuery": tagged("kind", "LifecycleQuery", &["series"], json!({
            "series": schema("EventNode"),
            "date_range": schema("DateRange"),
            "interval": schema("Interval"),
            "properties": properties()
        })),
        "StickinessQuery": tagged("kind", "StickinessQuery", &["series"], json!({
            "series": array_of(schema("EventNode")),
            "date_range": schema("DateRange"),
            "interval": schema("Interval"),
            "properties": properties()
        })),
        "PathsQuery": tagged("kind", "PathsQuery", &[], json!({
            "paths_type": { "type": "string", "enum": ["pageviews", "custom_events", "all"], "default": "pageviews" },
            "start_point": nullable("string"),
            "end_point": nullable("string"),
            "step_limit": { "type": "integer", "minimum": 0, "default": 5 },
            "edge_limit": { "type": "integer", "minimum": 0, "default": 50 },
            "date_range": schema("DateRange"),
            "properties": properties()
        })),
        "SqlQuery": tagged("kind", "SqlQuery", &["query"], json!({
            "query": { "type": "string", "description": "Read-only SQL over the relation `events` (uuid, event, distinct_id, person_id, timestamp, properties, and promoted columns). Capped in rows, time and memory." }
        })),
        "InsightQuery": discriminated("kind", &[
            ("TrendsQuery", "TrendsQuery"),
            ("FunnelsQuery", "FunnelsQuery"),
            ("RetentionQuery", "RetentionQuery"),
            ("LifecycleQuery", "LifecycleQuery"),
            ("StickinessQuery", "StickinessQuery"),
            ("PathsQuery", "PathsQuery"),
            ("SqlQuery", "SqlQuery"),
        ]),
        "QueryRequest": object(&["query"], json!({
            "query": schema("InsightQuery"),
            "refresh": { "type": "boolean", "default": false, "description": "Bypass the result cache." }
        })),
        "ActorSelectionTrendsPoint": tagged("type", "TrendsPoint", &["series_index", "day"], json!({
            "series_index": { "type": "integer", "minimum": 0 },
            "day": { "type": "string", "description": "Bucket start, as in `TrendSeries.days`." },
            "breakdown_value": nullable("string")
        })),
        "ActorSelectionFunnelStep": tagged("type", "FunnelStep", &["step", "converted"], json!({
            "step": { "type": "integer", "minimum": 0 },
            "converted": { "type": "boolean", "description": "`true`: reached `step`. `false`: reached `step - 1` but not `step`." },
            "breakdown_value": nullable("string")
        })),
        "ActorSelectionRetentionCell": tagged("type", "RetentionCell", &["cohort_date", "interval"], json!({
            "cohort_date": { "type": "string" },
            "interval": { "type": "integer", "minimum": 0 }
        })),
        "ActorSelectionLifecycleCell": tagged("type", "LifecycleCell", &["status", "day"], json!({
            "status": { "type": "string", "enum": ["new", "returning", "resurrecting", "dormant"] },
            "day": { "type": "string" }
        })),
        "ActorSelectionStickinessBar": tagged("type", "StickinessBar", &["series_index", "intervals"], json!({
            "series_index": { "type": "integer", "minimum": 0 },
            "intervals": { "type": "integer", "minimum": 0 }
        })),
        "ActorSelection": discriminated("type", &[
            ("TrendsPoint", "ActorSelectionTrendsPoint"),
            ("FunnelStep", "ActorSelectionFunnelStep"),
            ("RetentionCell", "ActorSelectionRetentionCell"),
            ("LifecycleCell", "ActorSelectionLifecycleCell"),
            ("StickinessBar", "ActorSelectionStickinessBar"),
        ]),
        "ActorsRequest": object(&["query", "selection"], json!({
            "query": schema("InsightQuery"),
            "selection": schema("ActorSelection"),
            "offset": { "type": "integer", "minimum": 0, "default": 0 },
            "limit": { "type": "integer", "minimum": 0, "default": 100 }
        })),
        "ActorsResponse": object(&["persons", "has_more"], json!({
            "persons": array_of(schema("PersonSummary")),
            "has_more": { "type": "boolean" }
        }))
    }))
}

fn result_schemas() -> Map<String, Value> {
    let numbers = || array_of(json!({ "type": "number" }));
    let integers = || array_of(json!({ "type": "integer" }));
    let strings = || array_of(json!({ "type": "string" }));
    into_map(json!({
        "QueryResponse": object(&["result", "meta"], json!({
            "result": schema("InsightResult"),
            "meta": schema("QueryMeta")
        })),
        "QueryMeta": object(&["kind", "elapsed_ms", "cached", "data_version", "date_from", "date_to", "timezone"], json!({
            "kind": { "type": "string" },
            "elapsed_ms": { "type": "integer" },
            "cached": { "type": "boolean" },
            "data_version": { "type": "integer", "description": "Changes whenever the project's stored events change." },
            "date_from": { "type": "string", "description": "Resolved range start, RFC 3339 UTC." },
            "date_to": { "type": "string", "description": "Resolved range end, RFC 3339 UTC." },
            "timezone": { "type": "string" }
        })),
        "TrendsResult": tagged("kind", "Trends", &["series"], json!({ "series": array_of(schema("TrendSeries")) })),
        "FunnelsResult": tagged("kind", "Funnels", &["steps", "breakdowns", "time_to_convert"], json!({
            "steps": array_of(schema("FunnelStepResult")),
            "breakdowns": array_of(schema("FunnelBreakdownResult")),
            "time_to_convert": array_of(schema("HistogramBin"))
        })),
        "RetentionResult": tagged("kind", "Retention", &["period", "cohorts"], json!({
            "period": { "type": "string", "enum": ["day", "week", "month"] },
            "cohorts": array_of(schema("RetentionCohort"))
        })),
        "LifecycleResult": tagged("kind", "Lifecycle", &["days", "labels", "new", "returning", "resurrecting", "dormant"], json!({
            "days": { "type": "array", "items": { "type": "string" }, "description": "Bucket starts, RFC 3339." },
            "labels": strings(),
            "new": integers(),
            "returning": integers(),
            "resurrecting": integers(),
            "dormant": { "type": "array", "items": { "type": "integer" }, "description": "Negative counts, as PostHog reports them." }
        })),
        "StickinessResult": tagged("kind", "Stickiness", &["series"], json!({ "series": array_of(schema("StickinessSeries")) })),
        "PathsResult": tagged("kind", "Paths", &["links"], json!({ "links": array_of(schema("PathLink")) })),
        "SqlResult": tagged("kind", "Sql", &["columns", "types", "rows", "truncated"], json!({
            "columns": strings(),
            "types": strings(),
            "rows": array_of(array_of(json!({}))),
            "truncated": { "type": "boolean" }
        })),
        "InsightResult": discriminated("kind", &[
            ("Trends", "TrendsResult"),
            ("Funnels", "FunnelsResult"),
            ("Retention", "RetentionResult"),
            ("Lifecycle", "LifecycleResult"),
            ("Stickiness", "StickinessResult"),
            ("Paths", "PathsResult"),
            ("Sql", "SqlResult"),
        ]),
        "TrendSeries": object(&["label", "series_index", "breakdown_value", "compare", "days", "labels", "data", "aggregated_value"], json!({
            "label": { "type": "string" },
            "series_index": { "type": ["integer", "null"], "description": "Index into the query's `series`; `null` for a formula series." },
            "breakdown_value": nullable("string"),
            "compare": { "type": ["string", "null"], "description": "`previous` for the comparison period." },
            "days": { "type": "array", "items": { "type": "string" }, "description": "Bucket starts, RFC 3339." },
            "labels": strings(),
            "data": numbers(),
            "aggregated_value": { "type": "number", "description": "Total over the range for counts; whole-range value for unique-person and property maths." }
        })),
        "FunnelStepResult": object(&["order", "name", "count", "conversion_from_previous", "conversion_from_start", "dropped_off", "average_conversion_time_s", "median_conversion_time_s"], json!({
            "order": { "type": "integer", "description": "0-based." },
            "name": { "type": "string" },
            "count": { "type": "integer" },
            "conversion_from_previous": { "type": "number" },
            "conversion_from_start": { "type": "number" },
            "dropped_off": { "type": "integer" },
            "average_conversion_time_s": nullable("number"),
            "median_conversion_time_s": nullable("number")
        })),
        "FunnelBreakdownResult": object(&["breakdown_value", "steps"], json!({
            "breakdown_value": { "type": "string" },
            "steps": array_of(schema("FunnelStepResult"))
        })),
        "HistogramBin": object(&["from_s", "to_s", "count"], json!({
            "from_s": { "type": "number" },
            "to_s": { "type": "number" },
            "count": { "type": "integer" }
        })),
        "RetentionCohort": object(&["date", "label", "size", "values"], json!({
            "date": { "type": "string", "description": "Cohort period start, RFC 3339." },
            "label": { "type": "string" },
            "size": { "type": "integer" },
            "values": { "type": "array", "items": { "type": "integer" }, "description": "`values[i]`: persons of the cohort who returned in period `i` (`values[0] == size`). Future periods are omitted." }
        })),
        "StickinessSeries": object(&["label", "series_index", "data", "labels"], json!({
            "label": { "type": "string" },
            "series_index": { "type": "integer" },
            "data": { "type": "array", "items": { "type": "integer" }, "description": "`data[i]`: persons active in exactly `i + 1` intervals." },
            "labels": strings()
        })),
        "PathLink": object(&["source", "target", "value", "average_conversion_time_s"], json!({
            "source": { "type": "string", "description": "Step-prefixed node, e.g. `1_/pricing`." },
            "target": { "type": "string" },
            "value": { "type": "integer" },
            "average_conversion_time_s": { "type": "number" }
        }))
    }))
}

fn explore_schemas() -> Map<String, Value> {
    let properties_object = || json!({ "type": "object", "additionalProperties": true });
    let web_metric = || schema("WebMetric");
    into_map(json!({
        "PersonSummary": object(&["id", "display_name", "distinct_ids", "properties", "is_identified", "created_at", "last_seen"], json!({
            "id": { "type": "string" },
            "display_name": { "type": "string", "description": "`email`, then `name`, then the first distinct id." },
            "distinct_ids": { "type": "array", "items": { "type": "string" }, "description": "Up to 10; the detail endpoint returns all." },
            "properties": properties_object(),
            "is_identified": { "type": "boolean" },
            "created_at": { "type": "string", "description": "RFC 3339." },
            "last_seen": nullable("string")
        })),
        "PersonListResponse": object(&["persons", "next_cursor"], json!({
            "persons": array_of(schema("PersonSummary")),
            "next_cursor": { "type": ["string", "null"], "description": "Pass back as `cursor`; `null` on the last page." }
        })),
        "PersonDetail": object(&["person", "distinct_ids", "event_count", "first_seen", "last_seen", "session_count"], json!({
            "person": schema("PersonSummary"),
            "distinct_ids": { "type": "array", "items": { "type": "string" } },
            "event_count": { "type": "integer" },
            "first_seen": nullable("string"),
            "last_seen": nullable("string"),
            "session_count": { "type": "integer" }
        })),
        "EventRow": object(&["uuid", "event", "distinct_id", "person_id", "timestamp", "properties"], json!({
            "uuid": { "type": "string" },
            "event": { "type": "string" },
            "distinct_id": { "type": "string" },
            "person_id": { "type": "string" },
            "timestamp": { "type": "string", "description": "RFC 3339." },
            "properties": properties_object()
        })),
        "EventListResponse": object(&["events", "next_before"], json!({
            "events": array_of(schema("EventRow")),
            "next_before": { "type": ["string", "null"], "description": "Pass back as `before`; `null` when there is nothing older." }
        })),
        "CatalogEvent": object(&["name", "count", "last_seen"], json!({
            "name": { "type": "string" },
            "count": { "type": "integer" },
            "last_seen": nullable("string")
        })),
        "CatalogProperty": object(&["key", "type", "property_type", "count"], json!({
            "key": { "type": "string" },
            "type": { "type": "string", "enum": ["event", "person"] },
            "property_type": { "type": "string", "examples": ["string", "number", "boolean", "array", "object"] },
            "count": { "type": "integer", "description": "Events (or persons) carrying the key." }
        })),
        "CatalogValue": object(&["value", "count"], json!({
            "value": { "type": "string" },
            "count": { "type": "integer" }
        })),
        "WebMetric": object(&["value", "previous"], json!({
            "value": { "type": "number" },
            "previous": { "type": ["number", "null"], "description": "Same metric over the preceding period of equal length." }
        })),
        "WebOverview": object(&["visitors", "pageviews", "sessions", "bounce_rate", "session_duration_s", "interval", "days", "visitors_series", "pageviews_series", "live_visitors"], json!({
            "visitors": web_metric(),
            "pageviews": web_metric(),
            "sessions": web_metric(),
            "bounce_rate": { "$ref": "#/components/schemas/WebMetric", "description": "Percent, 0–100." },
            "session_duration_s": web_metric(),
            "interval": schema("Interval"),
            "days": { "type": "array", "items": { "type": "string" }, "description": "Bucket starts, RFC 3339." },
            "visitors_series": array_of(json!({ "type": "integer" })),
            "pageviews_series": array_of(json!({ "type": "integer" })),
            "live_visitors": { "type": "integer", "description": "Persons with a pageview in the last 5 minutes." }
        })),
        "WebDimension": {
            "type": "string",
            "enum": ["page", "entry_page", "exit_page", "referring_domain", "utm_source", "utm_medium", "utm_campaign", "browser", "os", "device_type", "country"]
        },
        "WebBreakdownRow": object(&["value", "visitors", "views", "bounce_rate"], json!({
            "value": { "type": "string" },
            "visitors": { "type": "integer" },
            "views": { "type": "integer" },
            "bounce_rate": { "type": ["number", "null"], "description": "Percent; only for page-like dimensions." }
        })),
        "WebBreakdown": object(&["dimension", "rows"], json!({
            "dimension": schema("WebDimension"),
            "rows": array_of(schema("WebBreakdownRow"))
        })),
        "ProjectStatus": object(&["has_events", "last_event_at", "ingestion_lag_seconds", "stored_events", "stored_bytes", "first_day", "last_day"], json!({
            "has_events": { "type": "boolean" },
            "last_event_at": { "type": ["string", "null"], "description": "When the newest stored data was written, RFC 3339." },
            "ingestion_lag_seconds": { "type": "number", "description": "Age of the oldest acknowledged event that is not yet queryable; 0 when caught up." },
            "stored_events": { "type": "integer" },
            "stored_bytes": { "type": "integer" },
            "first_day": { "type": ["string", "null"], "format": "date" },
            "last_day": { "type": ["string", "null"], "format": "date" }
        }))
    }))
}

fn flag_schemas() -> Map<String, Value> {
    into_map(json!({
        "FlagVariant": object(&["key", "rollout_percentage"], json!({
            "key": { "type": "string" },
            "name": nullable("string"),
            "rollout_percentage": { "type": "number", "minimum": 0, "maximum": 100, "description": "Share of matched persons. Variants sum to 100." }
        })),
        "Multivariate": object(&["variants"], json!({ "variants": array_of(schema("FlagVariant")) })),
        "FlagConditionGroup": object(&[], json!({
            "properties": { "type": "array", "items": schema("PropertyFilter"), "description": "All must match (person properties)." },
            "rollout_percentage": { "type": ["number", "null"], "description": "Share of matching persons admitted; `null` = 100." },
            "variant": { "type": ["string", "null"], "description": "Force this variant for matching persons." }
        })),
        "FlagFilters": object(&[], json!({
            "groups": { "type": "array", "items": schema("FlagConditionGroup"), "description": "Release conditions. The first group that matches, rollout included, enables the flag." },
            "multivariate": nullable_ref("Multivariate"),
            "payloads": { "type": "object", "additionalProperties": true, "description": "Payload per value: key `true` for boolean flags, the variant key for multivariate flags." }
        })),
        "FeatureFlag": object(&["id", "key", "name", "active", "filters", "ensure_experience_continuity", "created_at", "updated_at"], json!({
            "id": { "type": "integer" },
            "key": { "type": "string" },
            "name": { "type": "string", "description": "Free-text description." },
            "active": { "type": "boolean" },
            "filters": schema("FlagFilters"),
            "ensure_experience_continuity": { "type": "boolean", "description": "Keep a person's value stable across identify." },
            "created_at": { "type": "string" },
            "updated_at": { "type": "string" }
        })),
        "FeatureFlagInput": object(&["key"], json!({
            "key": { "type": "string", "maxLength": 400 },
            "name": { "type": "string", "default": "" },
            "active": { "type": "boolean", "default": true },
            "filters": schema("FlagFilters"),
            "ensure_experience_continuity": { "type": "boolean", "default": false }
        })),
        "FeatureFlagPatch": object(&[], json!({
            "key": { "type": "string", "maxLength": 400 },
            "name": { "type": "string" },
            "active": { "type": "boolean" },
            "filters": schema("FlagFilters"),
            "ensure_experience_continuity": { "type": "boolean" }
        })),
        "FlagEvaluation": object(&["key", "enabled", "variant", "reason", "condition_index", "payload"], json!({
            "key": { "type": "string" },
            "enabled": { "type": "boolean" },
            "variant": nullable("string"),
            "reason": { "type": "string", "enum": ["condition_match", "no_condition_match", "out_of_rollout_bound", "disabled"] },
            "condition_index": { "type": ["integer", "null"] },
            "payload": { "description": "The payload for the evaluated value, or `null`." }
        }))
    }))
}

fn workspace_schemas() -> Map<String, Value> {
    let name = || json!({ "type": "string", "minLength": 1, "maxLength": 128 });
    into_map(json!({
        "BootstrapResponse": object(&["setup_required"], json!({ "setup_required": { "type": "boolean" } })),
        "SetupRequest": object(&["email", "password", "organization_name"], json!({
            "email": { "type": "string", "format": "email", "maxLength": 254 },
            "password": { "type": "string", "minLength": 12, "maxLength": 1024 },
            "organization_name": name(),
            "project_name": { "type": "string", "minLength": 1, "maxLength": 128, "default": "Default" },
            "existing_project_token": { "type": "string", "description": "Claim the organization of an existing project with this capture token." },
            "setup_token": { "type": "string", "description": "The `HOGLET_SETUP_TOKEN` value, when the server sets one (the `X-Hoglet-Setup-Token` header works too)." }
        })),
        "LoginRequest": object(&["email", "password"], json!({
            "email": { "type": "string" },
            "password": { "type": "string" }
        })),
        "StatusOk": object(&["status"], json!({ "status": { "const": "ok" } })),
        "User": object(&["id", "email", "name"], json!({
            "id": { "type": "string" },
            "email": { "type": "string" },
            "name": { "type": "string" }
        })),
        "Project": object(&["id", "name", "token"], json!({
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "token": { "type": "string", "description": "Capture token (`phc_…`) for SDKs." }
        })),
        "Role": { "type": "string", "enum": ["owner", "admin", "member"] },
        "Organization": object(&["id", "name", "role", "projects"], json!({
            "id": { "type": "string" },
            "name": { "type": "string" },
            "role": schema("Role"),
            "projects": array_of(schema("Project"))
        })),
        "Workspace": object(&["user", "organizations"], json!({
            "user": schema("User"),
            "organizations": array_of(schema("Organization"))
        })),
        "KeyScope": {
            "type": "string",
            "enum": ["read", "write"],
            "default": "read",
            "description": "`read`: read analytics and resources. `write`: also create, change and delete them, subject to the user's role."
        },
        "PersonalApiKey": object(&["id", "name", "scope", "key_prefix", "last_used", "created_at"], json!({
            "id": { "type": "string" },
            "name": { "type": "string" },
            "scope": schema("KeyScope"),
            "key_prefix": { "type": "string", "description": "First 12 characters of the secret." },
            "last_used": { "type": ["integer", "null"], "description": "Unix seconds." },
            "created_at": { "type": "integer", "description": "Unix seconds." }
        })),
        "CreateKeyRequest": object(&["name"], json!({
            "name": name(),
            "scope": schema("KeyScope")
        })),
        "CreatedPersonalApiKey": object(&["key", "secret"], json!({
            "key": schema("PersonalApiKey"),
            "secret": { "type": "string", "description": "`phx_…`. Shown once." }
        })),
        "NameRequest": object(&["name"], json!({ "name": name() }))
    }))
}

fn team_schemas() -> Map<String, Value> {
    let seconds = || json!({ "type": "integer", "description": "Unix seconds." });
    let email = || json!({ "type": "string", "format": "email", "maxLength": 254 });
    into_map(json!({
        "Member": object(&["user_id", "name", "email", "role", "joined_at"], json!({
            "user_id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "email": { "type": "string" },
            "role": schema("Role"),
            "joined_at": seconds()
        })),
        "Invite": object(&["id", "email", "role", "created_by", "created_at", "expires_at"], json!({
            "id": { "type": "string", "format": "uuid" },
            "email": { "type": "string" },
            "role": schema("Role"),
            "created_by": { "type": "string", "description": "User id of the inviter." },
            "created_at": seconds(),
            "expires_at": seconds()
        })),
        "CreateInviteRequest": object(&["email", "role"], json!({
            "email": email(),
            "role": schema("Role")
        })),
        "CreatedInvite": object(&["invite", "token", "path"], json!({
            "invite": schema("Invite"),
            "token": { "type": "string", "description": "`hgi_…`. Shown once." },
            "path": { "type": "string", "description": "`/invite/{token}`: prefix with the address people use to reach Hoglet." }
        })),
        "UpdateMemberRequest": object(&["role"], json!({ "role": schema("Role") })),
        "InviteTokenRequest": object(&["token"], json!({ "token": { "type": "string" } })),
        "InvitePreview": object(&["organization_name", "email", "role", "expires_at", "account_exists"], json!({
            "organization_name": { "type": "string" },
            "email": { "type": "string" },
            "role": schema("Role"),
            "expires_at": seconds(),
            "account_exists": { "type": "boolean", "description": "An account with this email exists: accepting signs in with its password." }
        })),
        "AcceptInviteRequest": object(&["token", "password"], json!({
            "token": { "type": "string" },
            "name": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Required for a new account; ignored otherwise." },
            "password": { "type": "string", "minLength": 12, "maxLength": 1024, "description": "The new password, or the current one for an existing account." }
        }))
    }))
}

fn resource_schemas() -> Map<String, Value> {
    let seconds = || json!({ "type": "integer", "description": "Unix seconds." });
    let name = || json!({ "type": "string", "minLength": 1, "maxLength": 512 });
    into_map(json!({
        "InsightDraft": closed_object(&["name", "query_ir"], json!({
            "name": name(),
            "description": { "type": "string", "default": "" },
            "query_ir": { "type": "object", "description": "The insight's query, validated against the supported query subset." }
        })),
        "SavedInsight": object(&["id", "project_id", "name", "description", "query_ir", "created_by", "created_at", "updated_at"], json!({
            "id": { "type": "string" },
            "project_id": { "type": "string" },
            "name": { "type": "string" },
            "description": { "type": "string" },
            "query_ir": { "type": "object" },
            "created_by": { "type": "string", "description": "User id." },
            "created_at": seconds(),
            "updated_at": seconds()
        })),
        "DashboardDraft": closed_object(&["name"], json!({ "name": name() })),
        "DashboardTileInput": closed_object(&["insight_id"], json!({
            "insight_id": { "type": "string" },
            "x": { "type": "integer", "minimum": 0, "default": 0 },
            "y": { "type": "integer", "minimum": 0, "default": 0 },
            "w": { "type": "integer", "minimum": 1, "default": 4 },
            "h": { "type": "integer", "minimum": 1, "default": 3 }
        })),
        "DashboardTile": object(&["insight_id", "x", "y", "w", "h"], json!({
            "insight_id": { "type": "string" },
            "x": { "type": "integer" },
            "y": { "type": "integer" },
            "w": { "type": "integer" },
            "h": { "type": "integer" },
            "insight": schema("SavedInsight")
        })),
        "Dashboard": object(&["id", "project_id", "name", "tiles", "created_by", "created_at"], json!({
            "id": { "type": "string" },
            "project_id": { "type": "string" },
            "name": { "type": "string" },
            "tiles": array_of(schema("DashboardTile")),
            "created_by": { "type": "string", "description": "User id." },
            "created_at": seconds()
        })),
        "ShareTarget": { "type": "string", "enum": ["insight", "dashboard"] },
        "ShareDraft": closed_object(&["object_type", "object_id"], json!({
            "object_type": schema("ShareTarget"),
            "object_id": { "type": "string" },
            "expires_at": { "type": ["integer", "null"], "description": "Unix seconds; `null` never expires." }
        })),
        "ShareLink": object(&["id", "project_id", "object_type", "object_id", "token", "created_at", "expires_at"], json!({
            "id": { "type": "string" },
            "project_id": { "type": "string" },
            "object_type": schema("ShareTarget"),
            "object_id": { "type": "string" },
            "token": { "type": "string", "description": "`phs_…`; the public URL is `/shared/{token}`." },
            "created_at": seconds(),
            "expires_at": { "type": ["integer", "null"], "description": "Unix seconds." }
        })),
        "PublicShare": object(&["share"], json!({
            "share": schema("ShareLink"),
            "insight": schema("SavedInsight"),
            "dashboard": schema("Dashboard")
        }))
    }))
}

fn project_schemas() -> Map<String, Value> {
    into_map(json!({
        "ForwardingConfig": object(&["enabled", "host", "posthog_token"], json!({
            "enabled": { "type": "boolean" },
            "host": { "type": "string", "maxLength": 256, "examples": ["https://us.i.posthog.com"] },
            "posthog_token": { "type": "string", "maxLength": 128, "description": "Project API key (`phc_…`) of the PostHog project." }
        })),
        "ForwardingStatus": object(&["config", "forwarded", "dropped", "failed", "queued", "last_error"], json!({
            "config": nullable_ref("ForwardingConfig"),
            "forwarded": { "type": "integer" },
            "dropped": { "type": "integer", "description": "Not forwarded because the bounded queue was full." },
            "failed": { "type": "integer" },
            "queued": { "type": "integer" },
            "last_error": nullable("string")
        })),
        "ErasureReport": object(&["distinct_ids", "events"], json!({
            "distinct_ids": { "type": "integer", "description": "Distinct ids removed." },
            "events": { "type": "integer", "description": "Stored events removed." }
        })),
        "DemoResult": object(&["events"], json!({ "events": { "type": "integer" } }))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::application::{Application, ApplicationConfig};

    const METHODS: &[&str] = &[
        "get", "put", "post", "delete", "patch", "head", "options", "trace",
    ];

    /// Documented but not yet mounted by the production router. Each must
    /// either 404 at the router or be served; once served, drop it from here.
    const PENDING: &[(&str, &str)] = &[];

    fn operations(document: &Value) -> Vec<(String, String, Value)> {
        let mut found = Vec::new();
        for (path, item) in document["paths"].as_object().expect("paths") {
            for (method, operation) in item.as_object().expect("path item") {
                assert!(
                    METHODS.contains(&method.as_str()),
                    "{path}: unexpected key {method}"
                );
                found.push((method.clone(), path.clone(), operation.clone()));
            }
        }
        found
    }

    /// Follows a local `$ref` (`#/a/b/c`) to its target.
    fn resolve<'a>(document: &'a Value, reference: &str) -> Option<&'a Value> {
        let pointer = reference.strip_prefix('#')?;
        document.pointer(pointer)
    }

    fn collect_refs(value: &Value, refs: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if key == "$ref" {
                        refs.push(value.as_str().expect("$ref is a string").to_owned());
                    } else if key == "mapping" {
                        for target in value.as_object().expect("mapping").values() {
                            refs.push(target.as_str().expect("mapping target").to_owned());
                        }
                    } else {
                        collect_refs(value, refs);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect_refs(item, refs)),
            _ => {}
        }
    }

    #[test]
    fn document_is_well_formed_openapi_3_1() {
        let document = spec();
        let text = serde_json::to_string(&document).unwrap();
        let reparsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(reparsed, document);
        assert_eq!(document["openapi"], "3.1.0");
        assert!(document["info"]["title"].is_string());
        assert!(document["info"]["version"].is_string());

        let tags: Vec<&str> = document["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            tags,
            [
                "SDK wire",
                "Auth & workspace",
                "Team",
                "Insights & query",
                "Persons & events",
                "Web analytics",
                "Catalog",
                "Feature flags",
                "Dashboards, insights & shares",
                "Project",
                "Health & metrics",
            ]
        );

        let mut operation_ids = std::collections::HashSet::new();
        let mut used_tags = std::collections::HashSet::new();
        for (method, path, operation) in operations(&document) {
            let id = operation["operationId"]
                .as_str()
                .unwrap_or_else(|| panic!("{method} {path}: operationId"));
            assert!(
                operation_ids.insert(id.to_owned()),
                "duplicate operationId {id}"
            );
            let op_tags = operation["tags"].as_array().unwrap();
            assert_eq!(op_tags.len(), 1, "{method} {path}: one tag");
            let tag = op_tags[0].as_str().unwrap();
            assert!(tags.contains(&tag), "{method} {path}: unknown tag {tag}");
            used_tags.insert(tag.to_owned());
            assert!(operation["summary"].is_string(), "{method} {path}: summary");
            assert!(
                operation["security"].is_array(),
                "{method} {path}: explicit security"
            );
            assert!(
                !operation["responses"].as_object().unwrap().is_empty(),
                "{method} {path}: responses"
            );

            // Every path template parameter is declared exactly once.
            let declared: Vec<String> = operation["parameters"]
                .as_array()
                .map(|parameters| {
                    parameters
                        .iter()
                        .map(|parameter| match parameter.get("$ref") {
                            Some(reference) => resolve(&document, reference.as_str().unwrap())
                                .unwrap_or_else(|| panic!("{method} {path}: dangling {reference}"))
                                .clone(),
                            None => parameter.clone(),
                        })
                        .filter(|parameter| parameter["in"] == "path")
                        .map(|parameter| parameter["name"].as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            let mut templated: Vec<String> = path
                .split('/')
                .filter_map(|segment| segment.strip_prefix('{'))
                .map(|segment| segment.split('}').next().unwrap().to_owned())
                .collect();
            let mut declared_sorted = declared.clone();
            declared_sorted.sort();
            templated.sort();
            assert_eq!(
                declared_sorted, templated,
                "{method} {path}: path parameters"
            );
        }
        assert_eq!(used_tags.len(), tags.len(), "every tag has operations");

        let mut refs = Vec::new();
        collect_refs(&document, &mut refs);
        assert!(!refs.is_empty());
        for reference in refs {
            assert!(
                resolve(&document, &reference).is_some(),
                "dangling {reference}"
            );
        }
    }

    #[test]
    fn key_routes_are_documented() {
        let document = spec();
        for (method, path) in [
            ("post", "/e"),
            ("post", "/i/v0/e"),
            ("post", "/batch"),
            ("get", "/array/{token}/config"),
            ("get", "/array/{token}/config.js"),
            ("post", "/flags"),
            ("post", "/decide"),
            ("get", "/flags/definitions"),
            ("get", "/api/feature_flag/local_evaluation"),
            ("get", "/static/surveys.js"),
            ("post", "/api/auth/setup"),
            ("post", "/api/auth/keys"),
            ("get", "/api/organizations/{organization_id}/members"),
            ("patch", "/api/organizations/{organization_id}/members/{user_id}"),
            ("post", "/api/organizations/{organization_id}/invites"),
            ("post", "/api/invites/accept"),
            ("post", "/api/projects/{project_id}/query"),
            ("post", "/api/projects/{project_id}/query/actors"),
            ("get", "/api/projects/{project_id}/persons"),
            ("get", "/api/projects/{project_id}/events"),
            ("get", "/api/projects/{project_id}/web/overview"),
            ("get", "/api/projects/{project_id}/catalog/events"),
            ("patch", "/api/projects/{project_id}/feature_flags/{id}"),
            (
                "put",
                "/api/projects/{project_id}/dashboards/{dashboard_id}/tiles",
            ),
            ("get", "/shared/{token}"),
            (
                "post",
                "/api/projects/{project_id}/persons/{person_id}/erase",
            ),
            ("put", "/api/projects/{project_id}/forwarding"),
            ("get", "/ready"),
            ("get", "/metrics"),
        ] {
            assert!(
                document["paths"][path][method].is_object(),
                "{method} {path} is not documented"
            );
        }
        // Removed surfaces stay removed.
        for path in [
            "/api/flags",
            "/api/stats",
            "/api/admin/projects",
            "/api/projects/{project_id}/flags",
        ] {
            assert!(
                document["paths"].get(path).is_none(),
                "{path} is documented"
            );
        }

        let capture = &document["paths"]["/e"]["post"]["responses"];
        for status in ["200", "204", "400", "401", "413", "429", "503"] {
            assert!(capture[status].is_object(), "/e/ lacks {status}");
        }
    }

    fn concrete(path: &str) -> String {
        path.split('/')
            .map(|segment| match segment {
                "{project_id}" => "00000000-0000-4000-8000-000000000000".to_owned(),
                "{id}" => "1".to_owned(),
                "{token}" => "phc_docs_test".to_owned(),
                other if other.starts_with('{') => "docs-test".to_owned(),
                other => other.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// A router miss is axum's empty 404 or 405. Handlers answer 404 for a
    /// missing object with a JSON body, so an empty 404/405 means "no route".
    async fn served(router: &axum::Router, method: &str, uri: &str) -> (bool, StatusCode) {
        let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes()).unwrap();
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let miss = (status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED)
            && body.is_empty();
        (!miss, status)
    }

    #[tokio::test]
    async fn every_documented_operation_is_served_by_the_production_router() {
        let data = tempfile::tempdir().unwrap();
        let application = Application::prepare(ApplicationConfig::new(data.path()))
            .await
            .unwrap();
        application.mark_ready();
        let router = application.router();

        // The detector itself: an unmounted path and a wrong method are misses.
        assert!(!served(&router, "get", "/api/projects/not-a-route").await.0);
        assert!(!served(&router, "get", "/e/").await.0);

        let mut checked = 0;
        for (method, path, _) in operations(&spec()) {
            let uri = concrete(&path);
            let (is_served, status) = served(&router, &method, &uri).await;
            let pending = PENDING.contains(&(method.as_str(), path.as_str()));
            assert!(
                is_served || pending,
                "{method} {path} is documented but the router does not serve it ({status})"
            );
            checked += 1;
        }
        assert!(checked > 60, "only {checked} operations checked");

        // Trailing-slash aliases the reference mentions in prose.
        for (method, uri) in [
            ("post", "/e/"),
            ("post", "/i/v0/e/"),
            ("post", "/capture/"),
            ("post", "/track/"),
            ("post", "/engage/"),
            ("post", "/batch/"),
            ("post", "/flags/"),
            ("post", "/decide/"),
            ("get", "/array/phc_docs_test/config/"),
            ("get", "/flags/definitions/"),
            ("get", "/api/feature_flag/local_evaluation/"),
        ] {
            assert!(
                served(&router, method, uri).await.0,
                "{method} {uri} not served"
            );
        }

        application.shutdown().await.unwrap();
    }
}
