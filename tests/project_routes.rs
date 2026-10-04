use std::{path::Path, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use hoglet::{
    capture::event::CapturedEvent,
    control::{ProjectAccess, SetupRequest},
    persons::PersonStore,
    query::{EngineConfig, QueryEngine},
    routes::project,
    source::DirectorySource,
    storage_bootstrap::bootstrap_storage,
};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn json_response(
    app: Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = app.oneshot(request).await.expect("router should respond");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body should be readable")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).expect("response should be JSON");
    (status, headers, body)
}

fn post(uri: &str, credential: Option<(&str, &str)>, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some((name, value)) = credential {
        request = request.header(name, value);
    }
    request
        .body(Body::from(body.to_string()))
        .expect("request should be valid")
}

fn trends_query() -> Value {
    json!({
        "kind": "TrendsQuery",
        "series": [{"event": "pageview", "math": "dau"}],
        "date_range": {"date_from": "2026-01-15", "date_to": "2026-01-15"},
        "interval": "day"
    })
}

fn bootstrap(directory: &Path) -> std::path::PathBuf {
    let control = directory.join("control.db");
    bootstrap_storage(&control, directory.join("projections.db"))
        .expect("storage pair should bootstrap");
    control
}

fn engine(directory: &Path, config: EngineConfig) -> Arc<QueryEngine> {
    // The publication pipeline installs the identity projection at startup.
    let projections = Connection::open(directory.join("projections.db")).expect("projections");
    hoglet::projections::initialize_schema(&projections).expect("identity projection schema");
    drop(projections);
    let persons = Arc::new(
        PersonStore::open(&directory.join("projections.db")).expect("person store should open"),
    );
    Arc::new(
        QueryEngine::new(
            Arc::new(DirectorySource::new(directory.join("lake"))),
            persons,
            config,
        )
        .expect("engine should open"),
    )
}

fn write_events(directory: &Path, project_id: &str, distinct_ids: &[&str]) {
    let source = DirectorySource::new(directory.join("lake"));
    let day = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).expect("test date");
    let partition = source.partition_dir(project_id, day);
    std::fs::create_dir_all(&partition).expect("partition directory");
    let events: Vec<CapturedEvent> = distinct_ids
        .iter()
        .map(|distinct_id| CapturedEvent {
            uuid: Uuid::new_v4(),
            event: "pageview".into(),
            distinct_id: (*distinct_id).into(),
            token: "phc_authorized".into(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
                .single()
                .expect("timestamp should be valid"),
            properties: serde_json::Map::new(),
        })
        .collect();
    hoglet::lake::parquet::write_file(&events, &partition.join("events.parquet"))
        .expect("events should be written");
}

fn seed_second_user(control_path: &Path) -> (String, String, String) {
    let user_id = Uuid::new_v4().to_string();
    let organization_id = Uuid::new_v4().to_string();
    let project_id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();
    let mut connection = Connection::open(control_path).expect("control database should open");
    connection
        .execute_batch("PRAGMA foreign_keys=ON;")
        .expect("foreign keys should enable");
    let transaction = connection
        .transaction()
        .expect("fixture transaction should start");
    transaction
        .execute(
            "INSERT INTO users(id,email,password_hash,name,created_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![
                user_id,
                "second@example.com",
                "unused-in-router-test",
                "Second User",
                now
            ],
        )
        .expect("second user should be inserted");
    transaction
        .execute(
            "INSERT INTO organizations(id,name,created_at) VALUES (?1,?2,?3)",
            rusqlite::params![organization_id, "Second Organization", now],
        )
        .expect("second organization should be inserted");
    transaction
        .execute(
            "INSERT INTO organization_members(organization_id,user_id,role) VALUES (?1,?2,'owner')",
            rusqlite::params![organization_id, user_id],
        )
        .expect("second membership should be inserted");
    transaction
        .execute(
            "INSERT INTO projects(id,organization_id,name,capture_token,created_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![project_id, organization_id, "Second Project", "phc_second", now],
        )
        .expect("second project should be inserted");
    transaction
        .execute(
            "INSERT INTO auth_sessions(id,user_id,created_at,expires_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![session_id, user_id, now, now + 3600],
        )
        .expect("second session should be inserted");
    transaction
        .commit()
        .expect("fixture transaction should commit");
    (user_id, project_id, session_id)
}

async fn setup(access: &ProjectAccess) -> hoglet::control::SetupResult {
    access
        .setup(SetupRequest {
            email: "owner@example.com".into(),
            password: "correct horse battery staple".into(),
            organization_name: "Acme".into(),
            project_name: "Website".into(),
            existing_project_token: Some("phc_authorized".into()),
        })
        .await
        .expect("setup should succeed")
}

