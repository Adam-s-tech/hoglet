//! `GET /array/:token/config` — the first request posthog-js makes.
//!
//! Implements spec/wire-compat.md "Config response". This endpoint is the hard
//! requirement: if it fails or is unparseable the SDK breaks; everything else
//! degrades gracefully. Features we don't implement are declared off
//! (`sessionRecording`, `surveys`, `heatmaps`) so the SDK never tries them.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::{capture::CaptureAuthorizer, token};

/// Remote-config fields shared by `/array/:token/config`, `config.js`, and
/// flattened into every flags response shape (spec/wire-compat.md: "All shapes
/// flatten a `config` object"). Features we don't implement are declared off
/// in every source the SDK consults, because an absent key means "unknown" and
/// can make it try loading the extension anyway.
pub fn remote_config_fields() -> serde_json::Map<String, serde_json::Value> {
    match json!({
        "supportedCompression": ["gzip", "gzip-js"],
        "hasFeatureFlags": true,
        "sessionRecording": false,
        "surveys": false,
        "heatmaps": false,
        "capturePerformance": false,
        "autocaptureExceptions": false,
        "isAuthenticated": false,
        "toolbarParams": {},
        "analytics": {"endpoint": "/i/v0/e/"},
        "defaultIdentifiedOnly": true,
        "siteApps": [],
        "config": {"enable_collect_everything": true},
    }) {
        serde_json::Value::Object(fields) => fields,
        _ => unreachable!(),
    }
}

#[derive(Clone)]
struct ConfigState {
    authorizer: Option<Arc<dyn CaptureAuthorizer>>,
}

/// Compatibility constructor for the legacy monolithic application.
pub fn router() -> Router {
    routes(ConfigState { authorizer: None })
}

/// Builds the public SDK config endpoint with fail-closed project-token
/// authorization.
pub fn wire_router(authorizer: Arc<dyn CaptureAuthorizer>) -> Router {
    routes(ConfigState {
        authorizer: Some(authorizer),
    })
}

fn routes(state: ConfigState) -> Router {
    // posthog-js requests both with and without a trailing slash depending on
    // version; serve both.
    Router::new()
        .route("/array/{token}/config", get(config))
        .route("/array/{token}/config/", get(config))
        .route("/array/{token}/config.js", get(config_js))
        .with_state(state)
}

async fn authorize(state: &ConfigState, token: &str) -> Result<(), StatusCode> {
    if token::validate(token).is_err() {
        // Invalid token shape → 401, which posthog-js never retries.
        return Err(StatusCode::UNAUTHORIZED);
    }
    if let Some(authorizer) = &state.authorizer
        && authorizer.authorize(token).await.is_err()
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

fn config_body(token: &str) -> serde_json::Value {
    let mut fields = remote_config_fields();
    fields.insert("token".into(), json!(token));
    serde_json::Value::Object(fields)
}

async fn config(State(state): State<ConfigState>, Path(token): Path<String>) -> Response {
    if let Err(status) = authorize(&state, &token).await {
        return status.into_response();
    }
    Json(config_body(&token)).into_response()
}

/// The script variant posthog-js tries first: assigns the remote config into
/// `window._POSTHOG_REMOTE_CONFIG[token]` (see posthog-js remote-config.ts;
/// on failure the SDK falls back to the JSON endpoint above).
async fn config_js(State(state): State<ConfigState>, Path(token): Path<String>) -> Response {
    if let Err(status) = authorize(&state, &token).await {
        return status.into_response();
    }
    let body = format!(
        "(function() {{\n  window._POSTHOG_REMOTE_CONFIG = window._POSTHOG_REMOTE_CONFIG || {{}};\n  window._POSTHOG_REMOTE_CONFIG[{token}] = {{\n    config: {config},\n    siteApps: []\n  }}\n}})();\n",
        token = json!(token),
        config = config_body(&token),
    );
    ([(header::CONTENT_TYPE, "application/javascript")], body).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(uri: &str) -> (StatusCode, serde_json::Value) {
        let res = crate::app()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("response body must be valid JSON")
        };
        (status, body)
    }

    #[tokio::test]
    async fn returns_parseable_config_with_recording_off() {
        let (status, body) = get("/array/phc_test123/config").await;
        assert_eq!(status, StatusCode::OK);
        // The three fields the SDK must see as disabled (CLAUDE.md milestone 1).
        assert_eq!(body["sessionRecording"], false);
        assert_eq!(body["surveys"], false);
        assert_eq!(body["heatmaps"], false);
        assert_eq!(body["token"], "phc_test123");
        assert_eq!(body["hasFeatureFlags"], true);
    }

    #[tokio::test]
    async fn serves_trailing_slash() {
        let (status, _) = get("/array/phc_test123/config/").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_personal_api_key_with_401() {
        let (status, _) = get("/array/phx_secret/config").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
