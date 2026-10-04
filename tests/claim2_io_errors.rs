//! Claim 2, disk-full and I/O errors: a failing write or fsync must never
//! produce a false 2xx. The WAL poisons itself, capture answers 503 (which
//! SDKs retry) for everything after, and after a restart every event that was
//! acknowledged before the failure is stored exactly once.
//!
//! Run: `cargo test --features fault-injection --test claim2_io_errors`
//! `HOGLET_FAULT_SEEDS=<n>` sets randomized iterations per scenario.
#![cfg(feature = "fault-injection")]

mod claim2_support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use claim2_support::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// WAL failure: poisoned, 503 forever, nothing false, recovery intact.
fn wal_failure(point: &str, seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x51ed);
    let spec = format!("{point}=fail@{}", rng.gen_range(1..=15));
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let mut server = Server::spawn(data, &[("HOGLET_FAULT", spec.clone())]);
    let project = setup(server.port);
    let ledger = Arc::new(Ledger::default());
    let writers = Writers::start(server.port, &ledger, seed, rng.gen_range(1..=4), rng.gen_range(1..=20), 5, "");
    wait_until(&format!("{spec} to surface as 503"), Duration::from_secs(60), || {
        ledger.retryable.load(Ordering::Relaxed) >= 10 || writers.finished()
    });
    // Keep hammering a little after the failure: still nothing may be acked.
    std::thread::sleep(Duration::from_millis(300));
    writers.stop();

    assert!(ledger.retryable.load(Ordering::Relaxed) >= 10, "{spec}: the failure never surfaced");
    assert_eq!(
        ledger.acks_after_failure.load(Ordering::SeqCst),
        0,
        "{spec}: a request sent after a 503 was acknowledged 2xx (false ack from a poisoned WAL)"
    );
    assert_eq!(
        *ledger.failure_codes.lock().unwrap(),
        BTreeSet::from([503]),
        "{spec}: failures must be retryable 503s, never 4xx (never retried) or other 5xx"
    );
    assert!(server.child.try_wait().expect("poll").is_none(), "{spec}: the server died instead of shedding load");
    server.stop_gracefully();
    // The failed write left frames (or half a frame) behind; shutdown could
    // not have changed the contract.
    let mut restarted = Server::spawn(data, &[]);
    check_consistent(data, &BTreeSet::new()).unwrap_or_else(|e| panic!("{spec}: {e}"));
    let report = reconcile(data, &ledger, &BTreeSet::new()).unwrap_or_else(|e| panic!("{spec}: {e}"));
    wait_until_queryable(restarted.port, &project, &ledger, &BTreeSet::new());
    // Ingestion resumes with a healthy disk.
    let fresh: Vec<_> = (0..3).map(|i| plan_event(EVENT, format!("after-{i}"), 0)).collect();
    assert_eq!(post_batch(restarted.port, &ledger, &fresh), Outcome::Acked, "{spec}: ingestion did not resume");
    wait_until_queryable(restarted.port, &project, &ledger, &BTreeSet::new());
    restarted.stop_gracefully();
    reconcile(data, &ledger, &BTreeSet::new()).unwrap_or_else(|e| panic!("{spec}: {e}"));
    let line = format!(
        "IOERR spec={spec} seed={seed} result=PASS acked={} stored={} refused_503={} false_acks=0",
        report.acked,
        report.stored,
        ledger.retryable.load(Ordering::Relaxed)
    );
    log_line(&line);
}

#[test]
fn a_failing_fsync_never_produces_a_false_ack() {
    for seed in 0..seeds() {
        wal_failure("wal.sync", seed);
    }
}

#[test]
fn a_full_disk_mid_write_never_produces_a_false_ack_and_leaves_a_repairable_tail() {
    for seed in 0..seeds() {
        wal_failure("wal.write", seed);
    }
}

