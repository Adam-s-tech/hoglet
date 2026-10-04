//! Process-boundary durability: every event acknowledged with a 2xx remains
//! queryable after SIGKILL and a normal production restart.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind an ephemeral port")
        .local_addr()
        .expect("read the ephemeral port")
        .port()
}

fn spawn_hoglet(data_dir: &std::path::Path, port: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hoglet"))
        .env("HOGLET_DATA", data_dir)
        .env("HOGLET_ADDR", format!("127.0.0.1:{port}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hoglet binary")
}

fn wait_until_up(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("hoglet did not come up on port {port}");
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    body: &Value,
    extra_headers: &[(&str, &str)],
) -> Option<String> {
    let body = body.to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    let mut headers = String::new();
    for (name, value) in extra_headers {
        headers.push_str(name);
        headers.push_str(": ");
        headers.push_str(value);
        headers.push_str("\r\n");
    }
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

fn status(response: &str) -> Option<u16> {
    response
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}



fn post_batch(port: u16, uuids: &[String]) -> bool {
    let batch: Vec<Value> = uuids
        .iter()
        .map(|uuid| {
            json!({
                "event": "crash_test",
                "distinct_id": format!("u-{}", &uuid[..8]),
                "uuid": uuid,
                "timestamp": "2026-08-20T12:00:00Z"
            })
        })
        .collect();
    request(
        port,
        "POST",
        "/batch/",
        &json!({"api_key": "phc_crash", "batch": batch}),
        &[],
    )
    .and_then(|response| status(&response))
        == Some(200)
}

/// Every uuid stored in the data directory's event files.
fn stored_uuids(data_dir: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![data_dir.join("events")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "parquet") {
                files.push(format!("'{}'", path.display()));
            }
        }
    }
    if files.is_empty() {
        return Vec::new();
    }
    let duck = duckdb::Connection::open_in_memory().unwrap();
    let mut statement = duck
        .prepare(&format!(
            "SELECT uuid FROM read_parquet([{}], union_by_name = true)",
            files.join(", ")
        ))
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Claim 2: every event acknowledged with a 2xx survives SIGKILL at an
/// arbitrary moment during concurrent ingest, and is stored exactly once.
#[test]
fn sigkill_during_concurrent_ingest_loses_no_acked_events() {
    let directory = tempfile::tempdir().expect("temporary data directory");
    let port = free_port();
    let mut server = spawn_hoglet(directory.path(), port);
    wait_until_up(port);

    let setup = request(
        port,
        "POST",
        "/api/auth/setup",
        &json!({
            "email": "owner@example.com",
            "password": "correct horse battery staple",
            "organization_name": "Crash Recovery",
            "project_name": "Durability",
            "existing_project_token": "phc_crash"
        }),
        &[],
    )
    .expect("setup response");
    assert_eq!(status(&setup), Some(200), "setup failed: {setup}");

    let acknowledged = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    for cycle in 0..4_u64 {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (0..6)
            .map(|_| {
                let stop = stop.clone();
                let acknowledged = acknowledged.clone();
                std::thread::spawn(move || {
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let uuids: Vec<String> = (0..10)
                            .map(|_| uuid::Uuid::now_v7().to_string())
                            .collect();
                        if post_batch(port, &uuids) {
                            acknowledged.lock().unwrap().extend(uuids);
                        }
                    }
                })
            })
            .collect();
        // Kill at a different moment each cycle, mid-ingest.
        std::thread::sleep(Duration::from_millis(150 + cycle * 170));
        server.kill().expect("SIGKILL");
        server.wait().expect("reap");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for writer in writers {
            writer.join().unwrap();
        }
        server = spawn_hoglet(directory.path(), port);
        wait_until_up(port);
    }

    // Recovery published everything before the last server came up; stop it
    // without SIGKILL so nothing is in flight while files are read.
    let _ = Command::new("kill")
        .arg("-INT")
        .arg(server.id().to_string())
        .status();
    server.wait().expect("graceful stop");

    let acknowledged = acknowledged.lock().unwrap().clone();
    assert!(acknowledged.len() > 100, "the load generator acknowledged too little");
    let stored = stored_uuids(directory.path());
    let stored_set: std::collections::HashSet<&String> = stored.iter().collect();
    assert_eq!(stored_set.len(), stored.len(), "an event was stored twice");
    let lost: Vec<&String> = acknowledged
        .iter()
        .filter(|uuid| !stored_set.contains(uuid))
        .collect();
    assert!(
        lost.is_empty(),
        "{} of {} acknowledged events were lost, e.g. {:?}",
        lost.len(),
        acknowledged.len(),
        lost.first()
    );
}
