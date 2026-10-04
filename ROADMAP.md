# Hoglet Roadmap

**North star:** a PostHog team switches in an afternoon, keeps every SDK call,
keeps their history, sees the same numbers, and never goes back.

How that kills PostHog for the self-host and cost-sensitive market:
- **Wire compat gets them in.** Change `api_host`, nothing else.
- **Import + shadow mode make switching free and safe.** Bring history, run
  side by side, cut over when the numbers match.
- **Things PostHog's stack can't do keep them.** One binary, numbers you can
  defend, flags that never go down, data that is just files you own.

Per-feature status lives in `spec/README.md`. This file sets order, cuts, and
exit criteria. **[boss]** marks a decision that needs a go-ahead.

---

## Audit, 2026-10-04: what's wrong today

1. **Two apps in one tree.** `lib.rs::app_with_state` (v1: `auth.rs`,
   `catalog.rs`, `dashboard_store.rs`, `file_index.rs`, `flush.rs`,
   `cohort.rs`, `routes/{api,catalog,dashboards,admin}.rs`) still compiles
   beside the real app in `application.rs` (v2: control/projections/event lake).
   Production serves v2. v1 is dead weight with tests that prove nothing users see.
2. **Migration code for users who don't exist.** `migration.rs`,
   `legacy_import.rs`, `legacy_resources.rs` (~2,100 lines) plus the `migrate`
   subcommand upgrade Hoglet's own pre-release layout. Nobody runs v1.
3. **"Single static binary" is false.** The release build is 60 MB and
   dynamically links libstdc++ and glibc. `launch.md`'s `FROM scratch` image
   would not start.
4. **The docs contradict each other.** `why-hoglet.md` says MIT/Apache, but the
   licence is AGPL-3.0. It also promises a Rust-core SDK, but native SDKs are out
   of scope. `stack.md` still budgets for Turso, which was rejected. `CLAUDE.md`
   "milestone one" is finished and still listed as the current scope.
5. **Unproven claims.** No fault injection beyond one SIGKILL test, no identity
   proptest, no measurement on a 1 GB box, no python SDK contract test.

## Cut from the plan

- Rollups: only if the R1 benchmark misses. DuckDB over pruned Parquet may be enough.
- Drag-layout dashboards: a fixed grid is enough.
- A docs site generated from `spec/`: specs are internal. Write user docs instead.
- "Emoji terminal output" and similar launch-checklist filler.
- The Rust-core/WASM SDK: we win by *not* forking the SDK ecosystem.
- Turning trace correlation into a feature: still just store `trace_id`.

---

## R0 — Clean house (≈1 week)

- Delete the v1 stack and the legacy migration path. One app constructor, one
  set of stores, tests rewired to `application.rs`. **[boss: deletion]**
- Real one-binary build: static musl (or zig-cc) target with DuckDB bundled.
  If that fails, claim "one binary, glibc ≥2.31" honestly and use a
  `distroless/cc` image. Add CI to measure binary size.
- Fix the contradictions in `why-hoglet.md`, `stack.md`, `CLAUDE.md`.
  Reset `spec/README.md` status marks to match the code.
- Exit: `cargo test` covers only code that ships; `ldd` output matches the claim.

## R1 — Trust (the moat, proven)

- Claim 2: kill-point fault injection across WAL → publish → generation swap
  (torn write, ENOSPC, crash between publish and WAL delete). Reconcile acked
  events against recovered events, seeded and in CI.
- Claim 4: proptest over identify/alias/merge permutations. Converge to one
  identical person graph.
- Claim 1: contract tests for posthog-js, posthog-node, and posthog-python at
  `latest`, nightly. Cover flags, local eval, `beacon=1` → 204, retry semantics.
- **Flags isolation test:** `/flags` p99 stays sub-ms while a 10M-event funnel
  runs. This is the direct answer to PostHog's three flag outages.
- **Ingestion lag in the UI:** if numbers are behind, the dashboard says so.
- Claim 3: benchmark on a rented 1 vCPU / 1 GB box: sustained ingest plus a
  concurrent funnel over ≥10M events. Numbers go into `claims.md`. This decides
  whether rollups come back.
- Exit: every public claim has evidence running in CI or a recorded measurement.

