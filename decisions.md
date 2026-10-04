# Decision Authority

Who decides what on Hoglet. Binding on every agent and session. When in doubt, it's the boss's call — ask, don't assume.

## The boss decides. Never proceed without an explicit go-ahead.

**Direction and scope**
- What Hoglet is and isn't. Product positioning, target user, the pitch.
- What goes in a milestone and what gets cut. Any scope change, in either direction.
- When work starts. Agents do not begin implementation until told to.
- Whether to build, buy, fork, or skip a capability.

**Anything structural**
- The core stack: language, HTTP framework, query engine, storage format, embedded DB.
- Adding a dependency that becomes load-bearing, or a new service to the deployment.
- Data model and wire contract changes that break compatibility.
- Anything that touches the single-binary promise.

**Anything outward-facing**
- Licence. Repo visibility. Naming. Public claims and benchmarks.
- Publishing, releasing, announcing. Anything a stranger could read.
- Pricing, hosting, commercial model.

**Anything irreversible or expensive**
- Deleting or rewriting existing work. Force pushes. History edits.
- Long agent fan-outs beyond a quick lookup.
- Anything that spends real money.

## The CTO decides. Proceed without asking.

**Implementation detail inside an approved milestone**
- Module and crate layout, function signatures, internal naming.
- Error handling, logging, tracing approach.
- Which small utility crate to pull for a bounded job.
- Test strategy, fixtures, CI wiring.
- Refactors that don't change behaviour or public surface.

**Correctness and quality**
- Fixing bugs found along the way.
- Rejecting an approach that's unsafe, unsound, or violates the engineering standard in CLAUDE.md.
- Flagging that a settled decision now looks wrong — flag it, argue it, but don't act on it alone.

**Research and analysis**
- How to investigate a question the boss asked.
- Which sources to read, how to structure findings.

**Working style**
- How to break a task into steps. What to do first.
- When to stop and report rather than push on.

## Grey areas — surface, don't decide

If it's genuinely ambiguous, say so in one line and give a recommendation. Don't silently pick and don't stall waiting for perfect clarity.

Live examples right now:
- Whether to ever ingest session replay. Currently out of scope.
- Whether to implement OpenFeature alongside PostHog flag compat.

## Locked decisions (boss, 2026-07-20)

Stack: Rust (tokio+axum), hand-rolled WAL, Parquet+DuckDB (events), SQLite (OLTP), React+TS embedded via rust-embed with ts-rs. Full detail in `stack.md`.

- **SQLite over Turso.** Turso's MVCC — its only advantage for us — is flagged not-production-ready by its own manual. Revisit at Turso 1.0 + stable MVCC + frozen format.
- **React over Dioxus/topcoat** (component layer: shadcn/ui on Base UI, see "UI stack").** No maintained Rust library does interactive analytics charts; charts are the whole dashboard.
- **DuckDB over DataFusion**, used read-only as a query engine over Parquet segments. Never a live read-write `.duckdb` file (single-writer wall).
- **traceparent deferred.** Store the header's trace_id as an event property if free; build no query/UI until users ask. A hypothesis, not a differentiator.
- **Scope ladder:** now = client+server events, identity, flags (AI events captured free via compat). Later = error tracking (exceptions are events), AI analytics dashboard, MCP server over the query API. Never = traces/logs/metrics including LLM trace waterfalls — store trace_id, link out.
- **One binary, two lanes.** Ingest+WAL get reserved capacity and bounded queues; DuckDB queries run on a capped blocking pool with a memory limit. Ingest always wins under contention.

## Launch decisions (boss, 2026-07-21)

- **License: AGPL-3.0**, sole copyright holder, cloud/commercial option open later.
- **Build private, launch big.** No public repo until milestone one + demo + measured numbers. One shot at Show HN.
- **Name: Hoglet** (locked). Tagline: "PostHog-compatible product analytics. One binary. One $5 server."
- **Community: Discord**, set up launch week, not before.

## Standing build method

Oracle + property-based testing + shadow mode, per hard component (proven by PostHog's own parser rewrite):
- Wire edge: real PostHog SDKs are the oracle (contract tests, nightly, SDK `latest`)
- Identity: proptest permutes merge order, asserts convergence
- WAL: turmoil fault injection, kill at adversarial points
- Funnel operator: slow obvious SQL is the oracle for the fast path
- Query semantics (later): same events into real PostHog and Hoglet, diff the insight results

## Recorded corrections

Lessons promoted here so they survive compaction. Do not repeat these mistakes.

**DataFusion is not a database; DuckDB is.** (boss, 2026-07-20) DataFusion is a query-engine toolkit — no storage, no catalog, no transactions, no persistence. DuckDB is a complete embedded database. Any comparison that treats them as peer query engines is wrong: choosing DataFusion means building a database around it. This raised the cost of the DataFusion option materially and tilted the standing recommendation toward DuckDB for v1.

**Do not conflate research with progress.** (CTO, 2026-07-20) Four parallel research fan-outs produced real findings, but the repo still has zero lines of Rust. Research is only justified when it changes a decision that is about to be made.

## Enforcement

Read this before acting. If a task would cross into the boss's column, stop and ask — even if the work seems obvious, even if it's small, even if the session is long. An unrequested "improvement" to something in the boss's column is a defect, not initiative.

## Rebuild decisions (CTO, 2026-10-04; boss asked to "make it super great")

Made inside the approved rebuild; listed so the boss can veto any of them.

- **Legacy stack and offline migration deleted.** No installation runs the
  pre-release v1 layout. Pre-release data directories must be recreated
  (projections schema 2 fails loud on schema 1).
- **Event lake = generation-tracked file catalog.** Files carry created/retired
  generations; publication and compaction cost O(files touched). Replaced the
  full-manifest-per-publish design that collapsed ingest to 684 events/s after
  280k events. Compaction (DuckDB) sorts by (event, timestamp) and drops uuid
  duplicates — eventual dedup, like PostHog's ReplacingMergeTree.
- **Publication left the ack path.** Writer thread: group commit, one fsync per
  group. Publisher thread: Parquet, projections, compaction, erasure.
- **Parquet event schema 2** promotes web-analytics fields to columns;
  readers always use `union_by_name`, so the schema may only grow.
- **Person ids are the first distinct id.** Queries resolve persons through an
  incrementally synced override table (`identity_state.seq/epoch`).
- **Personal API keys gain a scope** (read default, write opt-in). Write keys
  act like the owner's session, still bounded by the owner/admin role.
- **`ureq` (rustls) added** for the PostHog importer, reconcile and shadow-mode
  forwarding. Not on the ingest path; capture never waits on it.
- **Shadow mode forwards after the durable ack** through a bounded, droppable
  queue. Forwarding may drop under overload; Hoglet's own copy never does.
- **posthog-js surveys loader stub** at `/static/surveys.js` — the SDK loads it
  even with surveys off; a no-op extension avoids console errors.
- **Demo data** (`HOGLET_DEMO=1`, `POST /demo`) goes through the real pipeline.

## UI stack (boss, 2026-10-05)

Dashboard stays React + TypeScript (Vite, embedded via rust-embed) and moves its
component layer to **shadcn/ui on Base UI** (not Radix). Components are copied
into the repo (shadcn model), Tailwind is a build-time dependency only. Charts
stay hand-written SVG: no charting library, no recharts. `web/dist/` stays
committed so `cargo build` needs no Node.
