//! The durable pipeline end to end: capture ack → WAL → publication →
//! queryable Parquet, plus recovery, compaction, retention and identity.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use hoglet::capture::event::CapturedEvent;
use hoglet::lake::Lake;
use hoglet::lake::compactor::Compactor;
use hoglet::lake::publisher::Publisher;
use hoglet::persons::PersonStore;
use hoglet::pipeline::wal::{CapturedBatch, WalConfig, WriteAheadLog};
use hoglet::sink::{AuthorizedEventBatch, DurableWalSink, EventSink, PipelineConfig};
use hoglet::storage_bootstrap::{StoragePaths, bootstrap_storage};
use serde_json::{Map, Value, json};
use uuid::Uuid;

const TOKEN: &str = "phc_pipeline_test";

struct Fixture {
    directory: tempfile::TempDir,
    project_id: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let paths = StoragePaths::new(directory.path());
        bootstrap_storage(paths.control(), paths.projections()).expect("paired storage");
        Self {
            directory,
            project_id: Uuid::new_v4().to_string(),
        }
    }

    fn paths(&self) -> StoragePaths {
        StoragePaths::new(self.directory.path())
    }

    fn lake(&self) -> Arc<Lake> {
        Arc::new(
            Lake::open(&self.paths().projections(), &self.directory.path().join("events"))
                .expect("lake opens"),
        )
    }

    fn config(&self) -> PipelineConfig {
        PipelineConfig {
            wal_dir: self.directory.path().join("wal"),
            tmp_dir: self.directory.path().join("tmp"),
            retention_days: None,
        }
    }

    fn batch(&self, events: Vec<CapturedEvent>) -> AuthorizedEventBatch {
        let mut bindings = BTreeMap::new();
        bindings.insert(TOKEN.to_owned(), self.project_id.clone());
        AuthorizedEventBatch {
            events,
            project_ids_by_token: bindings,
            historical_migration: false,
        }
    }
}

fn event(name: &str, distinct_id: &str, hours_ago: i64) -> CapturedEvent {
    event_with(name, distinct_id, hours_ago, Map::new())
}

fn event_with(
    name: &str,
    distinct_id: &str,
    hours_ago: i64,
    properties: Map<String, Value>,
) -> CapturedEvent {
    CapturedEvent {
        uuid: Uuid::new_v4(),
        event: name.to_owned(),
        distinct_id: distinct_id.to_owned(),
        token: TOKEN.to_owned(),
        timestamp: Utc::now() - Duration::hours(hours_ago),
        properties,
    }
}

/// Rows a DuckDB reader sees across the lake's files for the project.
fn stored_rows(lake: &Lake, project_id: &str) -> Vec<(String, String)> {
    let lease = lake.lease_all(project_id);
    if lease.is_empty() {
        return Vec::new();
    }
    let files = lease
        .paths()
        .iter()
        .map(|path| format!("'{}'", path.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let duck = duckdb::Connection::open_in_memory().unwrap();
    let mut statement = duck
        .prepare(&format!(
            "SELECT uuid, event FROM read_parquet([{files}], union_by_name = true) ORDER BY uuid"
        ))
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn acknowledged_events_are_queryable_after_clean_shutdown() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();

    let events: Vec<_> = (0..5).map(|i| event("signed_up", &format!("u{i}"), 1)).collect();
    sink.append(fixture.batch(events.clone())).await.expect("durable ack");
    runtime.shutdown().await.expect("clean shutdown publishes");

    let rows = stored_rows(&lake, &fixture.project_id);
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|(_, name)| name == "signed_up"));
}