#[tokio::test]
async fn session_and_personal_key_query_the_path_project_only() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let control_path = bootstrap(directory.path());
    let (access, runtime) = ProjectAccess::open(control_path).expect("project access should open");
    let setup = setup(&access).await;
    let project_id = setup.workspace.organizations[0].projects[0].id.clone();
    let principal = access
        .validate_session(&setup.session_id)
        .await
        .expect("setup session should authenticate");
    let personal_key = access
        .create_personal_key(&principal, "Test client", hoglet::control::KeyScope::Read)
        .await
        .expect("personal key should be created");
    write_events(directory.path(), &project_id, &["person-1", "person-2"]);
    write_events(
        directory.path(),
        &Uuid::new_v4().to_string(),
        &["intruder-1", "intruder-2", "intruder-3"],
    );

    let app = project::router(
        Arc::new(access.clone()),
        engine(directory.path(), EngineConfig::default()),
    );
    let uri = format!("/api/projects/{project_id}/query");
    let cookie = format!("hoglet_sid={}", setup.session_id);
    let bearer = format!("Bearer {}", personal_key.secret);

    for credential in [
        (header::COOKIE.as_str(), cookie.as_str()),
        (header::AUTHORIZATION.as_str(), bearer.as_str()),
    ] {
        let (status, headers, body) = json_response(
            app.clone(),
            post(
                &uri,
                Some(credential),
                // A client-supplied token never selects data.
                json!({"query": trends_query(), "token": "phc_someone_else"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["meta"]["kind"], "TrendsQuery");
        assert_eq!(body["result"]["kind"], "Trends");
        assert_eq!(body["result"]["series"][0]["data"], json!([2.0]));
        assert!(headers.contains_key("x-request-id"));
    }

    let (status, _, body) = json_response(
        app.clone(),
        post(
            &format!("{uri}/actors"),
            Some((header::COOKIE.as_str(), &cookie)),
            json!({
                "query": trends_query(),
                "selection": {
                    "type": "TrendsPoint",
                    "series_index": 0,
                    "day": "2026-01-15T00:00:00Z"
                },
                "limit": 1
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["persons"][0]["id"], "person-1");
    assert_eq!(body["has_more"], true);

    drop(app);
    drop(principal);
    drop(access);
    runtime.close();
}

#[tokio::test]
async fn authorization_precedes_validation_and_errors_have_matching_statuses() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let control_path = bootstrap(directory.path());
    let (access, runtime) =
        ProjectAccess::open(control_path.clone()).expect("project access should open");
    let setup = setup(&access).await;
    let first_project_id = setup.workspace.organizations[0].projects[0].id.clone();
    let (_, _second_project_id, second_session_id) = seed_second_user(&control_path);
    let engine = engine(
        directory.path(),
        EngineConfig {
            connections: 1,
            max_queued: 0,
            queue_wait: Duration::from_millis(20),
            ..EngineConfig::default()
        },
    );
    let app = project::router(Arc::new(access.clone()), engine.clone());
    let invalid = json!({
        "query": {
            "kind": "TrendsQuery",
            "series": [],
            "date_range": {"date_from": "-7d"}
        }
    });
    let uri = format!("/api/projects/{first_project_id}/query");

    let second_cookie = format!("hoglet_sid={second_session_id}");
    let (status, headers, body) = json_response(
        app.clone(),
        post(
            &uri,
            Some((header::COOKIE.as_str(), &second_cookie)),
            invalid.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "forbidden");
    assert_eq!(
        body["error"]["request_id"],
        headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .expect("response should expose its request ID")
    );

    let first_cookie = format!("hoglet_sid={}", setup.session_id);
    let (status, _, body) = json_response(
        app.clone(),
        post(
            &uri,
            Some((header::COOKIE.as_str(), &first_cookie)),
            invalid,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_query");

    let (status, _, body) = json_response(
        app.clone(),
        post(
            &uri,
            Some((header::COOKIE.as_str(), &first_cookie)),
            json!({"query": {"kind": "Trends"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");

    let (status, _, body) = json_response(
        app.clone(),
        post(&uri, None, json!({"not": "valid query JSON"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");

    let (status, _, body) = json_response(
        app.clone(),
        post(
            "/api/projects/not-a-project/query",
            Some((header::COOKIE.as_str(), &first_cookie)),
            json!({"query": trends_query()}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    // A full queue answers 503 with Retry-After instead of waiting forever.
    let held = engine.admit().await.expect("first admission");
    let (status, headers, body) = json_response(
        app.clone(),
        post(
            &uri,
            Some((header::COOKIE.as_str(), &first_cookie)),
            json!({"query": trends_query()}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "query_busy");
    assert!(headers.contains_key(header::RETRY_AFTER));
    drop(held);

    drop(app);
    drop(access);
    runtime.close();
}
