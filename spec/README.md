# Hoglet — architecture and status

PostHog-compatible product analytics in one Rust binary. Stock PostHog SDKs
point at it and work. Rules: `../CLAUDE.md`. Decisions: `../decisions.md`,
`../docs/adr/`. Claims and their evidence: `../claims.md`. User docs:
`../docs/user/`. Roadmap: `../ROADMAP.md`. API shapes: `../src/contract/`.

## Architecture

```
SDK ─HTTP─▶ capture ─▶ WAL (group-commit fsync) ─ack▶ 2xx
                          │ seal each second
                          ▼
                 publisher thread ─▶ Parquet (per project/day) + identity + catalog
                          │              (one SQLite transaction advances the WAL checkpoint)
                          ▼
              compactor (merge, dedup by uuid, retention, erasure)
                          ▲
   DuckDB (read-only, capped) ◀─ typed query engine ◀─ /api/projects/{id}/query
                                  (identity-resolved persons, Rust oracles)
```

- **Events:** `events/<project>/<day>/*.parquet`, schema v2 (web-analytics
  fields promoted to columns), catalogued with generations in `projections.db`.
- **State:** `control.db` (accounts, projects, keys, flags, insights,
  dashboards, forwarding); `projections.db` (persons, identity, catalog, lake
  catalog). One writer per resource.
- **Edge:** PostHog wire semantics byte-for-byte (`wire-compat.md`).

## Invariants

1. A 2xx means the event is fsynced in the WAL.
2. Recovery drops only an unacknowledged torn tail; other damage refuses to start.
3. Publication is one SQLite transaction: projections, file rows, WAL checkpoint.
4. One person per (project, distinct id); person counts always go through identity.
5. Queries read leased snapshots; retired files are unlinked after the last lease.
6. Every query kind has an independent brute-force Rust oracle.
7. No DuckDB extension is ever loaded (the binary is static).
8. Overload sheds with 503; ingest is never starved by queries, and publication
   never starves erasure or shutdown.

## Status (2026-10)

Built and tested: capture and every PostHog endpoint, flags (all shapes, local
evaluation), identity, durable pipeline with crash/corruption evidence, lake
with compaction/retention/erasure, trends/funnels/retention/lifecycle/
stickiness/paths/SQL/actors, web analytics, persons, events, catalog, dashboard
UI, import/shadow/reconcile, static release builds.

Open: measured numbers on a real 1 vCPU / 1 GB VPS, cohorts, error tracking,
AI analytics views, MCP server, GeoIP, public share-link charts (needs a
decision), person-property filters on the feed/web endpoints.