#[tokio::test]
async fn events_become_queryable_without_shutdown_within_seconds() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();
    sink.append(fixture.batch(vec![event("pageview", "u1", 0)]))
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while stored_rows(&lake, &fixture.project_id).is_empty()
        || runtime.stats().lag_seconds() > 0.0
    {
        assert!(
            std::time::Instant::now() < deadline,
            "published, and reported caught up, within the freshness bound"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_appends_are_group_committed_and_stored_exactly_once() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();

    let mut tasks = Vec::new();
    for task in 0..32 {
        let sink = sink.clone();
        let batch = fixture.batch(
            (0..50)
                .map(|i| event("click", &format!("u{task}-{i}"), 2))
                .collect(),
        );
        tasks.push(tokio::spawn(async move { sink.append(batch).await }));
    }
    for task in tasks {
        task.await.unwrap().expect("every append acked");
    }
    runtime.shutdown().await.unwrap();

    let rows = stored_rows(&lake, &fixture.project_id);
    assert_eq!(rows.len(), 32 * 50);
    let unique: std::collections::HashSet<_> = rows.iter().map(|(uuid, _)| uuid).collect();
    assert_eq!(unique.len(), rows.len());
}

#[tokio::test]
async fn a_retried_event_inside_one_window_is_stored_once() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();
    let original = event("purchase", "u1", 1);
    sink.append(fixture.batch(vec![original.clone()])).await.unwrap();
    sink.append(fixture.batch(vec![original.clone()])).await.unwrap();
    runtime.shutdown().await.unwrap();

    assert_eq!(stored_rows(&lake, &fixture.project_id).len(), 1);
}

#[tokio::test]
async fn unpublished_wal_records_are_published_on_restart() {
    let fixture = Fixture::new();
    // Acknowledged by a WAL that crashed before publication.
    {
        let (mut wal, _) =
            WriteAheadLog::open(fixture.directory.path().join("wal"), WalConfig::default())
                .unwrap();
        let mut bindings = BTreeMap::new();
        bindings.insert(TOKEN.to_owned(), fixture.project_id.clone());
        for i in 0..3 {
            wal.append(
                CapturedBatch::authorized(
                    vec![event("recovered", &format!("u{i}"), 3)],
                    bindings.clone(),
                    false,
                )
                .unwrap(),
            )
            .unwrap();
        }
    }

    let lake = fixture.lake();
    let (_sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();
    // Published during open, before the server could report ready.
    assert_eq!(stored_rows(&lake, &fixture.project_id).len(), 3);
    runtime.shutdown().await.unwrap();
    assert_eq!(
        std::fs::read_dir(fixture.directory.path().join("wal"))
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "wal"))
            .count(),
        0,
        "published segments are reclaimed"
    );
}

#[test]
fn uncommitted_files_left_by_a_crash_are_removed_on_open() {
    let fixture = Fixture::new();
    let orphan_dir = fixture
        .directory
        .path()
        .join("events")
        .join(&fixture.project_id)
        .join("2026-01-01");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    let orphan = orphan_dir.join("g000000000001-0000.parquet");
    std::fs::write(&orphan, b"half written").unwrap();
    std::fs::write(orphan_dir.join("x.parquet.tmp"), b"temp").unwrap();

    let lake = fixture.lake();
    assert!(!orphan.exists());
    assert!(lake.lease_all(&fixture.project_id).is_empty());
}

/// Publish each batch as its own window, producing one file per batch.
fn publish_batches(fixture: &Fixture, lake: &Arc<Lake>, batches: Vec<Vec<CapturedEvent>>) {
    let (mut wal, _) =
        WriteAheadLog::open(fixture.directory.path().join("wal"), WalConfig::default()).unwrap();
    let publisher = Publisher::new(lake.clone(), wal.reader());
    let mut bindings = BTreeMap::new();
    bindings.insert(TOKEN.to_owned(), fixture.project_id.clone());
    for events in batches {
        wal.append(CapturedBatch::authorized(events, bindings.clone(), false).unwrap())
            .unwrap();
        wal.seal().unwrap();
        publisher.publish_all().unwrap();
    }
}

