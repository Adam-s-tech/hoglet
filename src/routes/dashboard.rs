//! The dashboard, embedded in the binary (spec/README.md "Dashboard").
//!
//! A real React + TypeScript app (source in `web/`, types generated from the
//! Rust API structs by ts-rs) built by Vite into `web/dist/` and compiled into
//! the binary with `rust-embed`. No Node process, no CDN, no separate deploy —
//! the moment the dashboard is anything but part of this binary, the
//! single-binary claim is dead (`claims.md`).
//!
//! `web/dist/` is committed so `cargo build` needs no Node; rebuild it with
//! `cd web && npm run build` after changing the frontend.
//!
//! The app routes on the client (`/project/{id}/insights/…`), so any GET that
//! is not an API call, a wire endpoint, or a file serves `index.html`. Hashed
//! bundles under `/assets/` are immutable and cached for a year; `index.html`
//! is always revalidated so a new binary's UI is picked up immediately.

use axum::{
    Router,
    extract::Path,
    http::{HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// First path segments that never fall back to the SPA: the JSON API, the
/// PostHog wire surface, operational endpoints, and the asset directory.
/// A miss under these is a real 404 (clients rely on it; posthog-js never
/// retries a 4xx).
const RESERVED_PREFIXES: &[&str] = &[
    "api",
    "assets",
    "e",
    "i",
    "s",
    "capture",
    "batch",
    "track",
    "engage",
    "flags",
    "decide",
    "array",
    "static",
    "shared",
    "health",
    "metrics",
    "docs",
    "openapi.json",
];

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

pub fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/dashboard", get(index))
        // Hashed JS/CSS bundles live under /assets/.
        .route("/assets/{*path}", get(asset))
        .fallback(spa_fallback)
}

async fn index() -> Response {
    serve_index()
}

fn serve_index() -> Response {
    match Assets::get("index.html") {
        Some(file) => {
            let mut response = (
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/html; charset=utf-8"),
                    ),
                    (header::CACHE_CONTROL, HeaderValue::from_static(REVALIDATE)),
                ],
                file.data.into_owned(),
            )
                .into_response();
            if let Ok(policy) = HeaderValue::from_str(index_csp()) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_SECURITY_POLICY, policy);
            }
            response
        }
        None => (StatusCode::INTERNAL_SERVER_ERROR, "dashboard not built").into_response(),
    }
}

/// The dashboard's Content-Security-Policy, with the hash of every inline
/// `<script>` in the embedded `index.html` (the pre-paint theme bootstrap).
fn index_csp() -> &'static str {
    static POLICY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    POLICY.get_or_init(|| {
        let html = Assets::get("index.html")
            .map(|file| String::from_utf8_lossy(&file.data).into_owned())
            .unwrap_or_default();
        crate::security::dashboard_csp(&inline_script_hashes(&html))
    })
}

fn inline_script_hashes(html: &str) -> Vec<String> {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let mut hashes = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script>") {
        let after = &rest[start + "<script>".len()..];
        let Some(end) = after.find("</script>") else {
            break;
        };
        let digest = Sha256::digest(&after.as_bytes()[..end]);
        hashes.push(format!(
            "sha256-{}",
            base64::engine::general_purpose::STANDARD.encode(digest)
        ));
        rest = &after[end..];
    }
    hashes
}

fn serve_file(path: &str, cache: &'static str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = HeaderValue::from_str(mime.as_ref())
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    Some(
        (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            ],
            file.data.into_owned(),
        )
            .into_response(),
    )
}

async fn asset(Path(path): Path<String>) -> Response {
    if path.split(['/', '\\']).any(|part| part == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Hashed filenames are immutable — cache hard.
    serve_file(&format!("assets/{path}"), IMMUTABLE)
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn spa_fallback(method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    match classify(uri.path()) {
        Fallback::Index => serve_index(),
        Fallback::File(path) => {
            serve_file(path, REVALIDATE).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
        }
        Fallback::NotFound => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Fallback<'a> {
    Index,
    File(&'a str),
    NotFound,
}

fn classify(path: &str) -> Fallback<'_> {
    let trimmed = path.trim_start_matches('/');
    let first = trimmed.split('/').next().unwrap_or("");
    if RESERVED_PREFIXES.contains(&first) {
        return Fallback::NotFound;
    }
    let last = trimmed.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        // Looks like a file: serve it if embedded (favicon, robots.txt),
        // never answer a missing file with HTML.
        return Fallback::File(trimmed);
    }
    Fallback::Index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_routes_fall_back_to_index() {
        assert_eq!(classify("/project/abc/insights/new"), Fallback::Index);
        assert_eq!(classify("/project/abc"), Fallback::Index);
        assert_eq!(classify("/share/token123"), Fallback::Index);
        assert_eq!(classify("/login"), Fallback::Index);
    }

    #[test]
    fn api_and_wire_paths_never_fall_back() {
        for path in [
            "/api/projects/x/unknown",
            "/e",
            "/i/v0/e",
            "/batch/",
            "/flags",
            "/decide/",
            "/array/phc_x/config",
            "/static/array.js",
            "/shared/abc",
            "/health",
            "/assets/missing.js",
        ] {
            assert_eq!(classify(path), Fallback::NotFound, "{path}");
        }
    }

    #[test]
    fn csp_hashes_the_inline_theme_script_and_forbids_framing() {
        let policy = index_csp();
        assert!(policy.contains("script-src 'self' 'sha256-"), "{policy}");
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(!policy.contains("script-src 'self' 'unsafe-inline'"));
    }

    #[tokio::test]
    async fn assets_refuse_parent_segments() {
        let response = asset(Path("../index.html".to_owned())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn file_like_paths_are_files() {
        assert_eq!(classify("/robots.txt"), Fallback::File("robots.txt"));
        assert_eq!(
            classify("/project/x/app.js.map"),
            Fallback::File("project/x/app.js.map")
        );
    }

    #[tokio::test]
    async fn fallback_serves_index_html_with_revalidation() {
        let response = spa_fallback(Method::GET, Uri::from_static("/project/p/web")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            REVALIDATE
        );
    }

    #[tokio::test]
    async fn fallback_rejects_non_get_and_reserved() {
        let post = spa_fallback(Method::POST, Uri::from_static("/project/p")).await;
        assert_eq!(post.status(), StatusCode::NOT_FOUND);
        let api = spa_fallback(Method::GET, Uri::from_static("/api/nope")).await;
        assert_eq!(api.status(), StatusCode::NOT_FOUND);
    }
}
