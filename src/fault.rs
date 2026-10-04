//! Test-only crash and I/O fault injection (claims.md, claim 2).
//!
//! Every durability-relevant step of the write path names itself by calling
//! one of the hooks below. In a normal build (`fault-injection` feature off)
//! each hook is an empty `#[inline(always)]` function: no state, no branch,
//! no environment read — production behavior is byte-identical.
//!
//! With the feature on, the process reads `HOGLET_FAULT` once and arms the
//! listed faults. Grammar, `,`-separated:
//!
//! ```text
//! point=action[:arg][@start][xcount]
//! ```
//!
//! * `point`  — a name passed to a hook, e.g. `pub.after_commit`.
//! * `action` — `abort` (SIGKILL to self: no destructors, no flush), `fail` (the
//!   hooked operation returns an I/O error; a WAL write first persists half
//!   its frame, like a real ENOSPC), `torn:PCT` (WAL write only: persist PCT %
//!   of the frame, fsync it, then abort), `lose:PCT` (before a WAL fsync:
//!   throw away all but PCT % of the unsynced bytes — a power cut — then
//!   abort), `revert` (after a rename: undo it, then abort — a power cut that
//!   lost the not-yet-durable directory entry).
//! * `@start` — first hit that fires (default 1); `xcount` — how many
//!   consecutive hits fire (default: all of them for `fail`, one for the
//!   others).
//!
//! `HOGLET_FAULT_MUTANT=skip_fsync` makes `WriteAheadLog::sync` a no-op that
//! still reports success. It exists so the kill matrix can prove it detects
//! "acknowledged before durable" — the plausible imitation of this WAL.
//! `HOGLET_FAULT_WAL_SEGMENT_BYTES` shrinks WAL segments so multi-segment
//! publication and reclamation are reachable in seconds.

#[cfg(not(feature = "fault-injection"))]
mod disarmed {
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    #[inline(always)]
    pub fn hit(_point: &'static str) {}

    #[inline(always)]
    pub fn io(_point: &'static str) -> std::io::Result<()> {
        Ok(())
    }

    #[inline(always)]
    pub fn wal_write(file: &mut File, frame: &[u8]) -> std::io::Result<()> {
        file.write_all(frame)
    }

    #[inline(always)]
    pub fn wal_synced(_bytes: u64) {}

    #[inline(always)]
    pub fn wal_before_sync(_file: &File) {}

    #[inline(always)]
    pub fn after_rename(_point: &'static str, _from: &Path, _to: &Path) {}

    #[inline(always)]
    pub fn mutant(_name: &'static str) -> bool {
        false
    }

    #[inline(always)]
    pub fn wal_segment_bytes(default: u64) -> u64 {
        default
    }
}

#[cfg(not(feature = "fault-injection"))]
pub use disarmed::*;

#[cfg(feature = "fault-injection")]
mod armed {
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        Abort,
        Fail,
        Torn(u64),
        Lose(u64),
        Revert,
    }

    #[derive(Debug)]
    struct Spec {
        point: String,
        action: Action,
        start: u64,
        count: u64,
        hits: u64,
    }

    static SPECS: OnceLock<Mutex<Vec<Spec>>> = OnceLock::new();
    /// Bytes of the active WAL segment known to be on stable storage.
    static SYNCED: AtomicU64 = AtomicU64::new(0);

    fn parse(text: &str) -> Vec<Spec> {
        text.split(',')
            .filter(|part| !part.trim().is_empty())
            .map(|part| {
                let (point, rest) = part
                    .split_once('=')
                    .unwrap_or_else(|| panic!("HOGLET_FAULT: expected point=action in {part:?}"));
                let (rest, count) = match rest.rsplit_once('x') {
                    Some((head, count)) if count.chars().all(|c| c.is_ascii_digit()) && !count.is_empty() => {
                        (head, Some(count.parse::<u64>().expect("count")))
                    }
                    _ => (rest, None),
                };
                let (action_text, start) = match rest.split_once('@') {
                    Some((head, start)) => (head, start.parse::<u64>().expect("start")),
                    None => (rest, 1),
                };
                let (name, arg) = match action_text.split_once(':') {
                    Some((name, arg)) => (name, arg.parse::<u64>().expect("arg")),
                    None => (action_text, 50),
                };
                let action = match name {
                    "abort" => Action::Abort,
                    "fail" => Action::Fail,
                    "torn" => Action::Torn(arg.clamp(1, 99)),
                    "lose" => Action::Lose(arg.min(99)),
                    "revert" => Action::Revert,
                    other => panic!("HOGLET_FAULT: unknown action {other:?}"),
                };
                let count = count.unwrap_or(if action == Action::Fail { u64::MAX } else { 1 });
                Spec {
                    point: point.to_owned(),
                    action,
                    start: start.max(1),
                    count,
                    hits: 0,
                }
            })
            .collect()
    }

    fn specs() -> &'static Mutex<Vec<Spec>> {
        SPECS.get_or_init(|| {
            Mutex::new(parse(&std::env::var("HOGLET_FAULT").unwrap_or_default()))
        })
    }