#[test]
fn compaction_merges_dedups_and_defers_deletion_until_leases_end() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let two_days_ago = 48;
    let retried = event("checkout", "u1", two_days_ago);
    publish_batches(
        &fixture,
        &lake,
        vec![
            vec![retried.clone(), event("view", "u1", two_days_ago)],
            vec![retried.clone()],
            vec![event("view", "u2", two_days_ago)],
        ],
    );
    let before = lake.lease_all(&fixture.project_id);
    assert_eq!(before.files().len(), 3);
    let old_paths = before.paths();

    let compactor = Compactor::new(lake.clone(), &fixture.directory.path().join("tmp"), None)
        .unwrap();
    let done = compactor.step().unwrap().expect("old partition is due");
    assert_eq!(done.input_files, 3);
    assert_eq!(done.input_rows, 4);
    assert_eq!(done.output_rows, 3, "the retried checkout is stored once");
    assert!(compactor.step().unwrap().is_none(), "nothing left to merge");

    let after = lake.lease_all(&fixture.project_id);
    assert_eq!(after.files().len(), 1);
    assert_eq!(stored_rows(&lake, &fixture.project_id).len(), 3);

    // The old lease still reads its files.
    assert_eq!(lake.sweep_graveyard(), 0);
    assert!(old_paths.iter().all(|path| path.exists()));
    drop(before);
    assert_eq!(lake.sweep_graveyard(), 3);
    assert!(old_paths.iter().all(|path| !path.exists()));

    // A reopened lake sees exactly the compacted file.
    drop(lake);
    let reopened = fixture.lake();
    assert_eq!(reopened.lease_all(&fixture.project_id).files().len(), 1);
    assert_eq!(stored_rows(&reopened, &fixture.project_id).len(), 3);
}

#[test]
fn todays_partition_waits_for_enough_small_files() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    publish_batches(
        &fixture,
        &lake,
        (0..3).map(|i| vec![event("view", &format!("u{i}"), 0)]).collect(),
    );
    let compactor = Compactor::new(lake.clone(), &fixture.directory.path().join("tmp"), None)
        .unwrap();
    assert!(compactor.step().unwrap().is_none());
}

#[test]
fn retention_retires_expired_partitions() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    publish_batches(
        &fixture,
        &lake,
        vec![vec![event("old", "u1", 24 * 40), event("new", "u1", 1)]],
    );
    assert_eq!(lake.lease_all(&fixture.project_id).files().len(), 2);
    let compactor = Compactor::new(lake.clone(), &fixture.directory.path().join("tmp"), Some(30))
        .unwrap();
    compactor.step().unwrap();
    let rows = stored_rows(&lake, &fixture.project_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "new");
}

#[test]
fn identify_publishes_an_identity_override() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let mut identify = Map::new();
    identify.insert("$anon_distinct_id".into(), json!("anon-1"));
    identify.insert("$set".into(), json!({"email": "a@example.com"}));
    publish_batches(
        &fixture,
        &lake,
        vec![vec![
            event("$pageview", "anon-1", 2),
            event_with("$identify", "user-1", 1, identify),
        ]],
    );

    let persons = PersonStore::open(&fixture.paths().projections()).unwrap();
    let batch = persons.overrides_since(0).unwrap();
    assert_eq!(batch.changes.len(), 1);
    assert_eq!(batch.changes[0].distinct_id, "anon-1");
    assert_eq!(batch.changes[0].person_id, "user-1");
    let person = persons
        .person_for_distinct_id(&fixture.project_id, "anon-1")
        .unwrap()
        .expect("anonymous id resolves");
    assert_eq!(person.id, "user-1");
    assert_eq!(person.properties["email"], "a@example.com");
    assert!(person.is_identified);
    assert!(persons.overrides_since(batch.max_seq).unwrap().changes.is_empty());
}

#[allow(dead_code)]
fn fixed_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap()
}

