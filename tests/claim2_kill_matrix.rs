//! Claim 2, adversarial kill-point matrix.
//!
//! A real `hoglet` process (built with `--features fault-injection`) is armed
//! to SIGKILL itself at one named point of the write path — mid-record,
//! between write and fsync, between fsync and ack, inside segment sealing,
//! inside publication, WAL reclamation, compaction and erasure. The harness
//! then restarts the real binary on the same data directory and reconciles
//! what clients were told (HTTP 200) against what is stored:
//!
//! * every acknowledged event is stored exactly once, byte-for-byte as sent;
//! * nothing is stored that was never sent;
//! * no orphan files, catalog row counts match Parquet, identity matches the
//!   events, repeated recovery changes nothing;
//! * every acknowledged event is queryable through the API;
//! * ingestion resumes.
//!
//! Run: `cargo test --features fault-injection --test claim2_kill_matrix`
//! `HOGLET_FAULT_SEEDS=<n>` sets randomized iterations per point (default 2).
//! `HOGLET_MATRIX_OUT=<file>` appends one line per run for reporting.
#![cfg(feature = "fault-injection")]

mod claim2_support;

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use claim2_support::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ingest,
    /// Erase a person after the crash point is armed; `atomic_only` means the
    /// crash precedes the erasure commit, so all-or-nothing is the contract.
    Erase { before_commit: bool },
}

struct Plan {
    point: &'static str,
    /// Fault spec for this seed, plus extra env.
    spec: String,
    segment_bytes: Option<u64>,
    kind: Kind,
}

