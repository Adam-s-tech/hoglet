//! Claim 2 under concurrency: many writers, background compaction, concurrent
//! readers, person erasures, and repeated SIGKILL / graceful restarts of the
//! real binary on one data directory.
//!
//! Invariants after every restart and at the end:
//!   acknowledged ⊆ stored ∪ erased, stored has no duplicates, nothing stored
//!   was not sent or was altered, erased persons are absent everywhere
//!   (events and identity), the lake and catalog are consistent, and readers
//!   never saw an error while compaction ran.
//!
//! Run: `cargo test --features fault-injection --test claim2_concurrency`
//! `HOGLET_FAULT_SEEDS` = independent runs; `HOGLET_CONC_CYCLES` = restarts per run.
#![cfg(feature = "fault-injection")]

mod claim2_support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use claim2_support::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn cycles() -> u64 {
    std::env::var("HOGLET_CONC_CYCLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4)
}

fn erase(port: u16, project: &Project, victim: &str) -> Outcome {
    http(
        "POST",
        port,
        &format!("/api/projects/{}/persons/{victim}/erase", project.id),
        None,
        Some(&project.cookie),
    )
    .0
}

struct Reader {
    stop: Arc<AtomicBool>,
    ok: Arc<AtomicU64>,
    bad: Arc<std::sync::Mutex<Vec<String>>>,
    handle: std::thread::JoinHandle<()>,
}

impl Reader {
    fn start(port: u16, project: &Project) -> Reader {
        let stop = Arc::new(AtomicBool::new(false));
        let ok = Arc::new(AtomicU64::new(0));
        let bad = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (stop2, ok2, bad2) = (stop.clone(), ok.clone(), bad.clone());
        let path = format!("/api/projects/{}/events?event={EVENT}&limit=100", project.id);
        let cookie = project.cookie.clone();
        let handle = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match http("GET", port, &path, None, Some(&cookie)) {
                    (Outcome::Acked, _) => {
                        ok2.fetch_add(1, Ordering::Relaxed);
                    }
                    (Outcome::Unknown, _) => {}
                    (other, body) => bad2.lock().unwrap().push(format!("{other:?} {body}")),
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        Reader { stop, ok, bad, handle }
    }

    fn stop(self) -> (u64, Vec<String>) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().expect("reader thread");
        let bad = self.bad.lock().unwrap().clone();
        (self.ok.load(Ordering::Relaxed), bad)
    }
}

fn run(seed: u64) -> String {
    let mut rng = StdRng::seed_from_u64(seed ^ 0xc0ffee);
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let ledger = Arc::new(Ledger::default());
    let mut erased: BTreeSet<String> = BTreeSet::new();
    let mut unconfirmed: Vec<String> = Vec::new();
    let mut project: Option<Project> = None;
    let mut reads = 0;
    let mut hard_kills = 0;
    let mut erasures = 0;
    let mut slowest_erase_ms = 0_u128;

    let total = cycles();
    for cycle in 0..total {
        let small = rng.gen_bool(0.5);
        let env: Vec<(&str, String)> = if small {
            vec![("HOGLET_FAULT_WAL_SEGMENT_BYTES", rng.gen_range(3_000..30_000).to_string())]
        } else {
            Vec::new()
        };
        let mut server = Server::spawn(data, &env);
        let project = project.get_or_insert_with(|| setup(server.port));

        // Finish erasures an earlier kill left unconfirmed, then check state.
        for victim in std::mem::take(&mut unconfirmed) {
            match erase(server.port, project, &victim) {
                // 404: the identity part had already committed; startup must
                // have finished the events, which `reconcile` verifies below.
                Outcome::Acked | Outcome::Rejected(404) => {
                    erased.insert(victim);
                }
                other => panic!("seed {seed} cycle {cycle}: retrying erasure of {victim}: {other:?}"),
            }
        }
        check_consistent(data, &erased).unwrap_or_else(|e| panic!("seed {seed} cycle {cycle} after restart: {e}"));
        reconcile(data, &ledger, &erased).unwrap_or_else(|e| panic!("seed {seed} cycle {cycle} after restart: {e}"));

        // Victims: acknowledged events on an old day (compactable partition).
        let victims: Vec<String> = (0..rng.gen_range(1..=3)).map(|k| format!("victim-s{seed}-c{cycle}-{k}")).collect();
        for victim in &victims {
            for _ in 0..rng.gen_range(1..=3) {
                let plans: Vec<_> = (0..rng.gen_range(1..=5)).map(|_| plan_event(EVENT, victim.clone(), 2)).collect();
                assert_eq!(post_batch(server.port, &ledger, &plans), Outcome::Acked);
            }
        }

        let writers = Writers::start(
            server.port,
            &ledger,
            seed * 100 + cycle,
            rng.gen_range(3..=6),
            rng.gen_range(1..=10),
            rng.gen_range(10..=40),
            "",
        );
        let reader = Reader::start(server.port, project);
        std::thread::sleep(Duration::from_millis(rng.gen_range(600..2_000)));

        // Erase while writers and compaction run.
        for victim in &victims {
            let started = std::time::Instant::now();
            let outcome = erase(server.port, project, victim);
            slowest_erase_ms = slowest_erase_ms.max(started.elapsed().as_millis());
            match outcome {
                Outcome::Acked => {
                    erased.insert(victim.clone());
                    erasures += 1;
                }
                other => panic!("seed {seed} cycle {cycle}: erasing {victim} on a healthy server: {other:?}\n{}\ndata kept at {}", server.log(), directory.keep().display()),
            }
        }
        // One more erasure that the kill below may interrupt.
        let doomed = format!("victim-s{seed}-c{cycle}-doomed");
        let plans: Vec<_> = (0..3).map(|_| plan_event(EVENT, doomed.clone(), 2)).collect();
        assert_eq!(post_batch(server.port, &ledger, &plans), Outcome::Acked);
        let doomed_port = server.port;
        let doomed_project = Project {
            id: project.id.clone(),
            cookie: project.cookie.clone(),
        };
        let doomed_name = doomed.clone();
        let interrupted = std::thread::spawn(move || erase(doomed_port, &doomed_project, &doomed_name));

        std::thread::sleep(Duration::from_millis(rng.gen_range(0..1_200)));
        if rng.gen_bool(0.75) {
            server.sigkill();
            hard_kills += 1;
        } else {
            server.stop_gracefully();
        }
        writers.stop();
        let outcome = interrupted.join().expect("erase thread");
        let (ok, bad) = reader.stop();
        reads += ok;
        assert!(bad.is_empty(), "seed {seed} cycle {cycle}: readers saw errors while compaction ran: {bad:?}");
        match outcome {
            Outcome::Acked => {
                erased.insert(doomed);
                erasures += 1;
            }
            _ => unconfirmed.push(doomed),
        }
    }

    // Final restart: finish what was interrupted, then the full audit.
    let mut server = Server::spawn(data, &[]);
    let project = project.expect("project");
    for victim in std::mem::take(&mut unconfirmed) {
        match erase(server.port, &project, &victim) {
            Outcome::Acked | Outcome::Rejected(404) => {
                erased.insert(victim);
            }
            other => panic!("seed {seed}: final erasure retry of {victim}: {other:?}"),
        }
    }
    wait_until_queryable(server.port, &project, &ledger, &erased);
    // The identity side of every erased person is gone too.
    let (ids, _, _) = identity_view(data);
    assert!(ids.is_disjoint(&erased), "seed {seed}: erased persons still have identity rows");
    server.stop_gracefully();
    let report = reconcile(data, &ledger, &erased).unwrap_or_else(|e| panic!("seed {seed} final: {e}"));
    check_consistent(data, &erased).unwrap_or_else(|e| panic!("seed {seed} final: {e}"));
    assert!(report.acked > 100, "the load generator acknowledged too little ({})", report.acked);
    format!(
        "CONCURRENCY seed={seed} cycles={total} hard_kills={hard_kills} erasures={erasures} slowest_erase_ms={slowest_erase_ms} reads_ok={reads} acked={} stored={} erased_persons={} result=PASS",
        report.acked,
        report.stored,
        erased.len()
    )
}

#[test]
fn writers_compaction_erasure_readers_and_restarts_keep_every_invariant() {
    for seed in 0..seeds() {
        let line = run(seed);
        println!("{line}");
        if let Ok(path) = std::env::var("HOGLET_MATRIX_OUT") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{line}");
            }
        }
    }
}
