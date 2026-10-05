# Claims and Evidence

What Hoglet promises, and what would prove it. Written before implementation, on purpose — this determines what we build, not just what we test.

Rule: a green check proves only its own assertion. Unit tests, type checks, and builds establish internal properties. None of them prove a claim below. Evidence must be gathered at the boundary where someone actually experiences the outcome.

Nothing here is proven until its evidence exists. Until then, do not make the claim in public.

---

## Claim 1 — Stock PostHog SDKs work unchanged

**Who experiences it:** a developer who changes `api_host` to point at Hoglet and touches nothing else.

**Starting state:** an app already instrumented with an unmodified PostHog SDK at its published latest version.

**Must happen:** the SDK initializes without error, events arrive with distinct_id, properties, and timestamps intact, `identify()` links the anonymous history to the identified person, and the SDK never enters a retry loop.

**Invariants:** we never ask the developer to change SDK code, pin a version, or set a Hoglet-specific option. Response codes stay on PostHog's contract — 4xx never retried, 5xx retried.

**Evidence that proves it:** real posthog-js running in a real browser under Playwright, against a real Hoglet process, asserting on what arrived in storage. Run nightly against SDK `latest`, not a pinned version — the claim is about *their* current SDK, so pinning would prove the wrong thing.

**Distinguishing success from a plausible imitation:** a mock server that returns 200 to everything also passes a naive check. The test must assert on stored events and on SDK-observable behavior — flags resolving, no retry queue growth, config fetch succeeding — not merely that a request was accepted.

**Outside the claim:** session replay, surveys, experiments, warehouse. Not implemented, not claimed.

---

## Claim 2 — We do not lose events

**Who experiences it:** the operator whose dashboard numbers have to be defensible.

**Starting state:** events in flight, some in memory, some in the WAL, some not yet flushed to Parquet.

**Must happen:** every event acknowledged with a 2xx is recoverable after a crash. Events not yet acknowledged may be lost — that's the contract, and the SDK retries them.

**Invariants:** acknowledgement happens only after the WAL write is durable, never before. Recovery truncates at the first checksum failure and loses nothing before it. A corrupted tail never silently becomes valid data.

**Evidence that proves it:** a real `hoglet` process, built with `--features fault-injection` (hooks in `src/fault.rs`, compiled out of normal builds and asserted absent from the default binary in CI), is made to SIGKILL itself at a named point of the write path, then the real binary is restarted on the same data directory and the clients' ledger (what was sent, what got a 200) is reconciled against what is stored. Plain process crashes, not turmoil: the failure model is "the process dies here" plus emulated power cuts (`lose:PCT` drops unsynced WAL bytes, `revert` undoes a not-yet-durable rename, `torn:PCT` persists part of a record). Run: `cargo test --features fault-injection --test claim2_kill_matrix` (and `claim2_io_errors`, `claim2_concurrency`, `claim2_wal_corruption`); `HOGLET_FAULT_SEEDS=<n>` sets randomized runs per point (random batch sizes, writer counts, hit number, WAL segment size, tear position), `HOGLET_PROPTEST_CASES=<n>` the proptest cases, `HOGLET_MATRIX_OUT=<file>` records one line per run.