fn log_line(line: &str) {
    println!("{line}");
    if let Ok(path) = std::env::var("HOGLET_MATRIX_OUT") {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn run(plan: &Plan, seed: u64) -> Result<Report, String> {
    let directory = tempfile::tempdir().expect("temporary data directory");
    let result = run_in(plan, seed, directory.path());
    match result {
        Ok(report) => Ok(report),
        // Keep the evidence of a failure for post-mortem.
        Err(error) => Err(format!("{error}\n  data kept at {}", directory.keep().display())),
    }
}

fn run_in(plan: &Plan, seed: u64, data: &std::path::Path) -> Result<Report, String> {
    let mut env = vec![("HOGLET_FAULT", plan.spec.clone())];
    if let Some(bytes) = plan.segment_bytes {
        env.push(("HOGLET_FAULT_WAL_SEGMENT_BYTES", bytes.to_string()));
    }
    let mut server = Server::spawn(data, &env);
    let project = setup(server.port);
    let ledger = Arc::new(Ledger::default());
    let mut rng = StdRng::seed_from_u64(seed ^ 0x9e37_79b9);
    let mut erased: BTreeSet<String> = BTreeSet::new();
    let mut victims: Vec<String> = Vec::new();

    match plan.kind {
        Kind::Ingest => {
            let writers = Writers::start(
                server.port,
                &ledger,
                seed,
                rng.gen_range(1..=4),
                rng.gen_range(1..=25),
                rng.gen_range(0..=40),
                "",
            );
            let died = server.wait_for_death(Duration::from_secs(90));
            writers.stop();
            if !died {
                return Err(format!("fault {} never fired within 90 s", plan.spec));
            }
        }
        Kind::Erase { .. } => {
            victims = (0..2).map(|k| format!("victim-{seed}-{k}")).collect();
            for victim in &victims {
                for _ in 0..rng.gen_range(1..=4) {
                    let size = rng.gen_range(1..=6);
                    let plans: Vec<_> = (0..size)
                        .map(|_| plan_event(EVENT, victim.clone(), 2))
                        .collect();
                    assert_eq!(post_batch(server.port, &ledger, &plans), Outcome::Acked);
                    let bystanders: Vec<_> = (0..size)
                        .map(|i| plan_event(EVENT, format!("bystander-{}", i % 5), 2))
                        .collect();
                    assert_eq!(post_batch(server.port, &ledger, &bystanders), Outcome::Acked);
                }
            }
            let acked = ledger.acked_count() as u64;
            wait_until("pre-erasure events to be published", Duration::from_secs(60), || {
                status_stored_events(server.port, &project).is_some_and(|rows| rows >= acked)
            });
            let (outcome, _) = http(
                "POST",
                server.port,
                &format!("/api/projects/{}/persons/{}/erase", project.id, victims[0]),
                None,
                Some(&project.cookie),
            );
            if outcome == Outcome::Acked {
                return Err(format!("fault {} never fired: erase succeeded", plan.spec));
            }
            if !server.wait_for_death(Duration::from_secs(30)) {
                return Err(format!("erase answered {outcome:?} but the server did not die"));
            }
        }
    }

    let log = server
        .killed_by_fault()
        .ok_or_else(|| format!("server died, but not from the injected fault:\n{}", server.log()))?;
    let aborted_at = log
        .lines()
        .rev()
        .find_map(|line| line.split("aborting at ").nth(1))
        .unwrap_or("?")
        .to_owned();
    if !aborted_at.starts_with(plan.point) {
        return Err(format!("died at {aborted_at}, expected {}", plan.point));
    }
    drop(server);

    // ---- restart on the same directory, no faults armed
    let mut server = Server::spawn(data, &[]);
    let mut check_atomic_then_retry = false;
    match plan.kind {
        Kind::Erase { before_commit: false } => {
            erased.insert(victims[0].clone());
        }
        Kind::Erase { before_commit: true } => check_atomic_then_retry = true,
        Kind::Ingest => {}
    }
    check_consistent(data, &erased).map_err(|e| format!("after restart: {e}"))?;
    wait_until_queryable(server.port, &project, &ledger, &erased);
    let api = feed_uuids(server.port, &project, EVENT);
    let direct = reconcile(data, &ledger, &erased).map_err(|e| format!("after restart: {e}"))?;
    if !direct_matches_api(data, &api) {
        return Err("the API and DuckDB over the lake files disagree".into());
    }

    if check_atomic_then_retry {
        // Crashed before the erasure committed: nothing may have changed, and
        // the client's retry must now succeed and finish the job.
        let (outcome, body) = http(
            "POST",
            server.port,
            &format!("/api/projects/{}/persons/{}/erase", project.id, victims[0]),
            None,
            Some(&project.cookie),
        );
        if outcome != Outcome::Acked {
            return Err(format!("retrying the erasure failed: {outcome:?} {body}"));
        }
        erased.insert(victims[0].clone());
    }
    if matches!(plan.kind, Kind::Erase { .. }) {
        // A second, ordinary erasure on the recovered server works too.
        let (outcome, body) = http(
            "POST",
            server.port,
            &format!("/api/projects/{}/persons/{}/erase", project.id, victims[1]),
            None,
            Some(&project.cookie),
        );
        if outcome != Outcome::Acked {
            return Err(format!("erasing on the recovered server failed: {outcome:?} {body}"));
        }
        erased.insert(victims[1].clone());
    }

    // ---- ingestion resumes
    let fresh: Vec<_> = (0..5).map(|i| plan_event(EVENT, format!("after-restart-{i}"), 0)).collect();
    if post_batch(server.port, &ledger, &fresh) != Outcome::Acked {
        return Err("ingestion did not resume after recovery".into());
    }
    wait_until_queryable(server.port, &project, &ledger, &erased);
    server.stop_gracefully();

    let report = reconcile(data, &ledger, &erased).map_err(|e| format!("after graceful stop: {e}"))?;
    check_consistent(data, &erased).map_err(|e| format!("after graceful stop: {e}"))?;

    // ---- recovery is idempotent: another restart changes nothing
    let mut again = Server::spawn(data, &[]);
    again.stop_gracefully();
    let repeat = reconcile(data, &ledger, &erased).map_err(|e| format!("after second restart: {e}"))?;
    if repeat.stored != report.stored {
        return Err(format!("a further restart changed the stored count {} -> {}", report.stored, repeat.stored));
    }
    let _ = direct;
    Ok(Report {
        aborted_at,
        ..report
    })
}

fn direct_matches_api(data: &std::path::Path, api: &BTreeSet<String>) -> bool {
    let direct: BTreeSet<String> = stored_rows(data)
        .expect("rows")
        .into_iter()
        .map(|row| row.uuid)
        .collect();
    &direct == api
}

fn matrix(name: &str, mut make: impl FnMut(&mut StdRng, u64) -> Plan) {
    let mut failures = Vec::new();
    for seed in 0..seeds() {
        let mut rng = StdRng::seed_from_u64(seed.wrapping_mul(7919) + name.len() as u64);
        let plan = make(&mut rng, seed);
        match run(&plan, seed) {
            Ok(report) => log_line(&format!(
                "MATRIX point={} seed={seed} spec={} result=PASS acked={} sent={} stored={} unacked_but_stored={} files={}",
                plan.point, plan.spec, report.acked, report.sent, report.stored, report.unacked_but_stored, report.lake_files
            )),
            Err(error) => {
                log_line(&format!(
                    "MATRIX point={} seed={seed} spec={} result=FAIL {error}",
                    plan.point, plan.spec
                ));
                failures.push(format!("seed {seed} ({}): {error}", plan.spec));
            }
        }
    }
    assert!(failures.is_empty(), "kill point {name}:\n{}", failures.join("\n"));
}

fn small_segments(rng: &mut StdRng) -> Option<u64> {
    rng.gen_bool(0.5).then(|| rng.gen_range(2_000..40_000))
}

macro_rules! ingest_point {
    ($test:ident, $point:literal, |$rng:ident| $spec:expr) => {
        #[test]
        fn $test() {
            matrix($point, |$rng, _seed| Plan {
                point: $point,
                spec: $spec,
                segment_bytes: small_segments($rng),
                kind: Kind::Ingest,
            });
        }
    };
}

macro_rules! erase_point {
    ($test:ident, $point:literal, $before:expr) => {
        #[test]
        fn $test() {
            matrix($point, |_rng, _seed| Plan {
                point: $point,
                spec: format!("{}=abort", $point),
                segment_bytes: None,
                kind: Kind::Erase { before_commit: $before },
            });
        }
    };
}

// WAL write path.
ingest_point!(wal_write_torn_mid_record, "wal.write", |rng| {
    format!("wal.write=torn:{}@{}", rng.gen_range(1..=99), rng.gen_range(1..=30))
});
ingest_point!(wal_after_write_before_fsync_process_crash, "wal.sync.before", |rng| {
    format!("wal.sync.before=abort@{}", rng.gen_range(1..=30))
});
ingest_point!(wal_after_write_before_fsync_power_loss, "wal.sync.before", |rng| {
    format!("wal.sync.before=lose:{}@{}", rng.gen_range(0..=99), rng.gen_range(1..=30))
});
ingest_point!(wal_after_fsync_before_ack, "sink.after_fsync_before_ack", |rng| {
    format!("sink.after_fsync_before_ack=abort@{}", rng.gen_range(1..=30))
});
ingest_point!(wal_seal_before_rename, "wal.seal.before_rename", |rng| {
    format!("wal.seal.before_rename=abort@{}", rng.gen_range(1..=3))
});
ingest_point!(wal_seal_after_rename_not_durable, "wal.seal.after_rename", |rng| {
    format!("wal.seal.after_rename=revert@{}", rng.gen_range(1..=3))
});
ingest_point!(wal_seal_after_rename_before_dirsync, "wal.seal.after_rename", |rng| {
    format!("wal.seal.after_rename=abort@{}", rng.gen_range(1..=3))
});
ingest_point!(wal_seal_after_dirsync, "wal.seal.after_dirsync", |rng| {
    format!("wal.seal.after_dirsync=abort@{}", rng.gen_range(1..=3))
});
// Publication.
ingest_point!(publisher_after_parquet_temp_write, "pub.after_tmp_write", |rng| {
    format!("pub.after_tmp_write=abort@{}", rng.gen_range(1..=4))
});
ingest_point!(publisher_after_rename_before_catalog_commit, "pub.after_rename", |rng| {
    format!("pub.after_rename=abort@{}", rng.gen_range(1..=4))
});
ingest_point!(publisher_before_catalog_commit, "pub.before_commit", |rng| {
    format!("pub.before_commit=abort@{}", rng.gen_range(1..=3))
});
ingest_point!(publisher_after_catalog_commit_before_wal_reclaim, "pub.after_commit", |rng| {
    format!("pub.after_commit=abort@{}", rng.gen_range(1..=3))
});
ingest_point!(publisher_mid_wal_reclaim, "wal.reclaim.after_remove", |rng| {
    format!("wal.reclaim.after_remove=abort@{}", rng.gen_range(1..=3))
});
ingest_point!(publisher_after_wal_reclaim, "pub.after_reclaim", |rng| {
    format!("pub.after_reclaim=abort@{}", rng.gen_range(1..=3))
});
// Compaction.
ingest_point!(compactor_after_output_written_before_commit, "compact.after_tmp", |rng| {
    format!("compact.after_tmp=abort@{}", rng.gen_range(1..=2))
});
ingest_point!(compactor_after_rename_before_commit, "compact.after_rename", |rng| {
    format!("compact.after_rename=abort@{}", rng.gen_range(1..=2))
});
ingest_point!(compactor_after_commit_before_activate, "compact.after_commit", |rng| {
    format!("compact.after_commit=abort@{}", rng.gen_range(1..=2))
});
ingest_point!(compactor_after_commit_before_graveyard_sweep, "compact.before_sweep", |rng| {
    format!("compact.before_sweep=abort@{}", rng.gen_range(1..=2))
});
ingest_point!(graveyard_sweep_after_unlink_before_catalog_delete, "lake.sweep.after_unlink", |rng| {
    format!("lake.sweep.after_unlink=abort@{}", rng.gen_range(1..=2))
});
// Erasure.
erase_point!(erasure_before_sqlite_commit, "erase.before_sqlite_commit", true);
erase_point!(erasure_after_sqlite_commit_before_lake_rewrite, "erase.after_sqlite_commit", false);
erase_point!(erasure_after_rewrite_written_before_commit, "erase.after_tmp", false);
erase_point!(erasure_after_rename_before_commit, "erase.after_rename", false);
erase_point!(erasure_after_lake_commit, "erase.after_commit", false);

/// The mutant check: a WAL whose fsync is a no-op acknowledges events that a
/// power cut then destroys. The matrix harness must catch it; if this test
/// ever passes with the mutant armed, the matrix proves nothing.
#[test]
fn harness_detects_a_wal_that_acks_before_fsync() {
    let directory = tempfile::tempdir().expect("temporary data directory");
    let data = directory.path();
    let env = vec![
        ("HOGLET_FAULT_MUTANT", "skip_fsync".to_owned()),
        ("HOGLET_FAULT", "wal.sync.before=lose:0@40".to_owned()),
    ];
    let mut server = Server::spawn(data, &env);
    let _project = setup(server.port);
    let ledger = Arc::new(Ledger::default());
    let writers = Writers::start(server.port, &ledger, 7, 2, 5, 5, "");
    assert!(server.wait_for_death(Duration::from_secs(60)), "fault never fired");
    writers.stop();
    drop(server);
    let _restarted = Server::spawn(data, &[]);
    let verdict = reconcile(data, &ledger, &BTreeSet::new());
    let error = verdict.expect_err("the harness failed to notice acknowledged events vanishing");
    assert!(error.contains("acknowledged events were lost"), "unexpected verdict: {error}");
}
