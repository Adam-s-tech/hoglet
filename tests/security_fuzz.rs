//! No-panic and bounded-cost properties for everything an anonymous client
//! controls: the capture decoder and parser, the capture and flags endpoints,
//! and flag regexes.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use hoglet::application::{Application, ApplicationConfig};
use hoglet::capture::{decompress, event};
use http_body_util::BodyExt;
use proptest::prelude::*;
use serde_json::{Value, json};
use tower::ServiceExt;

proptest! {
    #![proptest_config(ProptestConfig { cases: 8000, ..ProptestConfig::default() })]

    #[test]
    fn decode_never_panics(
        body in proptest::collection::vec(any::<u8>(), 0..512),
        form in any::<bool>(),
        hint in proptest::option::of(prop_oneof![Just("lz64"), Just("gzip"), Just("gzip-js"), Just("base64"), Just("x")]),
    ) {
        let _ = decompress::decode_limited(&body, form, hint, 1 << 20);
    }

    #[test]
    fn decode_never_panics_on_structured_inputs(
        text in "[A-Za-z0-9+/=%&_.\\- ]{0,400}",
        prefix in prop_oneof![Just(""), Just("data="), Just("data=H4sI"), Just("compression=lz64&data=")],
        form in any::<bool>(),
        hint in proptest::option::of(prop_oneof![Just("lz64"), Just("gzip")]),
    ) {
        let body = format!("{prefix}{text}");
        let _ = decompress::decode_limited(body.as_bytes(), form, hint, 1 << 20);
    }

    #[test]
    fn timestamps_never_panic(raw in "\\PC{0,40}", suffix in "[+-]?[0-9a-z]{0,3}") {
        let _ = event::parse_timestamp(&raw);
        let _ = event::parse_timestamp(&format!("{raw}{suffix}"));
    }

    #[test]
    fn parse_body_never_panics_on_arbitrary_text(text in ".{0,600}") {
        let now = chrono::Utc::now();
        let _ = event::parse_body(&text, None, now);
    }

    #[test]
    fn parse_body_never_panics_on_event_shaped_json(value in event_like()) {
        let now = chrono::Utc::now();
        let _ = event::parse_body(&value.to_string(), event::parse_sent_at_ms("1700000000000"), now);
    }
}

/// Event-shaped JSON with hostile field types and values.
fn event_like() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|number| json!(number)),
        any::<f64>().prop_map(|number| json!(number)),
        ".{0,40}".prop_map(Value::String),
        Just(json!("2026-10-05T00:00:00Z")),
        Just(json!("not a time")),
        Just(json!("00000000-0000-0000-0000-000000000000")),
    ];
    let field = prop_oneof![
        Just("event"), Just("distinct_id"), Just("$distinct_id"), Just("token"),
        Just("$token"), Just("api_key"), Just("timestamp"), Just("offset"),
        Just("uuid"), Just("properties"), Just("$set"), Just("$set_once"),
        Just("batch"), Just("sent_at"), Just("historical_migration"),
        Just("$ignore_sent_at"),
    ];
    leaf.prop_recursive(3, 24, 4, move |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::vec((field.clone(), inner), 0..6)
                .prop_map(|pairs| Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())),
        ]
    })
}

async fn send(router: &Router, request: Request<Body>) -> StatusCode {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let _ = response.into_body().collect().await.unwrap();
    status
}

struct Server {
    runtime: tokio::runtime::Runtime,
    application: Application,
    router: Router,
    cookie: String,
    project_id: String,
    _dir: tempfile::TempDir,
}

fn server() -> Server {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (application, router, cookie, project_id) = runtime.block_on(async {
        let application = Application::prepare(ApplicationConfig::new(dir.path())).await.unwrap();
        let router = application.router();
        let response = router
            .clone()
            .oneshot(
                Request::post("/api/auth/setup")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"email": "o@example.com", "password": "correct horse battery", "organization_name": "O", "existing_project_token": "phc_fuzz"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let workspace: Value = serde_json::from_slice(&body).unwrap();
        let project_id = workspace["organizations"][0]["projects"][0]["id"].as_str().unwrap().to_owned();
        (application, router, cookie, project_id)
    });
    Server { runtime, application, router, cookie, project_id, _dir: dir }
}

#[test]
fn wire_endpoints_never_5xx_or_panic_on_garbage() {
    let server = server();
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig { cases: 600, ..ProptestConfig::default() });
    let strategy = (
        prop_oneof![Just("/e/"), Just("/batch/"), Just("/i/v0/e/"), Just("/flags/?v=2"), Just("/decide/?v=3"), Just("/engage/"), Just("/array/phc_fuzz/config")],
        prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..300).prop_map(|bytes| bytes),
            event_like().prop_map(|value| value.to_string().into_bytes()),
            event_like().prop_map(|value| {
                let mut wrapped = json!({"api_key": "phc_fuzz", "batch": [value]}).to_string().into_bytes();
                wrapped.truncate(4000);
                wrapped
            }),
        ],
        proptest::option::of(prop_oneof![Just("gzip-js"), Just("lz64"), Just("base64")]),
        any::<bool>(),
    );
    runner
        .run(&strategy, |(path, body, compression, form)| {
            let uri = match compression {
                Some(c) if path.contains('?') => format!("{path}&compression={c}"),
                Some(c) => format!("{path}?compression={c}"),
                None => path.to_owned(),
            };
            let mut request = Request::builder().method(if path.starts_with("/array") { Method::GET } else { Method::POST }).uri(uri);
            if form {
                request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            }
            let status = server.runtime.block_on(send(&server.router, request.body(Body::from(body)).unwrap()));
            prop_assert!(
                status.as_u16() < 500 || status == StatusCode::SERVICE_UNAVAILABLE,
                "{path} answered {status}"
            );
            Ok(())
        })
        .unwrap();
    let Server { runtime, application, router, .. } = server;
    drop(router);
    runtime.block_on(application.shutdown()).unwrap();
}