#[tokio::test]
async fn erasure_physically_removes_a_person_and_all_their_events() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();
    let mut identify = Map::new();
    identify.insert("$anon_distinct_id".into(), json!("anon-9"));
    identify.insert("$set".into(), json!({"email": "erase-me@example.com"}));
    sink.append(fixture.batch(vec![
        event("$pageview", "anon-9", 50),
        event("$pageview", "anon-9", 2),
        event_with("$identify", "user-9", 1, identify),
        event("purchase", "user-9", 1),
        event("$pageview", "someone-else", 1),
    ]))
    .await
    .unwrap();

    let persons = PersonStore::open(&fixture.paths().projections()).unwrap();
    let epoch_before = persons.overrides_since(0).unwrap().epoch;
    let report = runtime
        .eraser()
        .erase_person(&fixture.project_id, "user-9")
        .await
        .expect("erasure succeeds");
    assert_eq!(report.distinct_ids, 2);
    assert_eq!(report.events, 4);

    let rows = stored_rows(&lake, &fixture.project_id);
    assert_eq!(rows.len(), 1, "only the other person's event remains");
    assert!(persons
        .person_for_distinct_id(&fixture.project_id, "anon-9")
        .unwrap()
        .is_none());
    assert!(persons
        .person_for_distinct_id(&fixture.project_id, "user-9")
        .unwrap()
        .is_none());
    assert_ne!(persons.overrides_since(0).unwrap().epoch, epoch_before);

    // No file on disk still holds the erased ids once leases are gone.
    lake.sweep_graveyard();
    runtime.shutdown().await.unwrap();
    let reopened = fixture.lake();
    assert_eq!(stored_rows(&reopened, &fixture.project_id).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "stress test, minutes in a debug build: run with --ignored (nightly CI)"]
async fn erasure_finishes_while_ingest_runs_flat_out() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (sink, runtime, _) = DurableWalSink::open(fixture.config(), lake.clone()).unwrap();
    sink.append(fixture.batch(vec![
        event("$pageview", "erase-me", 3),
        event("$pageview", "stay", 3),
    ]))
    .await
    .unwrap();

    // Writers that never stop, so new segments keep sealing behind the erasure.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut writers = Vec::new();
    for task in 0..4 {
        let sink = sink.clone();
        let stop = stop.clone();
        let batches: Vec<_> = (0..8)
            .map(|i| fixture.batch((0..40).map(|j| event("click", &format!("w{task}-{i}-{j}"), 1)).collect()))
            .collect();
        writers.push(tokio::spawn(async move {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                for batch in &batches {
                    let mut batch = batch.clone();
                    for event in &mut batch.events {
                        event.uuid = Uuid::new_v4();
                    }
                    let _ = sink.append(batch).await;
                }
            }
        }));
    }
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let started = std::time::Instant::now();
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        runtime.eraser().erase_person(&fixture.project_id, "erase-me"),
    )
    .await
    .expect("erasure must not be starved by concurrent ingest")
    .expect("erasure succeeds");
    assert_eq!(report.events, 1);
    assert!(started.elapsed() < std::time::Duration::from_secs(20));

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for writer in writers {
        writer.await.unwrap();
    }
    runtime.shutdown().await.unwrap();
}

/// The deterministic core of "erasure and shutdown are not starved by
/// ingest": steady-state publication is budgeted per call, and an erasure
/// publishes only through the segments sealed when it was requested.
#[test]
fn publication_is_bounded_per_call_and_erasure_stops_at_its_segment() {
    let fixture = Fixture::new();
    let lake = fixture.lake();
    let (mut wal, _) = WriteAheadLog::open(
        fixture.directory.path().join("wal"),
        WalConfig::new(1, 64 * 1024 * 1024).unwrap(), // every record seals its own segment
    )
    .unwrap();
    let publisher = Publisher::new(lake.clone(), wal.reader());
    let mut bindings = BTreeMap::new();
    bindings.insert(TOKEN.to_owned(), fixture.project_id.clone());
    for i in 0..6 {
        wal.append(
            CapturedBatch::authorized(vec![event("click", &format!("u{i}"), 1)], bindings.clone(), false)
                .unwrap(),
        )
        .unwrap();
    }
    wal.seal().unwrap();
    let active = wal.active_segment();
    assert!(active >= 6, "one sealed segment per record, got active {active}");

    // A zero budget does exactly one window, however much is waiting.
    let (done, _pending) = publisher.publish_for(std::time::Duration::ZERO).unwrap();
    assert_eq!(done.len(), 1);

    // `publish_through` stops once its segment is published (a window may
    // carry later segments with it, never fewer).
    publisher.publish_through(3).unwrap();
    assert!(publisher.checkpoint().unwrap().segment > 3);

    // The remainder drains normally and nothing is lost or repeated.
    publisher.publish_all().unwrap();
    assert_eq!(stored_rows(&lake, &fixture.project_id).len(), 6);
}
