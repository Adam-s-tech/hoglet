//! Shadow mode: events captured by Hoglet reach PostHog with the PostHog
//! project key, after Hoglet acknowledged them.

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use hoglet::application::{Application, ApplicationConfig};
use serde_json::{Value, json};

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

fn call(method: &'static str, url: String, body: Option<Value>, cookie: Option<String>) -> (u16, Value, Option<String>) {
    std::thread::spawn(move || {
        let mut request = ureq::request(method, &url);
        if let Some(cookie) = &cookie {
            request = request.set("Cookie", cookie);
        }
        let result = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => panic!("{error}"),
        };
        let status = response.status();
        let cookie = response
            .header("set-cookie")
            .and_then(|value| value.split(';').next())
            .map(str::to_owned);
        let body = response
            .into_string()
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        (status, body, cookie)
    })
    .join()
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn captured_events_are_forwarded_to_posthog() {
    let received: Arc<Mutex<Vec<Value>>> = Arc::default();
    let posthog = serve(
        Router::new()
            .route(
                "/batch/",
                post(|State(received): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    received.lock().unwrap().push(body);
                    Json(json!({"status": 1}))
                }),
            )
            .with_state(received.clone()),
    )
    .await;

    let data = tempfile::tempdir().unwrap();
    let application = Application::prepare(ApplicationConfig::new(data.path()))
        .await
        .unwrap();
    application.mark_ready();
    let hoglet = serve(application.router()).await;

    let (_, setup, cookie) = call(
        "POST",
        format!("{hoglet}/api/auth/setup"),
        Some(json!({"email": "o@example.com", "password": "correct horse battery", "organization_name": "Org", "project_name": "Shadow"})),
        None,
    );
    let cookie = cookie.unwrap();
    let project = &setup["organizations"][0]["projects"][0];
    let project_id = project["id"].as_str().unwrap().to_owned();
    let token = project["token"].as_str().unwrap().to_owned();

    let (status, _, _) = call(
        "PUT",
        format!("{hoglet}/api/projects/{project_id}/forwarding"),
        Some(json!({"enabled": true, "host": posthog, "posthog_token": "phc_posthog_project"})),
        Some(cookie.clone()),
    );
    assert_eq!(status, 200);

    let (status, _, _) = call(
        "POST",
        format!("{hoglet}/batch/"),
        Some(json!({"api_key": token, "batch": [
            {"event": "signed_up", "distinct_id": "u1", "uuid": "0190f000-0000-7000-8000-00000000000a"},
            {"event": "$pageview", "distinct_id": "u2", "properties": {"$pathname": "/"}}
        ]})),
        None,
    );
    assert_eq!(status, 200);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let bodies = received.lock().unwrap().clone();
        let events: Vec<&Value> = bodies.iter().flat_map(|body| body["batch"].as_array().unwrap().iter()).collect();
        if events.len() == 2 {
            assert!(bodies.iter().all(|body| body["api_key"] == "phc_posthog_project"));
            assert!(events.iter().any(|event| event["uuid"] == "0190f000-0000-7000-8000-00000000000a"));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "events were not forwarded");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let (_, status_body, _) = call(
        "GET",
        format!("{hoglet}/api/projects/{project_id}/forwarding"),
        None,
        Some(cookie),
    );
    assert_eq!(status_body["forwarded"], 2);
    assert_eq!(status_body["dropped"], 0);
    application.shutdown().await.unwrap();
}
