//! Security regression tests over the real composed application: CSRF,
//! cookies, login throttling, setup, response headers, metrics, SSRF policy,
//! capture amplification, share redaction, tenant isolation, file modes.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use hoglet::application::{Application, ApplicationConfig};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

struct Harness {
    application: Application,
    router: Router,
    dir: tempfile::TempDir,
    cookie: String,
    project_id: String,
    token: String,
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, body)
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn post(uri: &str, cookie: Option<&str>) -> axum::http::request::Builder {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder
}

async fn start_with(tweak: impl FnOnce(&mut ApplicationConfig)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ApplicationConfig::new(dir.path());
    tweak(&mut config);
    let application = Application::prepare(config).await.unwrap();
    application.mark_ready();
    let router = application.router();
    let (status, headers, body) = send(
        &router,
        post("/api/auth/setup", None)
            .body(Body::from(
                json!({
                    "email": "owner@example.com",
                    "password": PASSWORD,
                    "organization_name": "Acme",
                    "project_name": "Site",
                    "existing_project_token": "phc_security_tests"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let cookie = headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let workspace = json_body(&body);
    Harness {
        application,
        router,
        dir,
        cookie,
        project_id: workspace["organizations"][0]["projects"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
        token: "phc_security_tests".to_owned(),
    }
}

async fn start() -> Harness {
    start_with(|_| {}).await
}

impl Harness {
    async fn finish(self) {
        drop(self.router);
        self.application.shutdown().await.unwrap();
        drop(self.dir);
    }

    fn insights_uri(&self) -> String {
        format!("/api/projects/{}/insights", self.project_id)
    }

    fn insight_body() -> Body {
        Body::from(
            json!({
                "name": "SQL",
                "query_ir": {"kind": "SqlQuery", "query": "select 1 as secret_text"}
            })
            .to_string(),
        )
    }
}

// --- CSRF -----------------------------------------------------------------

#[tokio::test]
async fn cookie_authenticated_mutations_refuse_cross_site_requests() {
    let h = start().await;
    let uri = h.insights_uri();
    let make = |extra: Option<(&str, &str)>, cookie: bool| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "hoglet.example");
        if cookie {
            builder = builder.header(header::COOKIE, &h.cookie);
        }
        if let Some((name, value)) = extra {
            builder = builder.header(name, value);
        }
        builder.body(Body::from(
            json!({"name": "i", "query_ir": {"kind": "SqlQuery", "query": "select 1"}}).to_string(),
        ))
        .unwrap()
    };

    // A page on another site, or a sibling subdomain, cannot ride the cookie.
    for hostile in [
        ("origin", "https://evil.example"),
        ("origin", "null"),
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
    ] {
        let (status, _, _) = send(&h.router, make(Some(hostile), true)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{hostile:?}");
    }
    // The dashboard itself, and non-browser clients, are unaffected.
    for fine in [
        Some(("origin", "https://hoglet.example")),
        Some(("sec-fetch-site", "same-origin")),
        None,
    ] {
        let (status, _, body) = send(&h.router, make(fine, true)).await;
        assert_eq!(status, StatusCode::CREATED, "{fine:?}: {}", String::from_utf8_lossy(&body));
    }
    // A cross-site origin without the cookie is just an anonymous request.
    let (status, _, _) = send(&h.router, make(Some(("origin", "https://evil.example")), false)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The wire edge keeps its permissive CORS: SDKs run on other origins.
    let (status, headers, _) = send(
        &h.router,
        Request::builder()
            .method(Method::POST)
            .uri("/e/")
            .header("origin", "https://customer.example")
            .body(Body::from(
                json!({"event": "x", "distinct_id": "u", "token": h.token}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("access-control-allow-origin"));
    h.finish().await;
}

#[tokio::test]
async fn dashboard_and_api_routes_send_no_cors_headers() {
    let h = start().await;
    for (method, uri) in [
        (Method::OPTIONS, "/api/auth/login"),
        (Method::GET, "/api/auth/me"),
        (Method::GET, "/api/shares/phs_x"),
    ] {
        let (_, headers, _) = send(
            &h.router,
            Request::builder()
                .method(method)
                .uri(uri)
                .header("origin", "https://evil.example")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert!(
            !headers.contains_key("access-control-allow-origin"),
            "{uri} must not be cross-origin readable"
        );
    }
    h.finish().await;
}

// --- Cookies, setup, headers ----------------------------------------------

#[tokio::test]
async fn session_cookie_is_secure_behind_tls_and_http_only_always() {
    let h = start().await;
    let login = |proto: Option<&'static str>| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/auth/login")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(proto) = proto {
            builder = builder.header("x-forwarded-proto", proto);
        }
        builder
            .body(Body::from(
                json!({"email": "owner@example.com", "password": PASSWORD}).to_string(),
            ))
            .unwrap()
    };
    let (_, plain, _) = send(&h.router, login(None)).await;
    let (_, tls, _) = send(&h.router, login(Some("https"))).await;
    let plain = plain.get(header::SET_COOKIE).unwrap().to_str().unwrap().to_owned();
    let tls = tls.get(header::SET_COOKIE).unwrap().to_str().unwrap().to_owned();
    assert!(plain.contains("HttpOnly") && plain.contains("SameSite=Lax"));
    assert!(!plain.contains("Secure"));
    assert!(tls.contains("HttpOnly") && tls.contains("SameSite=Lax") && tls.contains("; Secure"));
    // 256-bit random id, not a UUID.
    let id = tls.split(';').next().unwrap().trim_start_matches("hoglet_sid=");
    assert_eq!(id.len(), 64);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    h.finish().await;
}

#[tokio::test]
async fn setup_token_gates_first_run_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ApplicationConfig::new(dir.path());
    config.security.setup_token = Some("let-me-in".into());
    let application = Application::prepare(config).await.unwrap();
    let router = application.router();
    let attempt = |token: Option<&'static str>| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/auth/setup")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header("x-hoglet-setup-token", token);
        }
        builder
            .body(Body::from(
                json!({
                    "email": "owner@example.com",
                    "password": PASSWORD,
                    "organization_name": "Acme"
                })
                .to_string(),
            ))
            .unwrap()
    };
    assert_eq!(send(&router, attempt(None)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&router, attempt(Some("guess"))).await.0, StatusCode::FORBIDDEN);
    // Still open for the real operator.
    let (_, _, body) = send(
        &router,
        Request::get("/api/auth/bootstrap").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(json_body(&body), json!({"setup_required": true}));
    assert_eq!(send(&router, attempt(Some("let-me-in"))).await.0, StatusCode::OK);
    // And closed to everyone afterwards.
    assert_eq!(send(&router, attempt(Some("let-me-in"))).await.0, StatusCode::CONFLICT);
    drop(router);
    application.shutdown().await.unwrap();
}

#[tokio::test]
async fn responses_carry_hardening_headers() {
    let h = start().await;
    let (status, headers, _) = send(&h.router, Request::get("/").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(csp.contains("script-src 'self' 'sha256-"), "{csp}");
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"), "{csp}");
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("referrer-policy").unwrap(), "no-referrer");

    // The SPA fallback (a share page is one) is covered the same way.
    let (_, headers, _) = send(
        &h.router,
        Request::get("/share/phs_abc").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");

    // Authenticated JSON is never cached; wire JSON keeps its own caching.
    let (_, headers, _) = send(
        &h.router,
        Request::get("/api/auth/me")
            .header(header::COOKIE, &h.cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");

    // HSTS only when the proxy says TLS.
    let (_, headers, _) = send(
        &h.router,
        Request::get("/health")
            .header("x-forwarded-proto", "https")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(headers.contains_key("strict-transport-security"));
    h.finish().await;
}

#[tokio::test]
async fn static_assets_refuse_traversal_and_unknown_paths() {
    let h = start().await;
    for uri in [
        "/assets/../index.html",
        "/assets/%2e%2e/index.html",
        "/assets/..%5cindex.html",
        "/assets/nope.js",
    ] {
        let (status, _, body) = send(&h.router, Request::get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(!String::from_utf8_lossy(&body).contains("<html"), "{uri}");
    }
    h.finish().await;
}

#[tokio::test]
async fn request_ids_are_sanitized_before_being_echoed() {
    let h = start().await;
    let (_, _, body) = send(
        &h.router,
        Request::get(format!("/api/projects/{}/status", h.project_id))
            .header("x-request-id", "<script>alert(1)</script>")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = String::from_utf8_lossy(&body);
    assert!(!body.contains("<script>"), "{body}");
    h.finish().await;
}

// --- Metrics ---------------------------------------------------------------

#[tokio::test]
async fn metrics_token_is_enforced_when_set_and_optional_otherwise() {
    let open = start().await;
    let (status, _, _) = send(&open.router, Request::get("/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    open.finish().await;

    let guarded = start_with(|config| config.security.metrics_token = Some("scrape-me".into())).await;
    let get = |auth: Option<&str>| {
        let mut builder = Request::get("/metrics");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, auth);
        }
        builder.body(Body::empty()).unwrap()
    };
    assert_eq!(send(&guarded.router, get(None)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        send(&guarded.router, get(Some("Bearer nope"))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&guarded.router, get(Some("Bearer scrape-me"))).await.0,
        StatusCode::OK
    );
    guarded.finish().await;
}

// --- Login throttling ------------------------------------------------------

#[tokio::test]
async fn repeated_wrong_passwords_are_throttled_per_email() {
    let h = start().await;
    let attempt = |email: &str, password: &str| {
        Request::builder()
            .method(Method::POST)
            .uri("/api/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"email": email, "password": password}).to_string()))
            .unwrap()
    };
    for _ in 0..5 {
        let (status, _, _) = send(&h.router, attempt("owner@example.com", "wrong password here")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, headers, body) =
        send(&h.router, attempt("OWNER@example.com", "wrong password here")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{}", String::from_utf8_lossy(&body));
    assert!(headers.contains_key(header::RETRY_AFTER));
    // Another account name is not locked out by this one.
    let (status, _, _) = send(&h.router, attempt("someone@example.com", "wrong password here")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn login_hashing_does_not_stall_capture_authorization() {
    let h = start().await;
    let access = h.application.access();
    // A burst of password verifications for a real account (each Argon2).
    let logins: Vec<_> = (0..16)
        .map(|_| {
            let access = access.clone();
            tokio::spawn(async move {
                access
                    .login(hoglet::control::LoginRequest {
                        email: "owner@example.com".into(),
                        password: "wrong password here".into(),
                    })
                    .await
            })
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    // An unknown token forces a control-database lookup, as a flood of
    // anonymous capture requests would.
    assert!(access.authorize_capture("phc_not_cached").await.is_err());
    assert!(
        started.elapsed() < std::time::Duration::from_millis(300),
        "control database answered after {:?}: it is queued behind password hashing",
        started.elapsed()
    );
    for login in logins {
        let result = login.await.unwrap();
        assert!(matches!(
            result,
            Err(hoglet::control::AccessError::InvalidCredentials
                | hoglet::control::AccessError::Unavailable)
        ));
    }
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_known_emails_cost_the_same_to_reject() {
    let h = start().await;
    let access = h.application.access();
    let time = |email: &'static str| {
        let access = access.clone();
        async move {
            let started = std::time::Instant::now();
            let result = access
                .login(hoglet::control::LoginRequest {
                    email: email.into(),
                    password: "wrong password here".into(),
                })
                .await;
            assert!(matches!(
                result,
                Err(hoglet::control::AccessError::InvalidCredentials)
            ));
            started.elapsed()
        }
    };
    // Warm the timing-equalizer hash, then compare.
    time("nobody@example.com").await;
    let unknown = time("nobody@example.com").await;
    let known = time("owner@example.com").await;
    assert!(
        unknown * 3 > known && known * 3 > unknown,
        "unknown {unknown:?} vs known {known:?} reveals which emails have accounts"
    );
    h.finish().await;
}

// --- SSRF policy -----------------------------------------------------------

#[tokio::test]
async fn forwarding_refuses_internal_hosts() {
    let h = start().await;
    let uri = format!("/api/projects/{}/forwarding", h.project_id);
    let put = |host: &str| {
        Request::builder()
            .method(Method::PUT)
            .uri(&uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Body::from(
                json!({"enabled": true, "host": host, "posthog_token": "phc_target"}).to_string(),
            ))
            .unwrap()
    };
    for host in [
        "http://169.254.169.254/latest/meta-data",
        "http://127.0.0.1:8000",
        "http://localhost:9000",
        "http://[::1]:9000",
        "http://[::ffff:10.0.0.1]/",
        "http://10.0.0.5",
        "http://192.168.0.10:8080",
        "http://100.100.100.200",
        "http://user:pass@example.com",
        "ftp://example.com",
        "file:///etc/passwd",
        "https://",
        "https://example.com:notaport",
    ] {
        let (status, _, body) = send(&h.router, put(host)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{host}: {}", String::from_utf8_lossy(&body));
    }
    for host in ["https://us.i.posthog.com", "https://eu.i.posthog.com/", "http://8.8.8.8:8000"] {
        let (status, _, body) = send(&h.router, put(host)).await;
        assert_eq!(status, StatusCode::OK, "{host}: {}", String::from_utf8_lossy(&body));
    }
    h.finish().await;
}

#[tokio::test]
async fn forwarding_may_reach_private_hosts_only_when_the_operator_allows_it() {
    let h = start_with(|config| config.security.allow_private_forwarding = true).await;
    let (status, _, _) = send(
        &h.router,
        Request::builder()
            .method(Method::PUT)
            .uri(format!("/api/projects/{}/forwarding", h.project_id))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Body::from(
                json!({"enabled": true, "host": "http://10.0.0.5:8000", "posthog_token": "phc_target"})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    h.finish().await;
}

/// Even a stored internal host (or a DNS name that resolves inside) is
/// refused when the forwarder connects, not just when the form is submitted.
#[tokio::test(flavor = "multi_thread")]
async fn forwarder_never_connects_to_loopback() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(false).unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = hits.clone();
    std::thread::spawn(move || {
        while let Ok((_stream, _)) = listener.accept() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let control = dir.path().join("control.db");
    hoglet::storage_bootstrap::bootstrap_storage(&control, dir.path().join("projections.db")).unwrap();
    let forwarder = hoglet::forward::Forwarder::open(&control).unwrap();
    forwarder
        .set_config(
            "project-1",
            hoglet::forward::ForwardingConfig {
                enabled: true,
                host: format!("http://127.0.0.1:{port}"),
                posthog_token: "phc_target".into(),
            },
        )
        .unwrap();
    let event = hoglet::capture::event::CapturedEvent {
        uuid: uuid::Uuid::now_v7(),
        event: "x".into(),
        distinct_id: "u".into(),
        token: "phc_t".into(),
        timestamp: chrono::Utc::now(),
        properties: Default::default(),
    };
    forwarder.offer("project-1", &[event]);
    for _ in 0..100 {
        if forwarder.status("project-1").failed > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let status = forwarder.status("project-1");
    assert_eq!(status.failed, 1, "{status:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the forwarder connected to loopback");
    forwarder.stop();
}

// --- Capture amplification -------------------------------------------------

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[tokio::test]
async fn decompression_bombs_are_refused_before_parsing() {
    let h = start().await;
    // 17 MiB of JSON squeezed into a few KiB, past the browser-endpoint ceiling.
    let mut bomb = Vec::with_capacity(17 * 1024 * 1024);
    bomb.push(b'[');
    while bomb.len() < 17 * 1024 * 1024 {
        bomb.extend_from_slice(b"{},");
    }
    bomb.extend_from_slice(b"{}]");
    let compressed = gzip(&bomb);
    assert!(compressed.len() < 256 * 1024, "{}", compressed.len());
    let (status, _, _) = send(
        &h.router,
        Request::post("/e/").body(Body::from(compressed.clone())).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // A legitimate compressed batch still works.
    let batch = json!([{"event": "ok", "distinct_id": "u", "token": h.token}]).to_string();
    let (status, _, _) = send(
        &h.router,
        Request::post("/e/?compression=gzip-js")
            .body(Body::from(gzip(batch.as_bytes())))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    h.finish().await;
}

#[tokio::test]
async fn flags_endpoint_bounds_decoded_size() {
    let h = start().await;
    let mut bomb = br#"{"token":"x","distinct_id":"u","pad":""#.to_vec();
    bomb.resize(8 * 1024 * 1024, b'a');
    bomb.extend_from_slice(b"\"}");
    let (status, _, _) = send(
        &h.router,
        Request::post("/flags/?v=2&compression=gzip-js")
            .body(Body::from(gzip(&bomb)))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    h.finish().await;
}

// --- Shares ----------------------------------------------------------------

#[tokio::test]
async fn public_share_hides_sql_text_and_internal_ids() {
    let h = start().await;
    let (status, _, body) = send(
        &h.router,
        Request::post(h.insights_uri())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Harness::insight_body())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let insight = json_body(&body);
    let (status, _, body) = send(
        &h.router,
        Request::post(format!("/api/projects/{}/shares", h.project_id))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Body::from(
                json!({"object_type": "insight", "object_id": insight["id"]}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = json_body(&body)["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("phs_") && token.len() >= 36);

    let (status, _, body) = send(
        &h.router,
        Request::get(format!("/api/shares/{token}")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("secret_text"), "SQL text leaked: {text}");
    assert!(!text.contains(&h.project_id), "project id leaked: {text}");
    let shared = json_body(&body);
    assert_eq!(shared["insight"]["name"], "SQL");
    assert_eq!(shared["insight"]["created_by"], "");

    // An unknown or expired token is a plain 404.
    let (status, _, _) = send(
        &h.router,
        Request::get("/api/shares/phs_00000000000000000000000000000000")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.finish().await;
}

// --- Tenant isolation ------------------------------------------------------

/// Add a second user with a project of their own, bypassing the API (which
/// deliberately has no way to create a second account).
fn add_other_tenant(control: &std::path::Path) -> (String, String) {
    let connection = rusqlite::Connection::open(control).unwrap();
    let project = uuid::Uuid::new_v4().to_string();
    connection
        .execute_batch(&format!(
            "INSERT INTO users(id,email,password_hash,name,created_at) VALUES ('u2','other@example.com','x','o',1);
             INSERT INTO organizations(id,name,created_at) VALUES ('o2','Other',1);
             INSERT INTO organization_members(organization_id,user_id,role) VALUES ('o2','u2','owner');
             INSERT INTO projects(id,organization_id,name,capture_token,created_at) VALUES ('{project}','o2','Theirs','phc_theirs',1);"
        ))
        .unwrap();
    (project, "phc_theirs".to_owned())
}

#[tokio::test]
async fn one_tenants_session_cannot_touch_another_tenants_project() {
    let h = start().await;
    let (other, _) = add_other_tenant(&h.dir.path().join("control.db"));
    let mine = h.project_id.clone();
    let cookie = h.cookie.clone();

    let paths: Vec<(Method, String, Value)> = vec![
        (Method::GET, format!("/api/projects/{other}/insights"), Value::Null),
        (Method::POST, format!("/api/projects/{other}/insights"), json!({"name": "x", "query_ir": {"kind": "SqlQuery", "query": "select 1"}})),
        (Method::GET, format!("/api/projects/{other}/dashboards"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/shares"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/feature_flags"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/persons"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/events"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/catalog/events"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/status"), Value::Null),
        (Method::GET, format!("/api/projects/{other}/forwarding"), Value::Null),
        (Method::PUT, format!("/api/projects/{other}/forwarding"), json!({"enabled": false, "host": "", "posthog_token": ""})),
        (Method::POST, format!("/api/projects/{other}/demo"), Value::Null),
        (Method::POST, format!("/api/projects/{other}/persons/u/erase"), Value::Null),
        (Method::POST, format!("/api/projects/{other}/query"), json!({"query": {"kind": "SqlQuery", "query": "select 1"}})),
        (Method::POST, "/api/organizations/o2/projects".to_owned(), json!({"name": "mine now"})),
    ];
    for (method, uri, body) in paths {
        let builder = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie);
        let request = builder
            .body(if body.is_null() { Body::empty() } else { Body::from(body.to_string()) })
            .unwrap();
        let (status, _, _) = send(&h.router, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }

    // An object id from another project, addressed through my own project, is
    // simply absent.
    let connection = rusqlite::Connection::open(h.dir.path().join("control.db")).unwrap();
    connection
        .execute(
            "INSERT INTO saved_insights(project_id,id,name,description,query_ir,query_supported,created_by,created_at,updated_at)
             VALUES (?1,'foreign-insight','Theirs','','{\"kind\":\"SqlQuery\",\"query\":\"select 1\"}',1,'u2',1,1)",
            [&other],
        )
        .unwrap();
    for method in [Method::GET, Method::DELETE] {
        let (status, _, _) = send(
            &h.router,
            Request::builder()
                .method(method.clone())
                .uri(format!("/api/projects/{mine}/insights/foreign-insight"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    h.finish().await;
}

#[tokio::test]
async fn read_scoped_keys_cannot_mutate_anything() {
    let h = start().await;
    let (status, _, body) = send(
        &h.router,
        Request::post("/api/auth/keys")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Body::from(json!({"name": "ci", "scope": "read"}).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = json_body(&body)["secret"].as_str().unwrap().to_owned();
    let pid = &h.project_id;
    let attempts: Vec<(Method, String, Value)> = vec![
        (Method::POST, format!("/api/projects/{pid}/insights"), json!({"name": "x", "query_ir": {"kind": "SqlQuery", "query": "select 1"}})),
        (Method::POST, format!("/api/projects/{pid}/dashboards"), json!({"name": "x"})),
        (Method::POST, format!("/api/projects/{pid}/shares"), json!({"object_type": "insight", "object_id": "x"})),
        (Method::POST, format!("/api/projects/{pid}/feature_flags"), json!({"key": "k"})),
        (Method::PUT, format!("/api/projects/{pid}/forwarding"), json!({"enabled": false, "host": "", "posthog_token": ""})),
        (Method::POST, format!("/api/projects/{pid}/demo"), Value::Null),
        (Method::POST, format!("/api/projects/{pid}/persons/u/erase"), Value::Null),
        (Method::POST, "/api/auth/keys".to_owned(), json!({"name": "escalate", "scope": "write"})),
        (Method::POST, "/api/organizations".to_owned(), json!({"name": "o"})),
    ];
    for (method, uri, body) in attempts {
        let request = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {secret}"))
            .body(if body.is_null() { Body::empty() } else { Body::from(body.to_string()) })
            .unwrap();
        let (status, _, _) = send(&h.router, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    // Reading is allowed.
    let (status, _, _) = send(
        &h.router,
        Request::get(format!("/api/projects/{pid}/insights"))
            .header(header::AUTHORIZATION, format!("Bearer {secret}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    h.finish().await;
}

#[tokio::test]
async fn flags_never_cross_projects() {
    let h = start().await;
    let (_, theirs) = add_other_tenant(&h.dir.path().join("control.db"));
    // A flag exists in my project...
    let (status, _, body) = send(
        &h.router,
        Request::post(format!("/api/projects/{}/feature_flags", h.project_id))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Body::from(
                json!({"key": "my-private-flag", "filters": {"groups": [{"properties": [], "rollout_percentage": 100}]}})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    // ...and is invisible to the other project's public token, and vice versa.
    let evaluate = |token: &str| {
        Request::post("/flags/?v=2")
            .body(Body::from(json!({"token": token, "distinct_id": "u"}).to_string()))
            .unwrap()
    };
    let (status, _, body) = send(&h.router, evaluate(&theirs)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains("my-private-flag"));
    let (_, _, body) = send(&h.router, evaluate(&h.token)).await;
    assert!(String::from_utf8_lossy(&body).contains("my-private-flag"));
    // The token is not a credential for the dashboard API.
    let (status, _, _) = send(
        &h.router,
        Request::get(format!("/api/projects/{}/feature_flags", h.project_id))
            .header(header::AUTHORIZATION, format!("Bearer {}", h.token))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    h.finish().await;
}

// --- SQL sandbox -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn sql_tab_cannot_read_files_or_change_settings() {
    let h = start().await;
    let run = |sql: &str| {
        Request::post(format!("/api/projects/{}/query", h.project_id))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &h.cookie)
            .body(Body::from(
                json!({"query": {"kind": "SqlQuery", "query": sql}}).to_string(),
            ))
            .unwrap()
    };
    for sql in [
        "select * from read_text('/etc/passwd')",
        "select * from read_csv('/etc/passwd')",
        "select * from read_parquet('/etc/*')",
        "select * from glob('/*')",
        "select * from read_json('/etc/passwd')",
        "copy (select 1) to '/tmp/hoglet-sql-escape.csv'",
        "attach '/tmp/hoglet-sql-escape.db'",
        "install httpfs",
        "load httpfs",
        "set enable_external_access = true",
        "pragma enable_external_access",
        "create table t as select 1",
        "select 1; select 2",
    ] {
        let (status, _, body) = send(&h.router, run(sql)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{sql}: {}", String::from_utf8_lossy(&body));
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("root:"), "{sql}: {text}");
    }
    assert!(!std::path::Path::new("/tmp/hoglet-sql-escape.csv").exists());
    assert!(!std::path::Path::new("/tmp/hoglet-sql-escape.db").exists());
    // Ordinary SQL still works.
    let (status, _, _) = send(&h.router, run("select count(*) from events")).await;
    assert_eq!(status, StatusCode::OK);
    h.finish().await;
}

// --- Files -----------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn a_loosely_created_data_directory_is_restricted_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let application = Application::prepare(ApplicationConfig::new(dir.path())).await.unwrap();
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.path()), 0o700);
    assert_eq!(mode(&dir.path().join("control.db")), 0o600);
    application.shutdown().await.unwrap();
}

// --- Slow clients ----------------------------------------------------------

#[tokio::test]
async fn slow_header_clients_are_disconnected() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let router = Router::new().route("/ok", axum::routing::get(|| async { "ok" }));
    let server = tokio::spawn(hoglet::server::serve_with(
        listener,
        router,
        async move {
            let _ = stopped.await;
        },
        std::time::Duration::from_millis(300),
        std::time::Duration::from_secs(5),
        64,
    ));

    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    slow.write_all(b"GET /ok HTTP/1.1\r\nHost: x\r\nX-Slow: ").await.unwrap();
    let mut buffer = [0_u8; 256];
    let read = tokio::time::timeout(std::time::Duration::from_secs(3), slow.read(&mut buffer))
        .await
        .expect("a half-sent header must not hold the connection open");
    // Closed (0 bytes) or a 408; never left hanging.
    assert!(matches!(read, Ok(0) | Ok(_) | Err(_)));

    // A well-behaved client is still served.
    let mut good = tokio::net::TcpStream::connect(address).await.unwrap();
    good.write_all(b"GET /ok HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    good.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let _ = stop.send(());
    server.await.unwrap().unwrap();
}
