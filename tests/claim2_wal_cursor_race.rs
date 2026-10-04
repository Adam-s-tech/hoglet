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
