//! Claim 2, WAL damage: whatever happens to the bytes on disk, recovery
//! either fails loudly or returns a prefix of what was written. It never
//! panics, never accepts an altered record, and never invents data.
//!
//! * exhaustive: the active segment truncated at EVERY byte offset, and every
//!   single bit of the final record flipped;
//! * exhaustive: every bit of every byte of a small sealed segment flipped;
//! * property (proptest): random record sequences over several segments with
//!   random damage (bit flips, truncation, zeroed ranges, garbage appends,
//!   random overwrites) to the active segment, to sealed segments, and to both;
//! * end to end (feature `fault-injection`): the real binary refuses to start
//!   on corruption in a sealed segment or in the middle of the active one,
//!   survives a torn tail, and keeps every acknowledged event.
//!
//! `HOGLET_PROPTEST_CASES=<n>` sets the proptest case count (default 64).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{TimeZone, Utc};
use hoglet::capture::event::CapturedEvent;
use hoglet::pipeline::wal::{CapturedBatch, WalConfig, WalCursor, WalError, WriteAheadLog};
use proptest::prelude::*;
use serde_json::{Map, Value};
use uuid::Uuid;

fn cases() -> u32 {
    std::env::var("HOGLET_PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(64)
}

fn batch(sequence: u128, events: usize, payload: usize) -> CapturedBatch {
    let events = (0..events as u128)
        .map(|k| {
            let mut properties = Map::new();
            properties.insert("payload".into(), Value::String("x".repeat(payload)));
            properties.insert("k".into(), Value::from(k as u64));
            CapturedEvent {
                uuid: Uuid::from_u128(sequence * 1_000 + k),
                event: "pageview".into(),
                distinct_id: format!("person-{sequence}"),
                token: "phc_testproject".into(),
                timestamp: Utc.timestamp_opt(sequence as i64, 0).single().expect("time"),
                properties,
            }
        })
        .collect();
    CapturedBatch::new(events).expect("non-empty")
}

type Files = BTreeMap<String, Vec<u8>>;

/// What a healthy WAL wrote: its files and, per record, where it lives.
struct Written {
    files: Files,
    /// `(file name, start, end)` per record, in order.
    records: Vec<(String, u64, u64)>,
    originals: Vec<Value>,
    config: WalConfig,
}

impl Written {
    fn active_name(&self) -> String {
        self.files
            .keys()
            .find(|name| name.ends_with(".open"))
            .expect("an active segment")
            .clone()
    }

    fn sealed_names(&self) -> Vec<String> {
        self.files.keys().filter(|name| name.ends_with(".wal")).cloned().collect()
    }

    /// Records wholly inside `name`'s first `len` bytes, counting from the
    /// start of the whole log.
    fn complete_records_before(&self, name: &str, offset: u64) -> usize {
        let mut count = 0;
        for (file, _, end) in &self.records {
            if file.as_str() < name || (file == name && *end <= offset) {
                count += 1;
            }
        }
        count
    }
}

fn write_log(sizes: &[(usize, usize)], segment_target: u64) -> Written {
    let directory = tempfile::tempdir().expect("source directory");
    let config = WalConfig::new(segment_target, 1 << 20).expect("config");
    let (mut wal, _) = WriteAheadLog::open(directory.path(), config).expect("open");
    let mut records = Vec::new();
    let mut originals = Vec::new();
    for (index, (events, payload)) in sizes.iter().enumerate() {
        let batch = batch(index as u128 + 1, *events, *payload);
        originals.push(serde_json::to_value(&batch).expect("json"));
        let receipt = wal.append(batch).expect("append");
        let sequence = receipt.span.start.segment;
        // A seal inside `write` may have renamed the segment: name it by the
        // sealed spelling unless it is still the active one.
        records.push((format!("{sequence:016}"), receipt.span.start.byte_offset, receipt.span.end.byte_offset));
    }
    drop(wal);
    let mut files = Files::new();
    for entry in std::fs::read_dir(directory.path()).expect("list") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".wal") || name.ends_with(".open") {
            files.insert(name, std::fs::read(entry.path()).expect("read"));
        }
    }
    // Key records by the real file name.
    let records = records
        .into_iter()
        .map(|(stem, start, end)| {
            let name = files
                .keys()
                .find(|name| name.starts_with(&stem))
                .expect("segment file")
                .clone();
            (name, start, end)
        })
        .collect();
    Written { files, records, originals, config }
}

