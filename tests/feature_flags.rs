//! Feature flags end to end through the production `Application`: the
//! dashboard API, `/flags` + `/decide` wire shapes, local evaluation, and
//! person-backed evaluation.

use std::io::Write as _;
use std::path::Path;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use base64::Engine as _;
use chrono::Utc;
use hoglet::application::{Application, ApplicationConfig};
use hoglet::flags::eval::hash;
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Harness {
    directory: tempfile::TempDir,
    application: Application,
    project_id: String,
    token: String,
    cookie: String,
    personal_key: String,
}

impl Harness {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let application = Application::prepare(ApplicationConfig::new(directory.path()))
            .await
            .expect("application starts");
        let router = application.router();
        let (status, headers, workspace) = call(
            &router,
            "POST",
            "/api/auth/setup",
            &[],
            json!({
                "email": "owner@example.com",
                "password": "correct horse battery staple",
                "organization_name": "Acme",
                "project_name": "Web",
            })
            .to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{workspace}");
        let cookie = headers
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .expect("session cookie")
            .to_owned();
        let project = &workspace["organizations"][0]["projects"][0];
        let project_id = project["id"].as_str().unwrap().to_owned();
        let token = project["token"].as_str().unwrap().to_owned();
        let (status, _, key) = call(
            &router,
            "POST",
            "/api/auth/keys",
            &[(header::COOKIE.as_str(), &cookie)],
            json!({"name": "server sdk"}).to_string(),
        )
        .await;
        assert!(status.is_success(), "{key}");
        let personal_key = key["secret"].as_str().unwrap().to_owned();
        Self {
            directory,
            application,
            project_id,
            token,
            cookie,
            personal_key,
        }
    }

    fn router(&self) -> Router {
        self.application.router()
    }

    fn flags_uri(&self) -> String {
        format!("/api/projects/{}/feature_flags", self.project_id)
    }

    async fn create(&self, body: Value) -> Value {
        let (status, _, flag) = call(
            &self.router(),
            "POST",
            &self.flags_uri(),
            &[(header::COOKIE.as_str(), &self.cookie)],
            body.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{flag}");
        flag
    }

    async fn flags(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        let (status, _, response) = call(&self.router(), "POST", uri, &[], body.to_string()).await;
        (status, response)
    }

    async fn shutdown(self) {
        self.application.shutdown().await.expect("clean shutdown");
        drop(self.directory);
    }
}

async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: impl Into<Body>,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, value)
}

fn seed_person(
    data_dir: &Path,
    project_id: &str,
    distinct_ids: &[&str],
    first_seen: &str,
    properties: Value,
) {
    let connection = Connection::open(data_dir.join("projections.db")).unwrap();
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let person_id = distinct_ids[0];
    connection
        .execute(
            "INSERT INTO persons(project_id,id,created_at,is_identified,properties,first_seen_key)
             VALUES (?1,?2,?3,1,?4,?5)",
            rusqlite::params![
                project_id,
                person_id,
                Utc::now().to_rfc3339(),
                properties.to_string(),
                first_seen
            ],
        )
        .unwrap();
    for distinct_id in distinct_ids {
        connection
            .execute(
                "INSERT INTO distinct_ids(project_id,distinct_id,person_id,seq) VALUES (?1,?2,?3,0)",
                rusqlite::params![project_id, distinct_id, person_id],
            )
            .unwrap();
    }
}

