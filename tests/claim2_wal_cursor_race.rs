//! Claim 2 / liveness: the publisher may read the WAL directory at any
//! instant, including the instant between the writer renaming the full
//! segment to `.wal` and creating the next `.open` one. A checkpoint taken in
//! that window must still name a segment that exists after reclamation.
//!
//! Before the fix the checkpoint stayed at the *end of the segment just
//! published*; reclamation then deleted that segment, and the next
//! publication failed forever with "cursor segment does not exist" — also at
//! every restart, so acknowledged events were stuck in the WAL and the server
//! could not start.

use chrono::{TimeZone, Utc};
use hoglet::pipeline::wal::{CapturedBatch, WalConfig, WalCursor, WriteAheadLog};
use serde_json::Map;
use uuid::Uuid;

fn batch(sequence: u128) -> CapturedBatch {
    CapturedBatch::new(vec![hoglet::capture::event::CapturedEvent {
        uuid: Uuid::from_u128(sequence),
        event: "pageview".into(),
        distinct_id: format!("person-{sequence}"),
        token: "phc_testproject".into(),
        timestamp: Utc.timestamp_opt(sequence as i64, 0).single().unwrap(),
        properties: Map::new(),
    }])
    .unwrap()
}

#[test]
fn checkpoint_taken_between_segment_seal_and_next_segment_creation_stays_valid() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wal, _) = WriteAheadLog::open(dir.path(), WalConfig::default()).unwrap();
    wal.append(batch(1)).unwrap();
    wal.seal().unwrap();
    // The writer's mid-seal state as the publisher can observe it: segment 1
    // is sealed, segment 2's `.open` file does not exist yet.
    std::fs::remove_file(dir.path().join(format!("{:016}.open", 2))).unwrap();

    let reader = wal.reader();
    let mut window = reader.read_window(WalCursor::origin(), 1 << 20).unwrap();
    assert_eq!(window.by_ref().count(), 1);
    let checkpoint = window.next_cursor();

    reader.reclaim_through(checkpoint).unwrap();
    // The writer finishes sealing and keeps appending.
    std::fs::File::create(dir.path().join(format!("{:016}.open", 2))).unwrap();

    reader
        .read_window(checkpoint, 1 << 20)
        .unwrap_or_else(|error| panic!("checkpoint {checkpoint:?} is unusable: {error}"));
}

/// The real thing: a writer sealing a tiny segment on nearly every append
/// while a publisher thread walks the log the way `Publisher` does (read a
/// window, move the checkpoint, reclaim). Every record must be seen exactly
/// once, in order, and no step may fail — least of all with a bogus
/// "corruption" report on a healthy log.
#[test]
fn a_publisher_walking_the_wal_while_the_writer_seals_never_errors_and_sees_every_record_once() {
    const RECORDS: u128 = 3_000;
    let dir = tempfile::tempdir().unwrap();
    let config = WalConfig::new(150, 1 << 20).unwrap();
    let (mut wal, _) = WriteAheadLog::open(dir.path(), config).unwrap();
    let reader = wal.reader();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_writer = done.clone();

    let writer = std::thread::spawn(move || {
        for sequence in 1..=RECORDS {
            wal.append(batch(sequence)).unwrap();
        }
        wal.seal().unwrap();
        done_writer.store(true, std::sync::atomic::Ordering::SeqCst);
        wal
    });

    let mut checkpoint = WalCursor::origin();
    let mut seen: Vec<u128> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    loop {
        let finished = done.load(std::sync::atomic::Ordering::SeqCst);
        match reader.read_window(checkpoint, 1 << 20) {
            Ok(mut window) => {
                let mut failed = false;
                for record in window.by_ref() {
                    match record {
                        Ok(record) => seen.push(record.batch.events[0].uuid.as_u128()),
                        Err(error) => {
                            errors.push(format!("reading: {error}"));
                            failed = true;
                        }
                    }
                }
                if !failed {
                    checkpoint = window.next_cursor();
                    if let Err(error) = reader.reclaim_through(checkpoint) {
                        errors.push(format!("reclaiming through {checkpoint:?}: {error}"));
                    }
                }
            }
            Err(error) => errors.push(format!("opening a window at {checkpoint:?}: {error}")),
        }
        if finished && seen.len() as u128 >= RECORDS {
            break;
        }
        assert!(errors.len() < 50, "too many errors: {errors:?}");
    }
    let _wal = writer.join().unwrap();
    assert!(
        errors.is_empty(),
        "publisher steps failed on a healthy log: {:?}",
        &errors[..errors.len().min(5)]
    );
    let expected: Vec<u128> = (1..=RECORDS).collect();
    assert_eq!(seen, expected, "every record exactly once, in order");
}