## R2 — The switch (the innovation PostHog can't block)

- **`hoglet import posthog`**: pull events, persons, distinct_id map, flags,
  cohorts, insights, and dashboards from a PostHog project through its API with
  a personal key. Resumable, idempotent on uuid.
- **Shadow mode**: Hoglet acks from its own WAL, then forwards the same events to
  PostHog from a WAL-tailing, bounded, droppable lane that never sits in the ack
  path. A **reconcile report** diffs Hoglet's numbers against PostHog's query
  API per insight. "Run us next to PostHog for a week, watch the numbers match,
  cut over" is the pitch.
- **PostHog management-API compat for what tooling touches:**
  `/api/feature_flag/local_evaluation`, flag CRUD, personal API keys, so
  server SDKs and existing scripts work unchanged.
- Exit: a real PostHog project moves to Hoglet with history and matching
  numbers, without code changes.

## R3 — The product a team opens daily

In order of who it wins:
1. **Web analytics screen**: visitors, pageviews, bounce, top pages,
   referrers, UTM, countries, devices. Plausible simplicity on top of real
   product depth. Add cookieless mode (salted daily hash) and a GeoIP file
   (DB-IP lite, CC-BY).
2. **Persons + identity audit**: profile, event timeline, merge history,
   distinct_id→person ratio. The visible answer to ghost profiles.
3. **Flags complete**: experience continuity, cohort targeting, payloads, UI edit.
4. **Cohorts**: behavioral (did / did-not / first-time), scheduled recompute.
5. **SQL tab**: plain DuckDB SQL over your own events. No HogQL to learn,
   capped and read-only.
6. **Paths-basic**: top next/previous events.
7. **Data management**: event and property definitions, verified/hidden, and
   a taxonomy-rot detector (one-off events, `_v2_FINAL` near-duplicates).
- Exit: the `spec/README.md` v1 definition holds for a design partner on real
  traffic.

## R4 — Charm (things a cluster can't be)

- **`hoglet dev`**: zero-config local mode, live event tail in the terminal,
  and the same dashboard on localhost while you build.
- **Your data is files**: day-partitioned Parquet you can open in
  DuckDB/Polars; optional S3/R2 tiering via `object_store` for cheap retention;
  backup is `cp`.
- **MCP server** (`hoglet mcp`): agents query trends, funnels, and persons
  through the typed query API. Pulled forward from "later": cheap on top of
  the IR, and agents are how people will query analytics.
- **Signed flag bundles** for edge/offline evaluation, so flags survive Hoglet
  itself being down.

## R5 — Launch **[boss]**

- Release binaries (linux/macOS × amd64/arm64), container image, install
  script, `HOGLET_DEMO=1`, README with demo GIF, user docs (quickstart, SDK
  setup, migrate-from-PostHog, deploy).
- **3–5 private design partners on real traffic before Show HN.** A one-shot
  launch with zero production users is the riskiest bet in the old plan.
- Repo polish: CONTRIBUTING, SECURITY, CHANGELOG, licence headers,
  history secret scan.

## After launch (ordered, not scheduled)

1. Error tracking: exceptions are events, so it rides the same pipeline.
2. AI analytics screen: cost, model, and latency over `$ai_generation`.
3. **Session replay [boss]**: after analytics, replay is PostHog's most-used
   product, and many teams can't switch without it. Recommend storage-only
   rrweb ingest to local disk or object store with a bundled player, no
   processing pipeline. Revisit after design-partner feedback.

Never: traces/logs/metrics. Out of scope: surveys, experiments, warehouse,
CDP, multi-node.

---

## Decisions needing the boss

- Delete the v1 stack and legacy migration (R0). Recommend yes.
- Binary claim: chase a fully static build, or claim "one binary" on glibc.
  Recommend trying musl for one day, then falling back honestly.
- Design partners before Show HN instead of a cold one-shot launch. Recommend yes.
- Pull the MCP server forward from "later" into R4. Recommend yes.
- Move session replay from "out of scope" to "post-launch, storage-only".
  Recommend yes.
- Shadow-mode forwarding to PostHog: outbound traffic to a third party, off by
  default and opt-in per project. Recommend yes.
- Add `proptest` and `turmoil` as dev-dependencies. Recommend yes.