    /// Count a hit of `point`; the action to perform now, if any.
    fn fire(point: &str) -> Option<Action> {
        let mut specs = specs().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut fired = None;
        for spec in specs.iter_mut().filter(|spec| spec.point == point) {
            spec.hits += 1;
            if spec.hits >= spec.start && spec.hits - spec.start < spec.count {
                fired = Some(spec.action);
            }
        }
        fired
    }

    /// SIGKILL ourselves: instant, no destructors, no flush, and — unlike
    /// SIGABRT — no multi-hundred-megabyte core dump.
    fn die(point: &str) -> ! {
        eprintln!("hoglet fault injection: aborting at {point}");
        // SAFETY: killing our own process; the call does not return.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
        }
        loop {
            std::thread::park();
        }
    }

    fn injected(point: &str) -> std::io::Error {
        std::io::Error::other(format!("injected fault at {point}: no space left on device"))
    }

    pub fn hit(point: &'static str) {
        if let Some(Action::Abort) = fire(point) {
            die(point);
        }
    }

    pub fn io(point: &'static str) -> std::io::Result<()> {
        match fire(point) {
            Some(Action::Abort) => die(point),
            Some(Action::Fail) => Err(injected(point)),
            _ => Ok(()),
        }
    }

    pub fn wal_write(file: &mut File, frame: &[u8]) -> std::io::Result<()> {
        match fire("wal.write") {
            Some(Action::Torn(percent)) => {
                let keep = ((frame.len() as u64 * percent / 100) as usize).clamp(1, frame.len() - 1);
                let _ = file.write_all(&frame[..keep]);
                let _ = file.sync_data();
                die("wal.write (torn)")
            }
            Some(Action::Fail) => {
                let _ = file.write_all(&frame[..frame.len() / 2]);
                Err(injected("wal.write"))
            }
            Some(Action::Abort) => die("wal.write"),
            _ => file.write_all(frame),
        }
    }

    /// The WAL tells us how much of its active segment an fsync made durable.
    pub fn wal_synced(bytes: u64) {
        SYNCED.store(bytes, Ordering::SeqCst);
    }

    /// Crash point immediately before a WAL fsync.
    pub fn wal_before_sync(file: &File) {
        match fire("wal.sync.before") {
            Some(Action::Abort) => die("wal.sync.before"),
            Some(Action::Lose(percent)) => {
                let synced = SYNCED.load(Ordering::SeqCst);
                if let Ok(metadata) = file.metadata() {
                    let unsynced = metadata.len().saturating_sub(synced);
                    let _ = file.set_len(synced + unsynced * percent / 100);
                    let _ = file.sync_all();
                }
                die("wal.sync.before (unsynced data lost)")
            }
            _ => {}
        }
    }

    /// Crash point immediately after a rename, before the directory fsync.
    pub fn after_rename(point: &'static str, from: &Path, to: &Path) {
        match fire(point) {
            Some(Action::Abort) => die(point),
            Some(Action::Revert) => {
                let _ = std::fs::rename(to, from);
                die(point)
            }
            _ => {}
        }
    }

    pub fn mutant(name: &'static str) -> bool {
        std::env::var("HOGLET_FAULT_MUTANT").is_ok_and(|value| value == name)
    }

    pub fn wal_segment_bytes(default: u64) -> u64 {
        std::env::var("HOGLET_FAULT_WAL_SEGMENT_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    }
}

#[cfg(feature = "fault-injection")]
pub use armed::*;