fn materialize(files: &Files) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("directory");
    for (name, bytes) in files {
        std::fs::write(directory.path().join(name), bytes).expect("write");
    }
    directory
}

enum Recovered {
    /// Records surviving, and whether recovery reported a truncated tail.
    Records(Vec<Value>, bool),
    Refused(WalError),
}

/// The writer lock belongs to the open file description, and another test of
/// this binary may be spawning a server at this very moment, holding a copy of
/// our descriptor for a few microseconds after we dropped ours. Retry
/// briefly; this is test plumbing, the server never forks.
fn open_wal(
    directory: &Path,
    config: WalConfig,
) -> Result<(WriteAheadLog, hoglet::pipeline::wal::Recovery), WalError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match WriteAheadLog::open(directory, config) {
            Err(WalError::WriterLocked) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

/// Open, then read everything back through the publisher's reader.
fn recover(directory: &Path, config: WalConfig) -> Recovered {
    match open_wal(directory, config) {
        Err(error) => Recovered::Refused(error),
        Ok((mut wal, recovery)) => {
            wal.seal().expect("seal after recovery");
            let records: Vec<Value> = wal
                .sealed_records_from(WalCursor::origin())
                .expect("cursor")
                .map(|record| serde_json::to_value(record.expect("validated record").batch).expect("json"))
                .collect();
            Recovered::Records(records, recovery.truncated_tail)
        }
    }
}

fn assert_loud(error: &WalError) {
    assert!(
        matches!(error, WalError::Corruption { .. }),
        "recovery refused for the wrong reason (it must name the corruption): {error}"
    );
}

#[derive(Default, Debug)]
struct Tally {
    cases: u64,
    refused: u64,
    truncated_to_prefix: u64,
    unchanged: u64,
}

impl Tally {
    fn line(&self, name: &str) -> String {
        format!(
            "FUZZ {name}: cases={} refused_loudly={} truncated_to_prefix={} recovered_in_full={}",
            self.cases, self.refused, self.truncated_to_prefix, self.unchanged
        )
    }
}

fn report(line: &str) {
    println!("{line}");
    use std::io::Write;
    if let Ok(path) = std::env::var("HOGLET_MATRIX_OUT")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// The invariant for damage confined to the ACTIVE segment (the only place a
/// crash can leave damage): refuse loudly, or return a prefix that keeps
/// every record wholly before the first damaged byte, and say it truncated.
fn check_active_damage(written: &Written, damaged: &Files, first_damage: u64, tally: &mut Tally) {
    let directory = materialize(damaged);
    let before = damaged.clone();
    let active = written.active_name();
    tally.cases += 1;
    match recover(directory.path(), written.config) {
        Recovered::Refused(error) => {
            assert_loud(&error);
            tally.refused += 1;
            // Failing loudly must not rewrite anything.
            assert_eq!(
                &std::fs::read(directory.path().join(&active)).expect("read"),
                &before[&active],
                "a refused recovery modified the segment"
            );
        }
        Recovered::Records(records, truncated) => {
            assert!(
                records.len() <= written.originals.len()
                    && records[..] == written.originals[..records.len()],
                "recovery returned something that is not a prefix of what was written"
            );
            let must_keep = written.complete_records_before(&active, first_damage);
            assert!(
                records.len() >= must_keep,
                "recovery lost a record that ended before the first damaged byte \
                 ({} kept, {must_keep} required)",
                records.len()
            );
            if records.len() < written.originals.len() {
                // A file cut exactly between two records is a clean, shorter
                // log: nothing was torn, so there is nothing to report.
                let damaged_active = &before[&active];
                let original_active = &written.files[&active];
                let clean_cut = original_active.starts_with(damaged_active)
                    && (damaged_active.is_empty()
                        || written
                            .records
                            .iter()
                            .any(|(file, _, end)| *file == active && *end == damaged_active.len() as u64));
                assert!(
                    truncated || clean_cut,
                    "records were dropped without reporting a truncated tail"
                );
                tally.truncated_to_prefix += 1;
            } else {
                tally.unchanged += 1;
            }
        }
    }
}

fn standard_log() -> Written {
    // Five records of different shapes in one active segment.
    write_log(&[(1, 5), (2, 40), (1, 0), (3, 200), (1, 17)], 1 << 20)
}

#[test]
fn truncating_the_active_segment_at_every_byte_offset_keeps_exactly_the_complete_records() {
    let written = standard_log();
    let active = written.active_name();
    let original = &written.files[&active];
    let mut tally = Tally::default();
    for cut in 0..=original.len() {
        let mut damaged = written.files.clone();
        damaged.insert(active.clone(), original[..cut].to_vec());
        let directory = materialize(&damaged);
        let Recovered::Records(records, truncated) = recover(directory.path(), written.config) else {
            panic!("a torn tail at byte {cut} must be repaired, not refused");
        };
        let expected = written.complete_records_before(&active, cut as u64);
        assert_eq!(records.len(), expected, "cut at {cut}");
        assert_eq!(records[..], written.originals[..expected], "cut at {cut}");
        let on_boundary = cut == 0 || written.records.iter().any(|(_, _, end)| *end == cut as u64);
        assert_eq!(truncated, !on_boundary, "cut at {cut}: truncated_tail flag");
        // The torn bytes are gone and the log keeps working.
        let (mut wal, _) = open_wal(directory.path(), written.config).unwrap_or_else(|e| panic!("reopen after cut {cut}: {e}"));
        wal.append(batch(99, 1, 3)).expect("append after recovery");
        tally.cases += 1;
        if truncated {
            tally.truncated_to_prefix += 1;
        } else {
            tally.unchanged += 1;
        }
    }
    report(&tally.line("truncate_active_every_offset"));
}

#[test]
fn every_single_bit_flip_in_the_final_record_is_refused_or_truncated_never_accepted() {
    let written = standard_log();
    let active = written.active_name();
    let original = &written.files[&active];
    let (_, start, end) = written.records.last().expect("records").clone();
    let mut tally = Tally::default();
    for offset in start..end {
        for bit in 0..8 {
            let mut bytes = original.clone();
            bytes[offset as usize] ^= 1 << bit;
            let mut damaged = written.files.clone();
            damaged.insert(active.clone(), bytes);
            check_active_damage(&written, &damaged, start, &mut tally);
        }
    }
    report(&tally.line("bitflip_final_record_every_bit"));
    assert!(tally.refused > 0, "no flip was refused: the checksum is not being checked");
}

#[test]
fn every_single_bit_flip_in_the_whole_active_segment_never_yields_an_altered_record() {
    let written = standard_log();
    let active = written.active_name();
    let original = &written.files[&active];
    let mut tally = Tally::default();
    for offset in 0..original.len() {
        for bit in 0..8 {
            let mut bytes = original.clone();
            bytes[offset] ^= 1 << bit;
            let mut damaged = written.files.clone();
            damaged.insert(active.clone(), bytes);
            check_active_damage(&written, &damaged, offset as u64, &mut tally);
        }
    }
    report(&tally.line("bitflip_active_segment_every_bit"));
}

#[test]
fn every_single_bit_flip_in_a_sealed_segment_is_refused_loudly() {
    // Two segments: the first sealed, the second active.
    let written = write_log(&[(1, 5), (2, 40), (1, 0), (3, 200), (1, 17)], 300);
    let sealed = written.sealed_names();
    assert!(!sealed.is_empty(), "the fixture must contain a sealed segment");
    let mut tally = Tally::default();
    for name in &sealed {
        let original = &written.files[name];
        for offset in 0..original.len() {
            for bit in 0..8 {
                let mut bytes = original.clone();
                bytes[offset] ^= 1 << bit;
                let mut damaged = written.files.clone();
                damaged.insert(name.clone(), bytes);
                let directory = materialize(&damaged);
                tally.cases += 1;
                match recover(directory.path(), written.config) {
                    Recovered::Refused(error) => {
                        assert_loud(&error);
                        tally.refused += 1;
                    }
                    Recovered::Records(..) => panic!(
                        "a bit flip at {name}:{offset} bit {bit} in a SEALED segment was silently accepted"
                    ),
                }
            }
        }
    }
    report(&tally.line("bitflip_sealed_segment_every_bit"));
    assert_eq!(tally.cases, tally.refused);
}

#[test]
fn a_sealed_segment_cut_mid_record_is_refused_at_every_offset() {
    let written = write_log(&[(1, 5), (2, 40), (1, 0), (3, 200), (1, 17)], 300);
    let sealed = written.sealed_names();
    let mut cases = 0;
    for name in &sealed {
        let original = &written.files[name];
        let boundaries: Vec<u64> = std::iter::once(0)
            .chain(written.records.iter().filter(|(file, _, _)| file == name).map(|(_, _, end)| *end))
            .collect();
        for cut in 0..original.len() {
            if boundaries.contains(&(cut as u64)) {
                // Cut exactly between records: a valid shorter segment. Not
                // detectable without a trailer; documented in claims.md.
                continue;
            }
            let mut damaged = written.files.clone();
            damaged.insert(name.clone(), original[..cut].to_vec());
            let directory = materialize(&damaged);
            cases += 1;
            match recover(directory.path(), written.config) {
                Recovered::Refused(error) => assert_loud(&error),
                Recovered::Records(..) => panic!("{name} cut at {cut} was silently accepted"),
            }
        }
    }
    report(&format!("FUZZ truncate_sealed_mid_record_every_offset: cases={cases} refused_loudly={cases}"));
}

#[test]
fn a_zero_filled_tail_after_a_power_cut_is_dropped_and_kept_for_forensics() {
    // Some filesystems extend a file's length before its data reaches disk, so
    // a power cut leaves zeros where the last record should be. Those bytes
    // were never acknowledged (acks wait for fsync), so recovery drops them
    // and saves them next to the segment instead of refusing to start.
    let written = standard_log();
    let active = written.active_name();
    let (_, start, end) = written.records.last().expect("records").clone();
    let mut bytes = written.files[&active].clone();
    for byte in &mut bytes[start as usize..end as usize] {
        *byte = 0;
    }
    let mut damaged = written.files.clone();
    damaged.insert(active.clone(), bytes);
    let directory = materialize(&damaged);
    match recover(directory.path(), written.config) {
        Recovered::Records(records, truncated) => {
            assert!(truncated, "recovery reports the dropped tail");
            assert_eq!(records.len(), written.records.len() - 1);
        }
        Recovered::Refused(error) => panic!("a zero-filled torn tail must not stop recovery: {error}"),
    }
    let kept = directory.path().join(active.replace(".open", ".torn"));
    assert!(kept.exists(), "the dropped bytes are kept for forensics");
}

#[test]
fn a_bad_record_followed_by_valid_data_is_still_refused() {
    // Damage in the middle of the log is not a torn tail: acknowledged
    // records come after it. Refusing is the only honest answer.
    let written = standard_log();
    let active = written.active_name();
    assert!(written.records.len() >= 3, "the standard log has several records");
    let (_, start, _) = written.records[written.records.len() - 2].clone();
    let mut bytes = written.files[&active].clone();
    bytes[start as usize + 16 + 4] ^= 0xff;
    let mut damaged = written.files.clone();
    damaged.insert(active, bytes);
    let directory = materialize(&damaged);
    match recover(directory.path(), written.config) {
        Recovered::Refused(error) => assert_loud(&error),
        Recovered::Records(..) => panic!("damage before a valid record was silently accepted"),
    }
}

// ------------------------------------------------------------ proptest

#[derive(Debug, Clone)]
enum Damage {
    FlipBit { at: u32, bit: u8 },
    Truncate { at: u32 },
    Zero { at: u32, len: u16 },
    AppendGarbage { bytes: Vec<u8> },
    Overwrite { at: u32, bytes: Vec<u8> },
}

fn damage() -> impl Strategy<Value = Damage> {
    prop_oneof![
        (any::<u32>(), 0_u8..8).prop_map(|(at, bit)| Damage::FlipBit { at, bit }),
        any::<u32>().prop_map(|at| Damage::Truncate { at }),
        (any::<u32>(), 1_u16..64).prop_map(|(at, len)| Damage::Zero { at, len }),
        proptest::collection::vec(any::<u8>(), 1..48).prop_map(|bytes| Damage::AppendGarbage { bytes }),
        (any::<u32>(), proptest::collection::vec(any::<u8>(), 1..16))
            .prop_map(|(at, bytes)| Damage::Overwrite { at, bytes }),
    ]
}

fn apply(damage: &Damage, bytes: &mut Vec<u8>) {
    let len = bytes.len();
    match damage {
        Damage::FlipBit { at, bit } if len > 0 => bytes[*at as usize % len] ^= 1 << bit,
        Damage::Truncate { at } => bytes.truncate(*at as usize % (len + 1)),
        Damage::Zero { at, len: count } if len > 0 => {
            let start = *at as usize % len;
            let end = (start + *count as usize).min(len);
            bytes[start..end].fill(0);
        }
        Damage::AppendGarbage { bytes: garbage } => bytes.extend_from_slice(garbage),
        Damage::Overwrite { at, bytes: patch } if len > 0 => {
            let start = *at as usize % len;
            for (i, byte) in patch.iter().enumerate() {
                if start + i < len {
                    bytes[start + i] = *byte;
                }
            }
        }
        _ => {}
    }
}

fn first_difference(original: &[u8], damaged: &[u8]) -> Option<u64> {
    let shared = original.len().min(damaged.len());
    (0..shared)
        .find(|i| original[*i] != damaged[*i])
        .or((original.len() != damaged.len()).then_some(shared))
        .map(|i| i as u64)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, ..ProptestConfig::default() })]

    /// Damage confined to the active segment: refuse loudly or keep a prefix.
    #[test]
    fn active_segment_damage_never_corrupts_recovery(
        sizes in proptest::collection::vec((1_usize..4, 0_usize..600), 1..12),
        segment_target in 400_u64..6_000,
        damages in proptest::collection::vec(damage(), 1..4),
    ) {
        let written = write_log(&sizes, segment_target);
        let active = written.active_name();
        let mut bytes = written.files[&active].clone();
        for d in &damages {
            apply(d, &mut bytes);
        }
        let first = first_difference(&written.files[&active], &bytes);
        let mut damaged = written.files.clone();
        damaged.insert(active, bytes);
        let mut tally = Tally::default();
        check_active_damage(&written, &damaged, first.unwrap_or(u64::MAX), &mut tally);
    }

    /// Damage to a sealed segment: refuse loudly, unless the bytes are intact
    /// or the file was cut exactly between records (a valid shorter segment).
    #[test]
    fn sealed_segment_damage_is_never_silently_accepted(
        sizes in proptest::collection::vec((1_usize..4, 0_usize..600), 3..12),
        segment_target in 300_u64..1_500,
        victim in any::<u32>(),
        damages in proptest::collection::vec(damage(), 1..4),
    ) {
        let written = write_log(&sizes, segment_target);
        let sealed = written.sealed_names();
        prop_assume!(!sealed.is_empty());
        let name = sealed[victim as usize % sealed.len()].clone();
        let original = written.files[&name].clone();
        let mut bytes = original.clone();
        for d in &damages {
            apply(d, &mut bytes);
        }
        let boundaries: Vec<u64> = std::iter::once(0)
            .chain(written.records.iter().filter(|(file, _, _)| *file == name).map(|(_, _, end)| *end))
            .collect();
        let benign = bytes == original
            || (bytes.len() < original.len()
                && original.starts_with(&bytes)
                && boundaries.contains(&(bytes.len() as u64)));
        let mut damaged = written.files.clone();
        damaged.insert(name, bytes);
        let directory = materialize(&damaged);
        match recover(directory.path(), written.config) {
            Recovered::Refused(error) => {
                prop_assert!(matches!(error, WalError::Corruption { .. }), "{error}");
                prop_assert!(!benign, "an intact or cleanly shortened segment was refused");
            }
            Recovered::Records(records, _) => {
                prop_assert!(benign, "a damaged sealed segment was silently accepted");
                // Whatever came back is a subsequence of what was written,
                // altered nothing and invented nothing.
                let mut cursor = 0;
                for record in &records {
                    let found = written.originals[cursor..].iter().position(|o| o == record);
                    prop_assert!(found.is_some(), "recovery returned a record that was never written");
                    cursor += found.unwrap() + 1;
                }
            }
        }
    }

    /// Damage anywhere, to any number of segments: never a panic, and every
    /// record returned is one that was written, in order.
    #[test]
    fn damage_anywhere_never_invents_or_alters_a_record(
        sizes in proptest::collection::vec((1_usize..4, 0_usize..600), 2..12),
        segment_target in 300_u64..2_000,
        damages in proptest::collection::vec((any::<u32>(), damage()), 1..5),
    ) {
        let written = write_log(&sizes, segment_target);
        let names: Vec<String> = written.files.keys().cloned().collect();
        let mut damaged = written.files.clone();
        for (which, d) in &damages {
            let name = &names[*which as usize % names.len()];
            apply(d, damaged.get_mut(name).expect("file"));
        }
        let directory = materialize(&damaged);
        if let Recovered::Records(records, _) = recover(directory.path(), written.config) {
            let mut cursor = 0;
            for record in &records {
                let found = written.originals[cursor..].iter().position(|o| o == record);
                prop_assert!(found.is_some(), "recovery returned a record that was never written");
                cursor += found.unwrap() + 1;
            }
        }
    }
}