- **Kill-point matrix** (`tests/claim2_kill_matrix.rs`, 24 points + 1 mutant): torn write mid-record; after write, before fsync (process crash and power cut); after fsync, before ack; WAL seal before rename, after rename (durable and reverted), after directory sync; publisher after Parquet temp write, after rename, before and after catalog commit, mid WAL reclaim, after reclaim; compactor after output written, after rename, after commit, before graveyard sweep, sweep after unlink; erasure before the identity commit, after it, after rewrite, after rename, after lake commit. After every crash: each acknowledged uuid is stored exactly once, byte-for-byte as sent (event, distinct id, timestamp, payload); nothing stored that was never sent; no orphan or missing files; catalog row counts equal Parquet row counts; identity rows match stored events; the API (`GET /events`, paged) and DuckDB over the lake files return the same set; ingestion resumes; a second restart changes nothing; erased persons are absent.
- **The harness can fail** (`harness_detects_a_wal_that_acks_before_fsync`): with `HOGLET_FAULT_MUTANT=skip_fsync` (a WAL whose fsync is a no-op, so it acknowledges before durable) plus a power cut, the same reconciliation must report lost acknowledged events. If that test ever passes the mutant, the matrix proves nothing.
- **WAL damage** (`tests/claim2_wal_corruption.rs`): the active segment truncated at every byte offset (recovery keeps exactly the complete records and reports a truncated tail iff it cut mid-record); every single bit of the final record, of the whole active segment, and of a sealed segment flipped; sealed segments cut mid-record at every offset; proptest over record sequences across several segments with random bit flips, truncation, zeroed ranges, garbage appends and overwrites to the active segment, sealed segments, or both. Invariants: never a panic; recovery refuses loudly (a `Corruption` error naming segment and offset, file left untouched) or returns a prefix of what was written that keeps every record before the first damaged byte; never an altered or invented record; damage to a sealed segment is never silently accepted. End to end (feature build): the real binary repairs a torn tail and keeps every acknowledged event, and refuses to start on corruption in a sealed segment or before a valid record in the active one.
- **Disk full and I/O errors** (`tests/claim2_io_errors.rs`): a failing fsync, a failing write that leaves half a record (ENOSPC), a failing segment seal, and a publisher that cannot write Parquet. A WAL failure poisons the log: every later request is a 503 (never 4xx, never 2xx), no request sent after a 503 was ever acknowledged, the process keeps serving; after restart every acknowledged event is stored once and ingestion resumes. A broken publisher never affects capture; acknowledged events wait in the WAL and appear exactly once when it recovers or the process restarts.
- **Concurrency** (`tests/claim2_concurrency.rs`): 3-6 writers, background compaction, a concurrent reader polling the API, person erasures (including one interrupted by the kill), and repeated SIGKILL / graceful restarts of one data directory. Invariants after every restart and at the end: acknowledged ⊆ stored ∪ erased, no duplicates, no phantom or altered events, erased persons absent from events and identity, readers never saw an error.
- **Regression tests for the three bugs this evidence found:** `claim2_erasure_atomicity` (an erasure interrupted after the identity commit left the person's events in the lake forever; now recorded in `pending_erasures` and completed on start), `claim2_wal_cursor_race` (a checkpoint taken while the writer was mid-seal named a segment that reclamation then deleted, wedging publication and every restart; and the publisher reported `WAL corruption: missing WAL segment` on a healthy log while the writer sealed).

**Last high-iteration local run (dev box, debug build, 2026-10):** kill-point matrix 25 randomized runs for each of 24 points (600 crashes), 676,279 acknowledged events reconciled, 0 lost, 0 duplicated, 0 orphan files, 567 events durable-but-unacknowledged (allowed); WAL damage 43,540 exhaustive cases (every offset / every bit) plus 30,000 proptest cases per property, 0 silently accepted; I/O errors 15 runs each of failing fsync and failing write, 0 false acknowledgements; concurrency 6 runs x 8 restart cycles (39 hard kills, 137 erasures), 0 violations. Under 25 parallel servers on a busy box the harness itself starved writers and 28 runs "never fired"; writers no longer retire on timeouts, and the rerun was 600/600.

**Recovery semantics as implemented (read this before quoting the invariants):** in the *active* segment, recovery drops a torn tail: an incomplete final record, a zero-filled tail (as some filesystems leave after a power cut), or a final record whose checksum or payload is bad. Acks wait for fsync, so none of those bytes was acknowledged; the dropped bytes are saved as `<segment>.torn` and logged. A bad record with *valid data after it* is not a torn tail — acknowledged records follow it — so the server refuses to start and names the corruption, as it does for any damage in a sealed segment. Nothing before the first damaged byte is ever lost.

**Distinguishing success from a plausible imitation:** a WAL that fsyncs after acknowledging passes every happy-path test and fails only under crash. The kill-point matrix kills at the adversarial points (between write and fsync, between fsync and ack, between rename and directory sync), and the mutant test above proves the reconciliation would notice.

**Not covered (do not claim):**
- Real power loss or a lying disk. Process death keeps the page cache; `lose:PCT` and `revert` emulate the worst case of unsynced data and renames, but no test pulls power or models a disk that acknowledges fsync and drops data.
- A sealed WAL segment truncated exactly at a record boundary, or a segment file deleted: indistinguishable from a valid shorter log without a trailer or manifest. Whole-active-file deletion is likewise undetectable.
- A 5xx is not "not stored": events whose fsync failed or whose ack was lost can be durable. An SDK retry of such an event arrives as a second record with the same uuid. Replaying the WAL never duplicates, but retries land in different publication windows and are collapsed only by compaction (`durable_pipeline::compaction_merges_dedups_and_defers_deletion_until_leases_end`); until then both copies are visible to queries. "Stored exactly once" in this section is about recovery, not about SDK retries.
- Erasure under sustained ingest: the publisher serializes erasure behind publication of the whole backlog, so with ingest faster than publication (observed on a debug build, unthrottled writers) an erasure request did not return for minutes. Correctness is unaffected; latency is unbounded. Found, not fixed.
- Kill points inside SQLite itself (its own journal is trusted), Parquet writer internals, the control database, and `HOGLET_RETENTION_DAYS` retention.
- Multi-node, replication, and any filesystem other than the Linux one the tests ran on.
- Capture-edge loss (ad-blockers, SDK queue limits, rate-limit 429s).

**Outside the claim:** events an ad-blocker prevented from ever reaching us. Different problem, tracked separately.

---

## Claim 3 — One binary, small enough for a $5 VPS

**Who experiences it:** someone who wants analytics without operating infrastructure.

**Starting state:** a single-core box with 1 GB RAM and a fresh install.

**Must happen:** one executable, no sidecar services, no container orchestration, no external database. It starts, serves, and survives normal traffic within that envelope.

**Invariants:** no Kafka, no ClickHouse, no Redis, no Zookeeper, no separate worker process. Memory does not grow unbounded under a traffic spike — ingest allocations are explicitly bounded.

**Evidence that proves it:** measured RSS under sustained load on a box with that spec, using oha or k6, reported as a real number. A dependency assertion in CI that fails if a forbidden service creeps into the deployment. A load test that spikes hard and shows memory returning to baseline. The load test must run a heavy analytical query (funnel over the full dataset) *concurrently* with sustained ingest on the 1 GB box — DuckDB's memory appetite under a capped `memory_limit` is the biggest untested assumption in this claim, and sequential tests would hide it.

**Distinguishing success from a plausible imitation:** "it compiled to one binary" proves nothing about runtime footprint. The number that matters is RSS under load, measured, not idle memory at startup.

**Measured locally (not the published number):** via `scripts/loadtest.sh` on a dev machine (not the 1 GB VPS), with a query racing ingest: peak RSS ~146 MB across 200k events at 64 concurrent connections; throughput ~10k events/s at 64 conns, ~1.5k at 8 (fsync/group-commit bound — scales with concurrency, exactly as designed). These clear the SPEC targets (<400 MB, ≥5k/s) but are dev-box numbers. The published claim still requires the stated 1 vCPU / 1 GB hardware.

**Measured under an emulated 1 vCPU / 1.5 GB cgroup (2026-10, not the published number):** sustained ingest 22k events/s at 194 MB peak RSS (`scripts/loadtest.sh`, 400k events, 32 connections, dashboard queries racing it: p50 9 ms, p95 85 ms). Query latency, same one-CPU cgroup (`systemd-run --user --scope -p CPUQuota=100% -p MemoryMax=1500M -p MemorySwapMax=0 <test binary> bench_ten_million --ignored --nocapture`, built with `CARGO_PROFILE_RELEASE_LTO=false cargo test --release --lib --no-run`; engine 512 MB / 2 DuckDB threads, the production defaults), over 10M events / 300k persons / 50k identity overrides / 30 days, files written by the compactor (sorted by event and timestamp, zstd), median of 5 runs, before and after measured back to back on a shared machine (load average ~20, so upper bounds). Before to after: trends total 0.19 s to 0.18 s; daily active users 1.17 s to 0.67 s; weekly active users 3.4 s to 0.64 s; three-step funnel 7.6 s to 2.3 s; web overview over 30 days 13.9 s to 2.6 s (before, it spilled 1.2 GB to the temp directory and was killed by the cgroup when that directory was tmpfs). With 36-character ids (what SDKs generate; `HOGLET_BENCH_IDS=uuid`): 0.19 s, 1.7 s to 0.90 s, 4.0 s to 0.87 s, 8.1 s to 2.5 s, web overview from failing (4 GB spill limit) to 2.9 s. Misses: the web overview stays above its 2 s target because this dataset has about 6M sessions in 10M events, and grouping 6M sessions on one core is about 2 s by itself; the funnel meets 3 s but not the 2 s stretch with long ids. Method details: `HOGLET_BENCH_DIR` keeps the generated data between runs, `HOGLET_QUERY_PROFILE=1` prints DuckDB's EXPLAIN ANALYZE for each statement.

**Flags under load (emulated 1 vCPU / 1 GB, `scripts/flags-isolation.sh`):** while flat-out ingest and two looping 90-day funnel/web-overview queries share the single core, `/flags` answers in p50 0.7 ms, p99 85 ms, max 86 ms (320 requests, 25 s). Flag evaluation never queues behind analytics (it reads SQLite and evaluates in memory, off the query lane), but on one core it competes for CPU time, so tail latency rises under saturation. No request failed or timed out.

**Outside the claim:** any specific *published* throughput figure. We do not publish an events-per-second number until we have measured one on the stated hardware.

---

## Claim 4 — Your person counts are honest

**Who experiences it:** anyone who has been burned by ghost profiles inflating their user count.

**Starting state:** a user browsing anonymously across sessions and devices, then logging in.

**Must happen:** anonymous history stitches to the identified person exactly once. No duplicate persons. Merge order does not change the outcome.

**Invariants:** merges are deterministic and order-insensitive. A distinct_id maps to exactly one person at any point in time. The distinct_id-to-person ratio is inspectable, not hidden.

**Evidence that proves it:** `tests/claim4_person_graph.rs` runs the real identity projection (`projections::apply_captured_event`) on a real SQLite database, one transaction per event as publication does, and checks it against reference models written independently of the SQL. Run: `cargo test --test claim4_person_graph`; `HOGLET_PROPTEST_CASES=<n>` sets cases per property (default 64, CI 256, nightly 5000).

- **Which orders must converge, stated precisely.** Order-independent (the claim): `$identify` of a long-lived id with an anonymous id that appears in no other identity event; `$create_alias` in either SDK direction (posthog-node `(user, anon)`, posthog-python `(anon, user)`) on such an id; `$merge_dangerously` between any ids; `$set`; plain events; SDK retries of the same identity event. Property `convergent_every_order` generates such sequences, applies them in the original order, reversed, and 3-8 random permutations each, and asserts every order gives the same partition of distinct ids as a union-find over the edges, the same `is_identified` flags, and no `$set` key lost or invented.
- **Not order-independent (not claimed, pinned by a test):** an `$identify` or `$create_alias` between two people who are *already identified* is refused, and who is already identified depends on arrival order (`a→b` then `b→c` differs from `b→c` then `a→b`: `refusals_between_identified_people_depend_on_arrival_order...` asserts the two orders differ, so a future change that makes them converge is noticed). Likewise one anonymous id identified as two different users: the first wins, by design (shared device).
- **Invariants for every order and every mix of events** (`all_orders_projection_matches_the_model...`, checked after every single event, over the original order and random permutations): every distinct id maps to exactly one person; no person without distinct ids; persons ≤ distinct ids and persons = number of distinct-id classes; no distinct id is ever lost or invented; merges only coarsen the partition; `$merge_dangerously` always unifies; `$identify` never absorbs an identified anonymous side and `$create_alias` never merges two identified people (from the pre-event flags read out of the database); and the projection agrees step by step with a flag-aware reference model of the documented merge rules.
- **Reconciliation** (`reconciliation_on_large_random_datasets...`, and inside every property): `count(persons)` equals the class count of the independent model for random datasets of up to 1,500 events over up to 400 ids; the distinct-id to person ratio is computed from the database and reported (sample: 329 ids, 91 persons).
- **Adversarial scenarios**, each in both orders where order exists: shared device (two users on one browser never merge, the device id belongs to exactly one); cleared cookies and repeated identify (one person, ids joined once); identify before and after pageviews carrying the old anonymous id; both SDK alias directions keep the identified person's id and properties, refuse two identified people, and `$merge_dangerously` unifies them; and concurrent identifies of the same people through the real durable pipeline (many tasks appending in arbitrary WAL order, 3 users × 20 browsers × 5 rounds, read back through `PersonStore`).
- **The tests can fail:** three deliberate mutations of `merge_persons` (drop the identified-source refusal; make `$merge_dangerously` honor the both-identified refusal; make `$identify` also refuse an identified winner) each failed 4-5 of the 9 tests when tried by hand.
- Atomicity with the event files (identity effects and Parquet files commit together, so a crash never leaves events without persons) is covered by claim 2's matrix, which checks identity against stored events after every kill.

**Last high-iteration local run (dev box, debug build, 2026-10):** 8,000 cases for each of the two sequence properties (480,012 per-event invariant checks across the original order and random permutations; 60,077 full replays in distinct orders for the convergent class), 400 reconciliation datasets, all adversarial scenarios; 0 violations.

**Not covered (do not claim):**
- Person ids, `created_at`, `first_seen_key` (the feature-flag bucketing key) and property conflicts (`$set` of the same key from two merged people, `$set_once`): these depend on which event touched an id first or which side wins a merge, so they can differ with arrival order. Only the partition of distinct ids, `is_identified` in the convergent class, and non-conflicting `$set` keys are claimed.
- Cross-class order independence of refusals (above), and any claim about *which* of two identified users keeps a shared anonymous id.
- Dashboard person counts (the query engine's DAU etc.): `src/query/proptests.rs` compares the engine with a brute-force oracle on datasets with identify/alias/merge, but this file does not re-prove it. There is no dedicated "persons vs distinct ids" endpoint to test; the ratio is read from SQLite.
- Merges across projects, `$groupidentify`, erasure's effect on identity (claim 2's matrix covers erasure), and identity from the `import` command.
- Person *splitting* (un-merging) is not supported and so not tested.

**Distinguishing success from a plausible imitation:** merging correctly in the common ordering is easy; PostHog does that and still produces ghosts. The convergence test permutes the order, compares against an independent union-find, and a stated class of sequences (above) is *excluded* rather than hidden.

**Outside the claim:** cross-device stitching without a login. Not solvable, not claimed.

---

## Adding a claim

Any new public promise gets an entry here before it ships, with its evidence named. If we cannot say what would distinguish it from a plausible imitation, we do not yet understand the claim well enough to make it.
