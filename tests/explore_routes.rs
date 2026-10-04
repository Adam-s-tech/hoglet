//! Explore routes through real routers: authorization, query parsing,
//! contract shapes, and error codes.

#[path = "support/explore_data.rs"]
mod explore_data;

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use explore_data::{Fixture, event};
use hoglet::application::{Application, ApplicationConfig};
use hoglet::control::{ProjectAccess, SetupRequest};
use hoglet::projection_catalog::ProjectionCatalog;
use hoglet::routes;
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

async fn get(app: &Router, uri: &str, credential: Option<(&str, &str)>) -> (StatusCode, Value) {
    let mut request = Request::get(uri);
    if let Some((name, value)) = credential {
        request = request.header(name, value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        assert_eq!(
            body["error"]["request_id"].as_str(),
            request_id.as_deref(),
            "{uri}: error bodies carry the request id"
        );
        assert!(body["error"]["code"].is_string(), "{uri}: {body}");
    }
    (status, body)
}

fn seed_second_user(control_path: &std::path::Path) -> String {
    let user_id = Uuid::new_v4().to_string();
    let organization_id = Uuid::new_v4().to_string();
    let project_id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();
    let connection = Connection::open(control_path).expect("control database");
    connection
        .execute_batch("PRAGMA foreign_keys=ON;")
        .expect("foreign keys");
    connection
        .execute(
            "INSERT INTO users(id,email,password_hash,name,created_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![user_id, "second@example.com", "unused", "Second", now],
        )
        .expect("user");
    connection
        .execute(
            "INSERT INTO organizations(id,name,created_at) VALUES (?1,?2,?3)",
            rusqlite::params![organization_id, "Second Org", now],
        )
        .expect("organization");
    connection
        .execute(
            "INSERT INTO organization_members(organization_id,user_id,role) VALUES (?1,?2,'owner')",
            rusqlite::params![organization_id, user_id],
        )
        .expect("membership");
    connection
        .execute(
            "INSERT INTO projects(id,organization_id,name,capture_token,created_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![project_id, organization_id, "Second Project", "phc_second", now],
        )
        .expect("project");
    connection
        .execute(
            "INSERT INTO auth_sessions(id,user_id,created_at,expires_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![session_id, user_id, now, now + 3600],
        )
        .expect("session");
    session_id
}

#[tokio::test]
async fn explore_routes_authorize_parse_and_answer() {
    let mut fixture = Fixture::new();
    let (access, runtime) = ProjectAccess::open(fixture.control.clone()).expect("project access");
    let setup = access
        .setup(SetupRequest {
            email: "owner@example.com".into(),
            password: "correct horse battery staple".into(),
            organization_name: "Acme".into(),
            project_name: "Website".into(),
            existing_project_token: Some("phc_explore".into()),
        })
        .await
        .expect("setup");
    let project_id = setup.workspace.organizations[0].projects[0].id.clone();
    let principal = access
        .validate_session(&setup.session_id)
        .await
        .expect("session");
    let key = access
        .create_personal_key(&principal, "explore test", hoglet::control::KeyScope::Read)
        .await
        .expect("personal key");
    let second_session = seed_second_user(&fixture.control);

    let now = Utc::now();
    let t = |minutes: i64| now - Duration::minutes(minutes);
    let events = vec![
        event(
            "$pageview",
            "anon-a",
            t(120),
            json!({"$session_id": "S1", "$pathname": "/", "$browser": "Chrome"}),
        ),
        event(
            "$pageview",
            "anon-a",
            t(115),
            json!({"$session_id": "S1", "$pathname": "/pricing", "$browser": "Chrome"}),
        ),
        event(
            "$identify",
            "user-1",
            t(114),
            json!({"$session_id": "S1", "$anon_distinct_id": "anon-a", "$set": {"email": "one@example.com", "name": "One"}}),
        ),
        event(
            "$pageview",
            "anon-b",
            t(60),
            json!({"$session_id": "S2", "$pathname": "/docs", "$browser": "Firefox"}),
        ),
        event(
            "$identify",
            "user-1",
            t(59),
            json!({"$session_id": "S2", "$anon_distinct_id": "anon-b"}),
        ),
        event(
            "$pageview",
            "anon-c",
            t(1),
            json!({"$session_id": "S3", "$pathname": "/", "$browser": "Safari"}),
        ),
    ];
    fixture.load(&project_id, &events);

    let access = Arc::new(access);
    let explorer = fixture.explorer.clone();
    let app = routes::persons::router(access.clone(), explorer.clone())
        .merge(routes::events::router(access.clone(), explorer.clone()))
        .merge(routes::web::router(access.clone(), explorer))
        .merge(routes::catalog_v2::router(
            access.clone(),
            Arc::new(ProjectionCatalog::open(&fixture.projections).expect("catalog")),
        ));
    let cookie = format!("hoglet_sid={}", setup.session_id);
    let me = Some((header::COOKIE.as_str(), cookie.as_str()));
    let base = format!("/api/projects/{project_id}");

    // Every endpoint: 401 without credentials, 403 for another user's
    // session, 404 for a malformed project id.
    let second_cookie = format!("hoglet_sid={second_session}");
    for path in [
        "/persons",
        "/persons/user-1",
        "/persons/user-1/events",
        "/events",
        "/web/overview",
        "/web/breakdown?dimension=page",
        "/catalog/events",
        "/catalog/properties",
        "/catalog/values?key=x",
    ] {
        let uri = format!("{base}{path}");
        assert_eq!(
            get(&app, &uri, None).await.0,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        assert_eq!(
            get(&app, &uri, Some((header::COOKIE.as_str(), &second_cookie)))
                .await
                .0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
        assert_eq!(
            get(&app, &format!("/api/projects/not-a-project{path}"), me)
                .await
                .0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }

    // Persons.
    let (status, body) = get(&app, &format!("{base}/persons"), me).await;
    assert_eq!(status, StatusCode::OK);
    let persons = body["persons"].as_array().expect("persons");
    assert_eq!(persons.len(), 2);
    assert_eq!(persons[0]["id"], "anon-c");
    assert_eq!(persons[1]["display_name"], "one@example.com");
    assert_eq!(
        persons[1]["distinct_ids"],
        json!(["anon-a", "user-1", "anon-b"])
    );
    assert!(body["next_cursor"].is_null());
    let (_, page) = get(&app, &format!("{base}/persons?limit=1"), me).await;
    let cursor = page["next_cursor"].as_str().expect("cursor").to_owned();
    let (_, next) = get(&app, &format!("{base}/persons?limit=1&cursor={cursor}"), me).await;
    assert_eq!(next["persons"][0]["id"], "user-1");
    assert!(next["next_cursor"].is_null());
    let (_, found) = get(&app, &format!("{base}/persons?search=ONE%40"), me).await;
    assert_eq!(found["persons"].as_array().map(Vec::len), Some(1));
    let (_, found) = get(&app, &format!("{base}/persons?search=anon-b"), me).await;
    assert_eq!(found["persons"][0]["id"], "user-1");

    let (status, detail) = get(&app, &format!("{base}/persons/anon-b"), me).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["person"]["id"], "user-1");
    assert_eq!(detail["event_count"], 5);
    assert_eq!(detail["session_count"], 2);
    assert!(detail["first_seen"].is_string());
    let (status, body) = get(&app, &format!("{base}/persons/nobody"), me).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let (_, first) = get(&app, &format!("{base}/persons/user-1/events?limit=3"), me).await;
    assert_eq!(first["events"].as_array().map(Vec::len), Some(3));
    let before = encode(first["next_before"].as_str().expect("next_before"));
    let (_, rest) = get(
        &app,
        &format!("{base}/persons/user-1/events?limit=3&before={before}"),
        me,
    )
    .await;
    assert_eq!(rest["events"].as_array().map(Vec::len), Some(2));
    assert!(rest["next_before"].is_null());
    assert_eq!(rest["events"][1]["distinct_id"], "anon-a");
    assert_eq!(rest["events"][1]["person_id"], "user-1");

    // Events feed.
    let (status, all) = get(&app, &format!("{base}/events"), me).await;
    assert_eq!(status, StatusCode::OK);
    let rows = all["events"].as_array().expect("events");
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["distinct_id"], "anon-c");
    assert_eq!(rows[0]["properties"]["$pathname"], "/");
    let (_, views) = get(&app, &format!("{base}/events?event=%24pageview"), me).await;
    assert_eq!(views["events"].as_array().map(Vec::len), Some(4));
    let (_, mine) = get(&app, &format!("{base}/events?person_id=user-1"), me).await;
    assert_eq!(mine["events"].as_array().map(Vec::len), Some(5));
    let (_, one) = get(
        &app,
        &format!("{base}/events?person_id=user-1&distinct_id=anon-b"),
        me,
    )
    .await;
    assert_eq!(one["events"].as_array().map(Vec::len), Some(1));
    let (_, nobody) = get(&app, &format!("{base}/events?person_id=nobody"), me).await;
    assert_eq!(nobody["events"], json!([]));
    let filter = encode(r#"[{"key":"$browser","operator":"exact","value":["Chrome","Safari"]}]"#);
    let (_, filtered) = get(&app, &format!("{base}/events?properties={filter}"), me).await;
    assert_eq!(filtered["events"].as_array().map(Vec::len), Some(3));

    // Web analytics.
    let (status, overview) = get(&app, &format!("{base}/web/overview?date_from=-24h"), me).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["visitors"]["value"], 2.0);
    assert_eq!(overview["pageviews"]["value"], 4.0);
    assert_eq!(overview["sessions"]["value"], 3.0);
    assert_eq!(overview["visitors"]["previous"], 0.0);
    assert_eq!(overview["interval"], "hour");
    assert_eq!(overview["live_visitors"], 1);
    assert_eq!(
        overview["days"].as_array().map(Vec::len),
        overview["pageviews_series"].as_array().map(Vec::len)
    );
    let (_, filtered) = get(
        &app,
        &format!("{base}/web/overview?date_from=-7d&interval=day&properties={filter}"),
        me,
    )
    .await;
    assert_eq!(filtered["pageviews"]["value"], 3.0);
    let (status, breakdown) = get(
        &app,
        &format!("{base}/web/breakdown?dimension=entry_page&limit=1&date_from=-24h"),
        me,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(breakdown["dimension"], "entry_page");
    assert_eq!(
        breakdown["rows"],
        json!([{"value": "/", "visitors": 2, "views": 2, "bounce_rate": 50.0}])
    );
    let (_, browsers) = get(&app, &format!("{base}/web/breakdown?dimension=browser"), me).await;
    assert_eq!(browsers["rows"].as_array().map(Vec::len), Some(3));
    assert!(browsers["rows"][0]["bounce_rate"].is_null());

    // Catalog.
    let (status, names) = get(&app, &format!("{base}/catalog/events?search=PAGE"), me).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        names,
        json!([{"name": "$pageview", "count": 4, "last_seen": names[0]["last_seen"]}])
    );
    let (_, keys) = get(&app, &format!("{base}/catalog/properties?type=person"), me).await;
    let keys: Vec<&str> = keys
        .as_array()
        .expect("keys")
        .iter()
        .filter_map(|k| k["key"].as_str())
        .collect();
    assert_eq!(keys, vec!["email", "name"]);
    let (_, keys) = get(
        &app,
        &format!("{base}/catalog/properties?type=event&search=brow"),
        me,
    )
    .await;
    assert_eq!(
        keys,
        json!([{"key": "$browser", "type": "event", "property_type": "string", "count": 4}])
    );
    let (_, values) = get(&app, &format!("{base}/catalog/values?key=%24pathname"), me).await;
    assert_eq!(values[0], json!({"value": "/", "count": 2}));
    let (_, values) = get(
        &app,
        &format!("{base}/catalog/values?key=email&type=person"),
        me,
    )
    .await;
    assert_eq!(values, json!([{"value": "one@example.com", "count": 1}]));

    // Personal API keys work like sessions.
    let bearer = format!("Bearer {}", key.secret);
    let (status, _) = get(
        &app,
        &format!("{base}/web/overview"),
        Some((header::AUTHORIZATION.as_str(), &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Malformed input is a 400, never a 500.
    let person_filter = encode(r#"[{"key":"email","type":"person","value":"x"}]"#);
    let bad_regex = encode(r#"[{"key":"$pathname","operator":"regex","value":"("}]"#);
    for path in [
        "/events?before=yesterday".to_owned(),
        "/events?limit=abc".to_owned(),
        "/events?properties=notjson".to_owned(),
        format!("/events?properties={person_filter}"),
        format!("/events?properties={bad_regex}"),
        format!("/events?event={}", "x".repeat(500)),
        "/persons?cursor=garbage".to_owned(),
        "/persons/user-1/events?before=nope".to_owned(),
        "/web/breakdown".to_owned(),
        "/web/breakdown?dimension=bogus".to_owned(),
        "/web/overview?date_from=garbage".to_owned(),
        "/web/overview?date_from=2020-01-01&interval=hour".to_owned(),
        "/web/overview?interval=fortnight".to_owned(),
        format!("/web/overview?properties={bad_regex}"),
        "/catalog/values".to_owned(),
        "/catalog/properties?type=group".to_owned(),
    ] {
        let (status, body) = get(&app, &format!("{base}{path}"), me).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
    }

    drop(app);
    drop(access);
    runtime.close();
}

#[tokio::test]
async fn application_mounts_explore_routes() {
    let directory = tempfile::tempdir().expect("data directory");
    let application = Application::prepare(ApplicationConfig::new(directory.path()))
        .await
        .expect("application starts");
    let router = application.router();
    let setup = router
        .clone()
        .oneshot(
            Request::post("/api/auth/setup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "email": "owner@example.com",
                        "password": "correct horse battery staple",
                        "organization_name": "Acme",
                        "project_name": "Website",
                        "existing_project_token": "phc_mounted"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("setup response");
    assert_eq!(setup.status(), StatusCode::OK);
    let cookie = setup
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("session cookie")
        .to_owned();
    let body: Value =
        serde_json::from_slice(&setup.into_body().collect().await.expect("body").to_bytes())
            .expect("workspace");
    let project_id = body["organizations"][0]["projects"][0]["id"]
        .as_str()
        .expect("project id")
        .to_owned();
    let me = Some((header::COOKIE.as_str(), cookie.as_str()));
    for path in [
        "/persons",
        "/events",
        "/web/overview",
        "/web/breakdown?dimension=country",
        "/catalog/events",
        "/catalog/properties?type=person",
    ] {
        let (status, body) = get(&router, &format!("/api/projects/{project_id}{path}"), me).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let (_, overview) = get(
        &router,
        &format!("/api/projects/{project_id}/web/overview"),
        me,
    )
    .await;
    assert_eq!(overview["visitors"]["value"], 0.0);
    assert_eq!(overview["days"].as_array().map(Vec::len), Some(8));
    drop(router);
    application.shutdown().await.expect("shutdown");
}