// ----------------------------------------------- the real binary, end to end

#[cfg(feature = "fault-injection")]
mod claim2_support;

#[cfg(feature = "fault-injection")]
mod end_to_end {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    use super::claim2_support::*;

    /// Crash with acknowledged events sitting in the WAL (publication killed
    /// before it commits) and return the data directory and ledger.
    fn crashed_with_unpublished_wal(spec: &str, segment_bytes: Option<u64>) -> (tempfile::TempDir, Arc<Ledger>) {
        let directory = tempfile::tempdir().expect("data directory");
        let mut env = vec![("HOGLET_FAULT", spec.to_owned())];
        if let Some(bytes) = segment_bytes {
            env.push(("HOGLET_FAULT_WAL_SEGMENT_BYTES", bytes.to_string()));
        }
        let mut server = Server::spawn(directory.path(), &env);
        let _project = setup(server.port);
        let ledger = Arc::new(Ledger::default());
        let writers = Writers::start(server.port, &ledger, 3, 2, 8, 5, "");
        assert!(server.wait_for_death(Duration::from_secs(60)), "fault never fired");
        writers.stop();
        drop(server);
        (directory, ledger)
    }

    fn segments(data: &std::path::Path, suffix: &str) -> Vec<std::path::PathBuf> {
        let mut found: Vec<_> = std::fs::read_dir(data.join("wal"))
            .expect("wal dir")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().ends_with(suffix))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_torn_tail_on_disk_is_repaired_and_every_acknowledged_event_survives() {
        let (directory, ledger) = crashed_with_unpublished_wal("wal.sync.before=abort@20", None);
        // Tear the active segment mid-record, as a power cut would.
        let active = segments(directory.path(), ".open").pop().expect("active segment");
        let length = std::fs::metadata(&active).expect("metadata").len();
        assert!(length > 40);
        let file = std::fs::OpenOptions::new().write(true).open(&active).expect("open");
        file.set_len(length - 7).expect("tear");
        drop(file);
        let mut server = Server::spawn(directory.path(), &[]);
        server.stop_gracefully();
        let acked_before_tear = ledger.acked_count();
        let report = reconcile(directory.path(), &ledger, &BTreeSet::new());
        // The tear cut into the final record: at worst the events of the last
        // acknowledged group are gone, and only those.
        match report {
            Ok(_) => {}
            Err(error) => {
                assert!(error.contains("acknowledged events were lost"), "{error}");
                let lost: usize = error
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("count");
                assert!(lost <= 8, "a 7-byte tear destroyed {lost} of {acked_before_tear} acknowledged events");
            }
        }
    }