#[test]
fn flag_regexes_are_bounded() {
    let server = server();
    let create = |pattern: String| {
        Request::post(format!("/api/projects/{}/feature_flags", server.project_id))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &server.cookie)
            .body(Body::from(
                json!({
                    "key": "regex-flag",
                    "filters": {"groups": [{"properties": [{"key": "email", "type": "person", "operator": "regex", "value": pattern}], "rollout_percentage": 100}]}
                })
                .to_string(),
            ))
            .unwrap()
    };
    // A program far past the compiled-size budget is refused or inert, and
    // quickly; never an allocation spike or a hang.
    let started = std::time::Instant::now();
    for pattern in ["((((a{100}){100}){100}){100})", "(a+)+$", "(.*){1000}", "[\\w\\W]{1000}{1000}"] {
        let status = server.runtime.block_on(send(&server.router, create(pattern.to_owned())));
        assert!(status.as_u16() < 500, "{pattern} -> {status}");
        let status = server.runtime.block_on(send(
            &server.router,
            Request::post("/flags/?v=2")
                .body(Body::from(
                    json!({"token": "phc_fuzz", "distinct_id": "u", "person_properties": {"email": "a".repeat(5000) + "!"}})
                        .to_string(),
                ))
                .unwrap(),
        ));
        assert!(status.as_u16() < 500, "{pattern} evaluate -> {status}");
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    let Server { runtime, application, router, .. } = server;
    drop(router);
    runtime.block_on(application.shutdown()).unwrap();
}

/// Flags with every operator and hostile values, evaluated against hostile
/// person properties: never a 5xx, never a panic.
#[test]
fn flag_evaluation_never_panics_for_any_operator_or_value() {
    let server = server();
    let operators = [
        "exact", "is_not", "icontains", "not_icontains", "regex", "not_regex", "gt", "gte", "lt",
        "lte", "is_set", "is_not_set", "is_date_before", "is_date_after",
    ];
    let value = || {
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|number| json!(number)),
            any::<f64>().prop_map(|number| json!(number)),
            "\\PC{0,24}".prop_map(Value::String),
            prop_oneof![
                Just("-99999999999999999999d"),
                Just("-9223372036854775808h"),
                Just("-10000d"),
                Just("-1y"),
                Just("2026-13-45"),
                Just("+262142-12-31T23:59:59Z"),
                Just("-262143-01-01T00:00:00Z"),
                Just("1e999"),
                Just("(a+)+$"),
            ]
            .prop_map(|text| Value::String(text.to_owned())),
            proptest::collection::vec(any::<i32>().prop_map(|number| json!(number)), 0..4)
                .prop_map(Value::Array),
        ]
    };
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 400,
        ..ProptestConfig::default()
    });
    let counter = std::cell::Cell::new(0_u32);
    runner
        .run(
            &(0..operators.len(), value(), value()),
            |(operator, filter_value, person_value)| {
                counter.set(counter.get() + 1);
                let key = format!("fuzz-flag-{}", counter.get());
                let create = Request::post(format!("/api/projects/{}/feature_flags", server.project_id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &server.cookie)
                    .body(Body::from(
                        json!({
                            "key": key,
                            "filters": {"groups": [{
                                "properties": [{"key": "p", "type": "person", "operator": operators[operator], "value": filter_value}],
                                "rollout_percentage": 100
                            }]}
                        })
                        .to_string(),
                    ))
                    .unwrap();
                let status = server.runtime.block_on(send(&server.router, create));
                prop_assert!(status.as_u16() < 500, "create {operator} -> {status}");
                let evaluate = Request::post("/flags/?v=2")
                    .body(Body::from(
                        json!({"token": "phc_fuzz", "distinct_id": "u", "person_properties": {"p": person_value}})
                            .to_string(),
                    ))
                    .unwrap();
                let status = server.runtime.block_on(send(&server.router, evaluate));
                prop_assert!(status == StatusCode::OK, "evaluate {operator} -> {status}");
                Ok(())
            },
        )
        .unwrap();
    let Server { runtime, application, router, .. } = server;
    drop(router);
    runtime.block_on(application.shutdown()).unwrap();
}
