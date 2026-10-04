# Hoglet Roadmap

**What we're doing:** our own PostHog backend — one program on a $5 server
that apps already using PostHog switch to by changing one URL, so teams keep
product analytics, web analytics and feature flags without PostHog's bill or
infrastructure.

**How it wins:** compatibility gets teams in (change `api_host`), the switch
is free and safe (import history, run side by side, reconcile the numbers),
and things a cluster can't be keep them (one binary, numbers you can defend,
flags that don't go down with the dashboard, data as files you own).

Per-feature status: `spec/README.md`. Decisions: `decisions.md`. **[boss]**
marks a call that needs a go-ahead.

---

## Done (2026-10)

- **Wire edge:** every PostHog capture endpoint, config, flags/decide in all
  shapes, local evaluation, surveys loader stub. Contract-tested against
  posthog-js, posthog-node and posthog-python `latest`.
- **Durable pipeline:** group-commit WAL, publisher thread, generation-tracked
  Parquet lake, compaction with uuid dedup, retention, physical erasure.
  SIGKILL during concurrent ingest loses nothing acknowledged.
- **Identity:** identify / alias / merge_dangerously / $set / $set_once /
  $unset / groups, applied to every person count through identity overrides.
- **Flags:** PostHog-exact bucketing (verified against PostHog's own
  consistency vectors), conditions, variants, payloads, continuity, CRUD.
- **Analytics:** trends, funnels, retention, lifecycle, stickiness, paths,
  SQL, actors drill-down; web analytics; persons; live activity; catalog.
- **The switch:** `hoglet import posthog`, shadow-mode forwarding,
  `hoglet reconcile posthog`.
- **Product:** rebuilt dashboard (all insight kinds, web analytics, persons,
  flags editor, dashboards, sharing, onboarding, demo data), scoped API keys.

## Now — launch-ready v1

1. Integrate and harden: one DuckDB memory budget for all readers, contract
   suites green on all three SDKs, OpenAPI docs regenerated from the contract.
2. Measure on a real 1 vCPU / 1 GB VPS: sustained ingest, concurrent funnel
   over ≥10M events, cold start, recovery. Numbers into `claims.md`.
   (Emulated with a cgroup cap: 18k events/s, 343 MB peak RSS.)
3. Static release binaries, container image, install script, systemd unit.
4. User docs: quickstart, SDK setup, migrate from PostHog, deploy, backup.
5. **[boss]** 3–5 design partners on real traffic before Show HN.

## Next — after launch, in order

1. **Cohorts** (behavioral + static) as filters and flag targets.
2. **Error tracking:** `$exception` issues grouped by fingerprint, trends,
   affected people — exceptions are already events.
3. **AI analytics:** cost / model / latency / per-user views over
   `$ai_generation` (already captured).
4. **MCP server** over the query API so agents can ask analytics questions.
5. **Object storage tiering** (S3/R2) for cheap long retention.
6. **Session replay [boss]:** storage-only rrweb ingest + bundled player;
   decide after design-partner feedback.

## Never

Traces, logs, metrics (store `trace_id`, link out). Multi-node clusters.
Kafka, ClickHouse, Redis, Zookeeper in the deployment.

## Open decisions [boss]

- Public launch timing and repo visibility.
- Design partners before Show HN (recommended: yes).
- Session replay after launch (recommended: storage-only, post-feedback).
- Container image name and registry (`ghcr.io/<owner>/hoglet` assumed).