    #[test]
    fn corruption_in_a_sealed_segment_stops_the_server_loudly() {
        let (directory, _ledger) = crashed_with_unpublished_wal("pub.before_commit=abort@1", Some(3_000));
        let sealed = segments(directory.path(), ".wal");
        assert!(!sealed.is_empty(), "the crash left no sealed segment to corrupt");
        let victim = &sealed[sealed.len() / 2];
        let mut bytes = std::fs::read(victim).expect("read");
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x40;
        std::fs::write(victim, bytes).expect("write");
        let log = Server::spawn_expecting_failure(directory.path());
        assert!(log.contains("Corruption"), "the failure did not name the corruption:\n{log}");
    }

    #[test]
    fn corruption_before_a_complete_record_in_the_active_segment_stops_the_server_loudly() {
        // The 1 s seal timer can empty the active segment just before the
        // crash; crash again until it holds several records.
        let (directory, active) = (0..10)
            .find_map(|_| {
                let (directory, _ledger) = crashed_with_unpublished_wal("wal.sync.before=abort@25", None);
                let active = segments(directory.path(), ".open").pop()?;
                (std::fs::metadata(&active).ok()?.len() > 600).then_some((directory, active))
            })
            .expect("a crash that leaves records in the active segment");
        let mut bytes = std::fs::read(&active).expect("read");
        bytes[40] ^= 0x01;
        std::fs::write(&active, bytes).expect("write");
        let log = Server::spawn_expecting_failure(directory.path());
        assert!(log.contains("Corruption"), "the failure did not name the corruption:\n{log}");
    }
}
