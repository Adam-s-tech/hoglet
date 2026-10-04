//! Claim 2 / erasure: an erasure interrupted after the identity transaction
//! committed but before the event files were rewritten must complete on the
//! next start. Before this was fixed, the person was gone from SQLite while
//! their events stayed in the lake forever: the retry answered 404 ("no such
//! person") and nothing knew which distinct ids to purge.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use hoglet::capture::event::CapturedEvent;
use hoglet::lake::Lake;
use hoglet::sink::{AuthorizedEventBatch, DurableWalSink, EventSink, PipelineConfig};
use hoglet::storage_bootstrap::{StoragePaths, bootstrap_storage};
use serde_json::Map;
use uuid::Uuid;

const TOKEN: &str = "phc_erasure_atomicity";

fn event(distinct_id: &str) -> CapturedEvent {
    CapturedEvent {
        uuid: Uuid::new_v4(),
        event: "pageview".to_owned(),
        distinct_id: distinct_id.to_owned(),
        token: TOKEN.to_owned(),
        // A past day: its partition is compactable, like real history.
        timestamp: Utc::now() - Duration::days(3),
        properties: Map::new(),
    }
}

fn stored_distinct_ids(lake: &Lake, project_id: &str) -> Vec<String> {
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
            "SELECT distinct_id FROM read_parquet([{files}], union_by_name = true)"
        ))
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn erasure_interrupted_after_the_identity_commit_completes_on_restart() {
    let directory = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(directory.path());
    bootstrap_storage(paths.control(), paths.projections()).unwrap();
    let project_id = Uuid::new_v4().to_string();
    let config = || PipelineConfig {
        wal_dir: directory.path().join("wal"),
        tmp_dir: directory.path().join("tmp"),
        retention_days: None,
    };
    let open_lake = || {
        Arc::new(Lake::open(&paths.projections(), &directory.path().join("events")).unwrap())
    };

    // History for a person to erase and a bystander, fully published.
    let lake = open_lake();
    let (sink, runtime, _) = DurableWalSink::open(config(), lake.clone()).unwrap();
    let mut bindings = BTreeMap::new();
    bindings.insert(TOKEN.to_owned(), project_id.clone());
    let events: Vec<_> = (0..4)
        .map(|_| event("victim"))
        .chain((0..3).map(|_| event("bystander")))
        .collect();
    sink.append(AuthorizedEventBatch {
        events,
        project_ids_by_token: bindings,
        historical_migration: false,
    })
    .await
    .unwrap();
    runtime.shutdown().await.unwrap();
    drop((sink, lake));

    // The erasure's first step commits, then the process dies: exactly what
    // `Eraser` does before it rewrites the event files.
    {
        let mut connection = rusqlite::Connection::open(paths.projections()).unwrap();
        let transaction = connection.transaction().unwrap();
        let ids = hoglet::projections::erase_person(&transaction, &project_id, "victim").unwrap();
        assert_eq!(ids, vec!["victim".to_owned()]);
        transaction.commit().unwrap();
    }

    // Restart: opening the pipeline must finish the erasure.
    let lake = open_lake();
    let (_sink, runtime, _) = DurableWalSink::open(config(), lake.clone()).unwrap();
    let stored = stored_distinct_ids(&lake, &project_id);
    assert!(
        !stored.iter().any(|id| id == "victim"),
        "the erased person's events survived the restart: {stored:?}"
    );
    assert_eq!(
        stored.iter().filter(|id| *id == "bystander").count(),
        3,
        "erasure must not touch anyone else"
    );
    runtime.shutdown().await.unwrap();
}
