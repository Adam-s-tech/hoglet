//! `hoglet import posthog` against a mock PostHog API and a real Hoglet.

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::{get, post}};
use hoglet::application::{Application, ApplicationConfig};
use hoglet::import::{ImportConfig, Importer};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct MockPostHog {
    queries: Arc<Mutex<Vec<String>>>,
}

async fn query(State(mock): State<MockPostHog>, Json(body): Json<Value>) -> Json<Value> {
    let hogql = body["query"]["query"].as_str().unwrap_or("").to_owned();
    mock.queries.lock().unwrap().push(hogql.clone());
    if hogql.contains("FROM events") {
        if hogql.contains("toString(uuid) >") {
            return Json(json!({"results": []}));
        }
        return Json(json!({"results": [
            ["0190f000-0000-7000-8000-000000000001", "$pageview", "anon-1", "2026-09-01 10:00:00.000000",
             "{\"$pathname\":\"/pricing\",\"$session_id\":\"s1\"}"],
            ["0190f000-0000-7000-8000-000000000002", "signed_up", "user-1", "2026-09-01 10:05:00.000000",
             "{\"plan\":\"pro\"}"],
            ["0190f000-0000-7000-8000-000000000003", "$pageview", "anon-2", "2026-09-02 09:00:00.000000", "{}"]
        ]}));
    }
    if hogql.contains("FROM persons") {
        if hogql.contains("> ''") {
            return Json(json!({"results": [
                ["person-a", "{\"email\":\"a@example.com\"}", ["anon-1", "user-1"]],
                ["person-b", "{}", ["anon-2"]]
            ]}));
        }
        return Json(json!({"results": []}));
    }
    Json(json!({"results": []}))
}

async fn flags() -> Json<Value> {
    Json(json!({
        "next": null,
        "results": [{
            "id": 7, "key": "new-onboarding", "name": "New onboarding", "active": true,
            "deleted": false, "ensure_experience_continuity": false,
            "filters": {"groups": [{"properties": [], "rollout_percentage": 50}], "payloads": {}}
        }]
    }))
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_posthog_project_moves_with_identity_and_flags() {
    let mock = MockPostHog::default();
    let posthog = serve(
        Router::new()
            .route("/api/projects/1/query/", post(query))
            .route("/api/projects/1/feature_flags/", get(flags))
            .with_state(mock.clone()),
    )
    .await;

    let data = tempfile::tempdir().unwrap();
    let application = Application::prepare(ApplicationConfig::new(data.path()))
        .await
        .unwrap();
    application.mark_ready();
    let hoglet = serve(application.router()).await;

    let client = reqwest_like(&hoglet);
    let setup = client.post_json(
        "/api/auth/setup",
        &json!({"email": "owner@example.com", "password": "correct horse battery", "organization_name": "Org", "project_name": "Imported"}),
        None,
    );
    let cookie = setup.cookie.expect("session cookie");
    let project = &setup.body["organizations"][0]["projects"][0];
    let project_id = project["id"].as_str().unwrap().to_owned();
    let token = project["token"].as_str().unwrap().to_owned();
    let key = client.post_json("/api/auth/keys", &json!({"name": "import", "scope": "write"}), Some(&cookie));
    let secret = key.body["secret"].as_str().expect("personal key secret").to_owned();

    let checkpoint = data.path().join("import-checkpoint.json");
    let config = ImportConfig {
        posthog_host: posthog,
        posthog_project_id: "1".into(),
        posthog_key: "phx_posthog".into(),
        hoglet_host: hoglet.clone(),
        hoglet_token: token,
        hoglet_key: Some(secret),
        hoglet_project_id: Some(project_id.clone()),
        since: None,
        checkpoint: checkpoint.clone(),
        skip_events: false,
        skip_persons: false,
    };
    let report = tokio::task::spawn_blocking(move || Importer::new(config, Box::new(|_| {})).run())
        .await
        .unwrap()
        .expect("import succeeds");
    assert_eq!(report.events, 3);
    assert_eq!(report.persons, 2);
    assert_eq!(report.flags, 1);
    assert!(checkpoint.exists());

    // Events, uuids intact, become queryable.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let events = loop {
        let events = client.get(&format!("/api/projects/{project_id}/events?limit=100"), &cookie);
        let names: Vec<String> = events.body["events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|event| event["event"].as_str().map(str::to_owned))
            .collect();
        if names.iter().filter(|name| !name.starts_with("$merge") && *name != "$set").count() >= 3 {
            break events.body;
        }
        assert!(std::time::Instant::now() < deadline, "imported events never became visible");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    let uuids: Vec<&str> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["uuid"].as_str())
        .collect();
    assert!(uuids.contains(&"0190f000-0000-7000-8000-000000000001"));

    // anon-1 and user-1 are one person carrying PostHog's properties.
    let person = client.get(&format!("/api/projects/{project_id}/persons/anon-1"), &cookie);
    let ids: Vec<&str> = person.body["distinct_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(ids.contains(&"anon-1") && ids.contains(&"user-1"), "{}", person.body);
    assert_eq!(person.body["person"]["properties"]["email"], "a@example.com");

    // The flag arrived with its rollout.
    let flags = client.get(&format!("/api/projects/{project_id}/feature_flags"), &cookie);
    let flag = flags.body.as_array().and_then(|flags| flags.first()).cloned().unwrap_or(flags.body["results"][0].clone());
    assert_eq!(flag["key"], "new-onboarding");

    // Re-running resumes from the checkpoint and adds nothing.
    let rerun = ImportConfig {
        posthog_host: String::new(),
        posthog_project_id: "1".into(),
        posthog_key: String::new(),
        hoglet_host: hoglet,
        hoglet_token: String::new(),
        hoglet_key: None,
        hoglet_project_id: None,
        since: None,
        checkpoint,
        skip_events: false,
        skip_persons: false,
    };
    let again = tokio::task::spawn_blocking(move || Importer::new(rerun, Box::new(|_| {})).run())
        .await
        .unwrap()
        .expect("a finished import is a no-op");
    assert_eq!(again.events, 3);
    assert_eq!(again.persons, 0);

    application.shutdown().await.unwrap();
}

/// Minimal blocking HTTP helper over ureq, run off the async runtime.
struct Client {
    base: String,
}

struct Reply {
    body: Value,
    cookie: Option<String>,
}

fn reqwest_like(base: &str) -> Client {
    Client {
        base: base.to_owned(),
    }
}

impl Client {
    fn post_json(&self, path: &str, body: &Value, cookie: Option<&str>) -> Reply {
        let url = format!("{}{path}", self.base);
        let body = body.clone();
        let cookie = cookie.map(str::to_owned);
        std::thread::spawn(move || {
            let mut request = ureq::post(&url);
            if let Some(cookie) = &cookie {
                request = request.set("Cookie", cookie);
            }
            reply(request.send_json(body))
        })
        .join()
        .unwrap()
    }

    fn get(&self, path: &str, cookie: &str) -> Reply {
        let url = format!("{}{path}", self.base);
        let cookie = cookie.to_owned();
        std::thread::spawn(move || reply(ureq::get(&url).set("Cookie", &cookie).call()))
            .join()
            .unwrap()
    }
}

fn reply(result: Result<ureq::Response, ureq::Error>) -> Reply {
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(error) => panic!("request failed: {error}"),
    };
    let cookie = response
        .header("set-cookie")
        .and_then(|value| value.split(';').next())
        .map(str::to_owned);
    let body = response
        .into_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    Reply { body, cookie }
}