#[tokio::test]
async fn sdk_shapes_return_values_variants_and_payloads() {
    let harness = Harness::start().await;
    harness
        .create(json!({
            "key": "everyone",
            "filters": {"groups": [{"rollout_percentage": 100}], "payloads": {"true": {"theme": "dark"}}}
        }))
        .await;
    harness
        .create(json!({
            "key": "experiment",
            "filters": {
                "groups": [{"rollout_percentage": 100}],
                "multivariate": {"variants": [
                    {"key": "control", "rollout_percentage": 0},
                    {"key": "test", "rollout_percentage": 100}]},
                "payloads": {"test": "plain text"}
            }
        }))
        .await;
    harness
        .create(json!({
            "key": "pros",
            "filters": {"groups": [{"properties": [
                {"key": "plan", "type": "person", "operator": "exact", "value": ["pro", "team"]}]}]}
        }))
        .await;
    harness
        .create(json!({"key": "paused", "active": false, "filters": {"groups": [{}]}}))
        .await;
    let token = harness.token.clone();
    let request =
        json!({"token": token, "distinct_id": "user-1", "person_properties": {"plan": "PRO"}});

    // posthog-node / posthog-js: /flags/?v=2 → `flags` details.
    let (status, body) = harness.flags("/flags/?v=2", request.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["flags"]["everyone"]["enabled"], true);
    assert_eq!(body["flags"]["everyone"]["variant"], Value::Null);
    assert_eq!(
        body["flags"]["everyone"]["reason"]["code"],
        "condition_match"
    );
    assert_eq!(
        body["flags"]["everyone"]["metadata"]["payload"],
        r#"{"theme":"dark"}"#
    );
    assert_eq!(body["flags"]["experiment"]["variant"], "test");
    assert_eq!(
        body["flags"]["experiment"]["metadata"]["payload"],
        "plain text"
    );
    assert_eq!(body["flags"]["pros"]["enabled"], true);
    assert!(
        body["flags"].get("paused").is_none(),
        "inactive flags are not served"
    );
    assert_eq!(body["errorsWhileComputingFlags"], false);
    assert_eq!(body["sessionRecording"], false);

    // /flags without v and /decide?v=3 → featureFlags + featureFlagPayloads.
    for uri in ["/flags", "/flags/?v=1", "/decide/?v=3"] {
        let (status, body) = harness.flags(uri, request.clone()).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(
            body["featureFlags"],
            json!({"everyone": true, "experiment": "test", "pros": true}),
            "{uri}"
        );
        assert_eq!(
            body["featureFlagPayloads"],
            json!({"everyone": r#"{"theme":"dark"}"#, "experiment": "plain text"}),
            "{uri}"
        );
    }
    let (_, body) = harness.flags("/decide/?v=4", request.clone()).await;
    assert_eq!(body["flags"]["experiment"]["variant"], "test");
    let (_, body) = harness.flags("/decide/?v=2", request.clone()).await;
    assert_eq!(body["featureFlags"]["pros"], true);
    assert!(body.get("featureFlagPayloads").is_none());
    let (_, body) = harness.flags("/decide/?v=1", request.clone()).await;
    let mut enabled: Vec<&str> = body["featureFlags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    enabled.sort_unstable();
    assert_eq!(enabled, ["everyone", "experiment", "pros"]);

    // Property gating uses request person_properties.
    let (_, body) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": token, "distinct_id": "user-2", "person_properties": {"plan": "free"}}),
        )
        .await;
    assert_eq!(body["flags"]["pros"]["enabled"], false);
    assert_eq!(
        body["flags"]["pros"]["reason"]["code"],
        "no_condition_match"
    );

    // flag_keys_to_evaluate (posthog-node) narrows the response.
    let (_, body) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": token, "distinct_id": "u", "flag_keys_to_evaluate": ["everyone"]}),
        )
        .await;
    assert_eq!(body["flags"].as_object().unwrap().len(), 1);

    harness.shutdown().await;
}

#[tokio::test]
async fn wire_bodies_decode_and_errors_are_not_retryable() {
    let harness = Harness::start().await;
    harness
        .create(json!({"key": "on", "filters": {"groups": [{"rollout_percentage": 100}]}}))
        .await;
    let router = harness.router();
    let body = json!({"token": harness.token, "distinct_id": "u"}).to_string();

    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(body.as_bytes()).unwrap();
    let (status, _, response) = call(
        &router,
        "POST",
        "/flags/?v=2&compression=gzip-js",
        &[],
        gzip.finish().unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["flags"]["on"]["enabled"], true);

    let encoded = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
    let form = format!(
        "data={}",
        encoded
            .replace('+', "%2B")
            .replace('=', "%3D")
            .replace('/', "%2F")
    );
    let request = Request::post("/decide/?v=3")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // json5 (NaN) is tolerated.
    let json5 = format!(
        r#"{{"token": "{}", "distinct_id": "u", "person_properties": {{"x": NaN}}}}"#,
        harness.token
    );
    let (status, _, _) = call(&router, "POST", "/flags/?v=2", &[], json5).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": "phc_unknown", "distinct_id": "u"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = harness
        .flags("/flags/?v=2", json!({"distinct_id": "u"}))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = harness
        .flags("/flags/?v=2", json!({"token": harness.token}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&router, "POST", "/flags/?v=2", &[], "not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let deep = format!(
        r#"{{"token": "{}", "distinct_id": "u", "x": {}{}}}"#,
        harness.token,
        "[".repeat(500),
        "]".repeat(500)
    );
    let (status, _, _) = call(&router, "POST", "/flags/?v=2", &[], deep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    harness.shutdown().await;
}

#[tokio::test]
async fn dashboard_api_validates_versions_and_soft_deletes() {
    let harness = Harness::start().await;
    let router = harness.router();
    let cookie = [(header::COOKIE.as_str(), harness.cookie.as_str())];
    let uri = harness.flags_uri();

    for (body, field) in [
        (json!({"key": "has space"}), "key"),
        (
            json!({"key": "v", "filters": {"multivariate": {"variants": [{"key": "a", "rollout_percentage": 60}]}}}),
            "multivariate",
        ),
        (
            json!({"key": "r", "filters": {"groups": [{"rollout_percentage": 150}]}}),
            "rollout_percentage",
        ),
    ] {
        let (status, _, error) = call(&router, "POST", &uri, &cookie, body.to_string()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(error["error"]["code"], "invalid_flag");
        assert!(
            error["error"]["message"].as_str().unwrap().contains(field),
            "{error}"
        );
        assert!(error["error"]["request_id"].is_string());
    }
    let (status, _, _) = call(&router, "POST", &uri, &cookie, "{").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let flag = harness
        .create(json!({"key": "beta", "name": "Beta", "filters": {"groups": [{"rollout_percentage": 0}]}}))
        .await;
    let id = flag["id"].as_i64().unwrap();
    assert_eq!(flag["active"], true);
    assert_eq!(flag["ensure_experience_continuity"], false);
    let (status, _, _) = call(
        &router,
        "POST",
        &uri,
        &cookie,
        json!({"key": "beta"}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let flag_uri = format!("{uri}/{id}");
    let (status, _, fetched) = call(&router, "GET", &flag_uri, &cookie, "").await;
    assert_eq!(
        (status, fetched["key"].clone()),
        (StatusCode::OK, json!("beta"))
    );
    let (status, _, list) = call(&router, "GET", &uri, &cookie, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    let evaluate = format!("{flag_uri}/evaluate?distinct_id=someone");
    let (status, _, evaluation) = call(&router, "GET", &evaluate, &cookie, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(evaluation["enabled"], false);
    assert_eq!(evaluation["reason"], "out_of_rollout_bound");
    assert_eq!(evaluation["condition_index"], 0);

    let (_, before) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": "someone"}),
        )
        .await;
    assert_eq!(before["flags"]["beta"]["metadata"]["version"], 1);

    let (status, _, patched) = call(
        &router,
        "PATCH",
        &flag_uri,
        &cookie,
        json!({"filters": {"groups": [{"rollout_percentage": 100}], "payloads": {"true": 42}}})
            .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["name"], "Beta", "absent fields are kept");
    let (_, _, evaluation) = call(&router, "GET", &evaluate, &cookie, "").await;
    assert_eq!(evaluation["enabled"], true);
    assert_eq!(evaluation["reason"], "condition_match");
    assert_eq!(evaluation["payload"], 42);
    // The wire sees the edit immediately, with a bumped version.
    let (_, after) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": "someone"}),
        )
        .await;
    assert_eq!(after["flags"]["beta"]["enabled"], true);
    assert_eq!(after["flags"]["beta"]["metadata"]["version"], 2);
    assert_eq!(after["flags"]["beta"]["metadata"]["description"], "Beta");

    let (status, _, error) = call(
        &router,
        "PATCH",
        &flag_uri,
        &cookie,
        json!({"filters": {"groups": [{"variant": "nope"}]}}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");

    // Personal keys read but never mutate.
    let bearer = format!("Bearer {}", harness.personal_key);
    let key_auth = [(header::AUTHORIZATION.as_str(), bearer.as_str())];
    let (status, _, _) = call(&router, "GET", &flag_uri, &key_auth, "").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&router, "DELETE", &flag_uri, &key_auth, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&router, "GET", &flag_uri, &[], "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = call(&router, "DELETE", &flag_uri, &cookie, "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = call(&router, "GET", &flag_uri, &cookie, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, after) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": "someone"}),
        )
        .await;
    assert!(after["flags"].get("beta").is_none());
    // Soft delete frees the key.
    let again = harness.create(json!({"key": "beta"})).await;
    assert_ne!(again["id"], flag["id"]);

    let (status, _, _) = call(&router, "GET", &format!("{uri}/not-a-number"), &cookie, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    harness.shutdown().await;
}

#[tokio::test]
async fn local_evaluation_requires_a_personal_key_with_project_access() {
    let harness = Harness::start().await;
    harness
        .create(json!({
            "key": "exp",
            "filters": {
                "groups": [{"properties": [{"key": "email", "operator": "icontains", "value": "@acme.com"}], "rollout_percentage": 50}],
                "multivariate": {"variants": [{"key": "a", "rollout_percentage": 50}, {"key": "b", "rollout_percentage": 50}]},
                "payloads": {"a": {"x": 1}}
            }
        }))
        .await;
    harness
        .create(json!({"key": "off", "active": false, "filters": {"groups": [{}]}}))
        .await;
    let router = harness.router();
    let bearer = format!("Bearer {}", harness.personal_key);
    let uri = format!("/flags/definitions?token={}&send_cohorts", harness.token);

    let (status, headers, body) = call(
        &router,
        "GET",
        &uri,
        &[(header::AUTHORIZATION.as_str(), &bearer)],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["group_type_mapping"], json!({}));
    assert_eq!(body["cohorts"], json!({}));
    let flags = body["flags"].as_array().unwrap();
    assert_eq!(
        flags.len(),
        2,
        "inactive flags are included with active=false"
    );
    let exp = flags.iter().find(|flag| flag["key"] == "exp").unwrap();
    assert_eq!(exp["active"], true);
    assert_eq!(
        exp["filters"]["groups"][0]["properties"][0]["type"],
        "person"
    );
    assert_eq!(
        exp["filters"]["groups"][0]["properties"][0]["operator"],
        "icontains"
    );
    assert_eq!(exp["filters"]["multivariate"]["variants"][1]["key"], "b");
    assert_eq!(exp["filters"]["payloads"]["a"], r#"{"x":1}"#);
    let off = flags.iter().find(|flag| flag["key"] == "off").unwrap();
    assert_eq!(off["active"], false);

    let etag = headers
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (status, _, _) = call(
        &router,
        "GET",
        &uri,
        &[
            (header::AUTHORIZATION.as_str(), &bearer),
            (header::IF_NONE_MATCH.as_str(), &etag),
        ],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    let legacy = format!(
        "/api/feature_flag/local_evaluation/?token={}&send_cohorts",
        harness.token
    );
    let (status, _, _) = call(
        &router,
        "GET",
        &legacy,
        &[(header::AUTHORIZATION.as_str(), &bearer)],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = call(&router, "GET", &uri, &[], "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let wrong_key = format!("Bearer phx_{}", Uuid::new_v4().simple());
    let (status, _, _) = call(
        &router,
        "GET",
        &uri,
        &[(header::AUTHORIZATION.as_str(), &wrong_key)],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(
        &router,
        "GET",
        "/flags/definitions?token=phc_nope",
        &[(header::AUTHORIZATION.as_str(), &bearer)],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A project token is never a personal key.
    let token_bearer = format!("Bearer {}", harness.token);
    let (status, _, _) = call(
        &router,
        "GET",
        &uri,
        &[(header::AUTHORIZATION.as_str(), &token_bearer)],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A valid key whose user cannot access the project → 403.
    let other_session = seed_unrelated_owner(&harness.directory.path().join("control.db"));
    let (status, _, other_key) = call(
        &router,
        "POST",
        "/api/auth/keys",
        &[(
            header::COOKIE.as_str(),
            &format!("hoglet_sid={other_session}"),
        )],
        json!({"name": "other"}).to_string(),
    )
    .await;
    assert!(status.is_success(), "{other_key}");
    let other_bearer = format!("Bearer {}", other_key["secret"].as_str().unwrap());
    let (status, _, _) = call(
        &router,
        "GET",
        &uri,
        &[(header::AUTHORIZATION.as_str(), &other_bearer)],
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    harness.shutdown().await;
}

#[tokio::test]
async fn stored_persons_drive_properties_and_experience_continuity() {
    let harness = Harness::start().await;
    let rollout_key = "continuity";
    harness
        .create(json!({
            "key": rollout_key,
            "ensure_experience_continuity": true,
            "filters": {"groups": [{"rollout_percentage": 50}]}
        }))
        .await;
    harness
        .create(json!({
            "key": "stored-plan",
            "filters": {"groups": [{"properties": [{"key": "plan", "type": "person", "value": "enterprise"}]}]}
        }))
        .await;
    // Find an anonymous id inside the rollout whose identified id is outside.
    let inside = |id: &str| hash(rollout_key, id, "") <= 0.5;
    let (anon, user) = (0..1000)
        .map(|index| (format!("anon-{index}"), format!("user-{index}")))
        .find(|(anon, user)| inside(anon) && !inside(user))
        .expect("a split pair exists");

    // Unknown person: bucketed by distinct id (out), or by $anon_distinct_id
    // when posthog-js sends it before the merge is published (in).
    let (_, body) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": user}),
        )
        .await;
    assert_eq!(body["flags"][rollout_key]["enabled"], false);
    let (_, body) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": user, "$anon_distinct_id": anon}),
        )
        .await;
    assert_eq!(body["flags"][rollout_key]["enabled"], true);

    // After the merge: the person's first-seen key (the anon id) buckets.
    seed_person(
        harness.directory.path(),
        &harness.project_id,
        &[&user, &anon],
        &anon,
        json!({"plan": "Enterprise"}),
    );
    let (_, body) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": user}),
        )
        .await;
    assert_eq!(body["flags"][rollout_key]["enabled"], true);
    assert_eq!(body["flags"]["stored-plan"]["enabled"], true);
    // Request person_properties override stored ones.
    let (_, body) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": user, "person_properties": {"plan": "free"}}),
        )
        .await;
    assert_eq!(body["flags"]["stored-plan"]["enabled"], false);

    let flag_list_uri = harness.flags_uri();
    let (_, _, list) = call(
        &harness.router(),
        "GET",
        &flag_list_uri,
        &[(header::COOKIE.as_str(), &harness.cookie)],
        "",
    )
    .await;
    let stored = list
        .as_array()
        .unwrap()
        .iter()
        .find(|flag| flag["key"] == "stored-plan")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (_, _, evaluation) = call(
        &harness.router(),
        "GET",
        &format!("{flag_list_uri}/{stored}/evaluate?distinct_id={user}"),
        &[(header::COOKIE.as_str(), &harness.cookie)],
        "",
    )
    .await;
    assert_eq!(evaluation["enabled"], true, "{evaluation}");

    harness.shutdown().await;
}

#[tokio::test]
async fn flags_and_ids_never_cross_projects() {
    let harness = Harness::start().await;
    let router = harness.router();
    let cookie = [(header::COOKIE.as_str(), harness.cookie.as_str())];
    let flag = harness
        .create(json!({"key": "checkout", "filters": {"groups": [{"rollout_percentage": 100}]}}))
        .await;
    let id = flag["id"].as_i64().unwrap();

    let (_, _, me) = call(&router, "GET", "/api/auth/me", &cookie, "").await;
    let organization_id = me["organizations"][0]["id"].as_str().unwrap();
    let (status, _, second) = call(
        &router,
        "POST",
        &format!("/api/organizations/{organization_id}/projects"),
        &cookie,
        json!({"name": "Second"}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let second_id = second["id"].as_str().unwrap();
    let second_token = second["token"].as_str().unwrap();
    let second_uri = format!("/api/projects/{second_id}/feature_flags");

    // The first project's id is invisible through the second project's path.
    for method in ["GET", "PATCH", "DELETE"] {
        let (status, _, _) = call(
            &router,
            method,
            &format!("{second_uri}/{id}"),
            &cookie,
            "{}",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    let (_, _, list) = call(&router, "GET", &second_uri, &cookie, "").await;
    assert_eq!(list, json!([]));
    // The same key is free in the second project.
    let (status, _, _) = call(
        &router,
        "POST",
        &second_uri,
        &cookie,
        json!({"key": "checkout", "active": false}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // Each token sees only its own project's flags.
    let (_, first) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": harness.token, "distinct_id": "u"}),
        )
        .await;
    assert_eq!(first["flags"]["checkout"]["enabled"], true);
    let (_, second) = harness
        .flags(
            "/flags/?v=2",
            json!({"token": second_token, "distinct_id": "u"}),
        )
        .await;
    assert_eq!(second["flags"], json!({}));
    // The first flag is untouched.
    let (status, _, fetched) = call(
        &router,
        "GET",
        &format!("{}/{id}", harness.flags_uri()),
        &cookie,
        "",
    )
    .await;
    assert_eq!(
        (status, fetched["active"].clone()),
        (StatusCode::OK, json!(true))
    );

    harness.shutdown().await;
}

fn seed_unrelated_owner(control_path: &Path) -> String {
    let user_id = Uuid::now_v7().to_string();
    let organization_id = Uuid::now_v7().to_string();
    let project_id = Uuid::now_v7().to_string();
    let session_id = Uuid::now_v7().to_string();
    let now = Utc::now().timestamp();
    let connection = Connection::open(control_path).unwrap();
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO users(id,email,password_hash,name,created_at)
                 VALUES ('{user_id}','other@example.com','unused','Other',{now});
             INSERT INTO organizations(id,name,created_at) VALUES ('{organization_id}','Other',{now});
             INSERT INTO organization_members(organization_id,user_id,role)
                 VALUES ('{organization_id}','{user_id}','owner');
             INSERT INTO projects(id,organization_id,name,capture_token,created_at)
                 VALUES ('{project_id}','{organization_id}','Other','phc_other',{now});
             INSERT INTO auth_sessions(id,user_id,created_at,expires_at)
                 VALUES ('{session_id}','{user_id}',{now},{});",
            now + 3600
        ))
        .unwrap();
    session_id
}
