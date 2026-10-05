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
  posthog-js, posthog-node and posthog-python `latest` (43 + 25 + 47 checks).
- **Durable pipeline:** group-commit WAL, publisher thread, generation-stamped
  Parquet lake, compaction with uuid dedup, retention, physical erasure.
  Crash evidence: 24 kill points × randomized runs, WAL corruption fuzzing, I/O
  error injection (`claims.md` claim 2); it found and fixed three real bugs.
- **Identity:** identify / alias (both SDK directions) / merge_dangerously /
  $set / $set_once / $unset / groups, property-tested (`claims.md` claim 4).
- **Flags:** PostHog-exact bucketing, conditions, variants, payloads,
  continuity, CRUD, local evaluation.
- **Analytics:** trends, funnels, retention, lifecycle, stickiness, paths,
  SQL, actors drill-down for every kind; web analytics; persons; live
  activity; catalog. Every kind has an independent oracle.
- **Teams and security:** roles, invites, offline password reset; audited and
  hardened (CSRF guard, login throttling, SSRF policy, amplification limits,
  CSP); `SECURITY.md`.
- **The switch:** `hoglet import posthog`, shadow-mode forwarding,
  `hoglet reconcile posthog`.
- **Product:** dashboard on shadcn/ui + Base UI + TanStack (all insight kinds,
  accessible, responsive), demo data, scoped API keys, user docs in
  `docs/user/`, OpenAPI reference.
- **Packaging:** fully static Linux binary (amd64/arm64), `FROM scratch` image
  with built-in healthcheck, installer, systemd unit, proxy configs.

## Now — to launch

1. Measure on a real 1 vCPU / 1 GB VPS and put the numbers in `claims.md`.
   (Emulated cap, 10M events: ingest ~22k events/s at ~194 MB RSS; DAU 0.7 s,
   WAU 0.6 s, funnel 2.3 s, web overview 2.6 s. Misses are listed in claims.md.)
2. First tag dry run of `release.yml` (macOS builds are unverified) and the
   CI jobs on a real runner.
3. **[boss]** merge `make-it-real` into `main`; container image name; launch
   date; 3–5 design partners before Show HN.

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
