//! `hoglet reconcile posthog` against a mock PostHog and a real Hoglet.

use axum::{Json, Router, routing::post};
use hoglet::application::{Application, ApplicationConfig};
use hoglet::reconcile::{ReconcileConfig, render, run};
use serde_json::{Value, json};

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

fn call(url: String, body: Value, cookie: Option<String>) -> (Value, Option<String>) {
    std::thread::spawn(move || {
        let mut request = ureq::post(&url);
        if let Some(cookie) = &cookie {
            request = request.set("Cookie", cookie);
        }
        let response = match request.send_json(body) {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => panic!("{error}"),
        };
        let cookie = response
            .header("set-cookie")
            .and_then(|value| value.split(';').next())
            .map(str::to_owned);
        (response.into_json().unwrap_or(Value::Null), cookie)
    })
    .join()
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn matching_numbers_reconcile_and_differences_are_flagged() {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let posthog_rows = json!({"results": [
        [today, "signed_up", 2, 2],
        [today, "$pageview", 3, 1]
    ]});
    let posthog = serve(Router::new().route(
        "/api/projects/1/query/",
        post(move || {
            let rows = posthog_rows.clone();
            async move { Json(rows) }
        }),
    ))
    .await;

    let data = tempfile::tempdir().unwrap();
    let application = Application::prepare(ApplicationConfig::new(data.path()))
        .await
        .unwrap();
    application.mark_ready();
    let hoglet = serve(application.router()).await;
    let (setup, cookie) = call(
        format!("{hoglet}/api/auth/setup"),
        json!({"email": "o@example.com", "password": "correct horse battery", "organization_name": "Org", "project_name": "Reconcile"}),
        None,
    );
    let project = &setup["organizations"][0]["projects"][0];
    let project_id = project["id"].as_str().unwrap().to_owned();
    let token = project["token"].as_str().unwrap().to_owned();
    let (key, _) = call(format!("{hoglet}/api/auth/keys"), json!({"name": "reconcile"}), cookie);
    let secret = key["secret"].as_str().unwrap().to_owned();

    // Hoglet sees one signed_up fewer than PostHog.
    call(
        format!("{hoglet}/batch/"),
        json!({"api_key": token, "batch": [
            {"event": "signed_up", "distinct_id": "a"},
            {"event": "$pageview", "distinct_id": "a"},
            {"event": "$pageview", "distinct_id": "a"},
            {"event": "$pageview", "distinct_id": "a"}
        ]}),
        None,
    );

    let config = ReconcileConfig {
        posthog_host: posthog,
        posthog_project_id: "1".into(),
        posthog_key: "phx_posthog".into(),
        hoglet_host: hoglet,
        hoglet_project_id: project_id,
        hoglet_key: secret,
        days: 7,
        tolerance_percent: 0.5,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let rows = loop {
        let attempt = config.clone();
        let rows = tokio::task::spawn_blocking(move || run(&attempt)).await.unwrap().expect("reconcile runs");
        if rows.iter().any(|row| row.hoglet.events > 0) {
            break rows;
        }
        assert!(std::time::Instant::now() < deadline, "Hoglet never answered with data");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    let pageview = rows.iter().find(|row| row.event == "$pageview").unwrap();
    assert_eq!((pageview.hoglet.events, pageview.hoglet.persons), (3, 1));
    assert_eq!(pageview.events_diff_percent(), 0.0);
    let signup = rows.iter().find(|row| row.event == "signed_up").unwrap();
    assert_eq!((signup.hoglet.events, signup.posthog.events), (1, 2));
    let (table, all_match) = render(&rows, 0.5);
    assert!(!all_match);
    assert!(table.contains("signed_up"));
    application.shutdown().await.unwrap();
}