#[test]
fn a_failing_segment_seal_never_produces_a_false_ack() {
    // Small segments make every few writes seal.
    for seed in 0..seeds() {
        let mut rng = StdRng::seed_from_u64(seed);
        let spec = format!("wal.seal.sync=fail@{}", rng.gen_range(1..=3));
        let directory = tempfile::tempdir().expect("data directory");
        let data = directory.path();
        let env = [
            ("HOGLET_FAULT", spec.clone()),
            ("HOGLET_FAULT_WAL_SEGMENT_BYTES", "3000".to_owned()),
        ];
        let mut server = Server::spawn(data, &env);
        let project = setup(server.port);
        let ledger = Arc::new(Ledger::default());
        let writers = Writers::start(server.port, &ledger, seed, 2, 10, 5, "");
        wait_until("seal failure to surface", Duration::from_secs(60), || {
            ledger.retryable.load(Ordering::Relaxed) >= 5 || writers.finished()
        });
        writers.stop();
        assert_eq!(ledger.acks_after_failure.load(Ordering::SeqCst), 0, "{spec}: false ack after failure");
        assert_eq!(*ledger.failure_codes.lock().unwrap(), BTreeSet::from([503]), "{spec}");
        server.stop_gracefully();
        let restarted = Server::spawn(data, &[]);
        check_consistent(data, &BTreeSet::new()).unwrap_or_else(|e| panic!("{spec}: {e}"));
        reconcile(data, &ledger, &BTreeSet::new()).unwrap_or_else(|e| panic!("{spec}: {e}"));
        wait_until_queryable(restarted.port, &project, &ledger, &BTreeSet::new());
    }
}

/// A publisher that cannot write Parquet (disk full) must not hurt capture:
/// the WAL keeps every acknowledged event, publication retries, and once the
/// disk recovers everything appears exactly once.
#[test]
fn a_publisher_that_cannot_write_parquet_loses_nothing_and_catches_up() {
    for seed in 0..seeds() {
        let directory = tempfile::tempdir().expect("data directory");
        let data = directory.path();
        // Fail the first 4 publication attempts, then recover.
        let mut server = Server::spawn(data, &[("HOGLET_FAULT", "pub.parquet_write=fail@1x4".to_owned())]);
        let project = setup(server.port);
        let ledger = Arc::new(Ledger::default());
        let writers = Writers::start(server.port, &ledger, seed, 2, 10, 20, "");
        std::thread::sleep(Duration::from_secs(3));
        writers.stop();
        assert_eq!(ledger.retryable.load(Ordering::Relaxed), 0, "capture must not be affected by publication failures");
        assert!(ledger.acked_count() > 0);
        // The publisher retries on its own tick; the disk "recovers".
        wait_until_queryable(server.port, &project, &ledger, &BTreeSet::new());
        server.stop_gracefully();
        let report = reconcile(data, &ledger, &BTreeSet::new()).expect("reconcile");
        check_consistent(data, &BTreeSet::new()).expect("consistent");
        assert_eq!(report.stored, report.sent, "every sent event is stored: all were acked");
    }
}

/// A publisher that never succeeds: events stay in the WAL, acknowledged,
/// and a restart on a healthy disk publishes every one exactly once.
#[test]
fn a_permanently_failing_publisher_leaves_acknowledged_events_in_the_wal_for_the_next_start() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let mut server = Server::spawn(data, &[("HOGLET_FAULT", "pub.parquet_write=fail".to_owned())]);
    let _project = setup(server.port);
    let ledger = Arc::new(Ledger::default());
    let writers = Writers::start(server.port, &ledger, 1, 2, 10, 20, "");
    std::thread::sleep(Duration::from_secs(3));
    writers.stop();
    assert_eq!(ledger.retryable.load(Ordering::Relaxed), 0);
    server.sigkill();
    assert!(
        stored_rows(data).expect("rows").len() < ledger.acked_count(),
        "the publisher was supposed to be broken"
    );
    let _restarted = Server::spawn(data, &[]);
    let report = reconcile(data, &ledger, &BTreeSet::new()).expect("every acknowledged event is recovered");
    assert_eq!(report.stored, report.sent);
    check_consistent(data, &BTreeSet::new()).expect("consistent");
}
