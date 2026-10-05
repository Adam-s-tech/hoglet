# 🦔 Hoglet

**PostHog-compatible product analytics. One binary. One $5 server.**

Point your existing PostHog SDKs at Hoglet — change `api_host`, nothing else —
and get product analytics, web analytics and feature flags from a single Rust
binary. No ClickHouse, Kafka, Redis, Zookeeper or Postgres. Your data stays on
your server, as plain Parquet files you own.

```sh
HOGLET_DEMO=1 ./hoglet        # 90 days of realistic data, ready in a second
# open http://localhost:8000  — demo@hoglet.dev / hoglet-demo-1
```

## Quick start

```sh
./hoglet                                     # listens on 127.0.0.1:8000
docker run -p 8000:8000 -v hoglet:/data ghcr.io/debpalash/hoglet   # or the container
```

Open the dashboard, create your account and project, then point any PostHog
SDK at it:

```js
posthog.init('phc_your_project_token', { api_host: 'https://analytics.example.com' })
```

```python
posthog = Posthog('phc_your_project_token', host='https://analytics.example.com')
```

Put Hoglet behind any TLS reverse proxy (Caddy, nginx) — see [`deploy/`](deploy/).

## Documentation

User docs are in [`docs/user/`](docs/user/README.md):
[quickstart](docs/user/quickstart.md),
[SDK setup](docs/user/sdk-setup.md),
[migrate from PostHog](docs/user/migrate-from-posthog.md),
[deploy](docs/user/deploy.md),
[data and backup](docs/user/data-and-backup.md),
[configuration](docs/user/configuration.md),
[insights and queries](docs/user/insights-and-queries.md),
[feature flags](docs/user/feature-flags.md),
[API](docs/user/api.md),
[FAQ](docs/user/faq.md) and
[troubleshooting](docs/user/troubleshooting.md).
A running instance serves its API reference at `/docs`.

## What you get

- **Every PostHog SDK, unchanged.** Capture on every PostHog endpoint, gzip and
  base64 bodies, `sendBeacon`, retries with PostHog's exact response codes.
  Proven nightly against posthog-js, posthog-node and posthog-python `latest`.
- **Insights.** Trends (every PostHog math, breakdowns, formulas, period
  comparison), funnels (sequential, strict, any order, conversion windows,
  exclusions, time to convert), retention, lifecycle, stickiness, paths, and
  plain SQL over your events. Click any number to see the people behind it.
- **Web analytics.** Visitors, pageviews, sessions, bounce rate and session
  duration with period-over-period change, pages, entry/exit pages, referrers,
  UTMs, browsers, devices and countries (when events carry
  `$geoip_country_code`) on one screen.
- **Honest person counts.** `identify`, `alias` and `merge_dangerously` follow
  PostHog's merge rules and every person count goes through them — an
  anonymous visitor who logs in is one person, not two.
- **Feature flags that can't go down with your dashboard.** PostHog's exact
  bucketing (same users in the same rollout as PostHog would pick), release
  conditions, multivariate variants, payloads, experience continuity, and
  local evaluation for server SDKs. Evaluated in-process, isolated from
  analytics queries.
- **Data you can trust.** An event acknowledged with a 2xx is on disk; a
  `kill -9` at any moment loses nothing acknowledged. The dashboard always
  shows how fresh the numbers are.
- **Persons, activity, dashboards, sharing, API keys,** and physical GDPR
  erasure of a person and all their events.

## Switching from PostHog

1. **Shadow mode.** Point SDKs at Hoglet and turn on forwarding (`PUT /api/projects/{id}/forwarding`): every event still reaches PostHog, so both see identical data.
2. **Bring your history.**
   `hoglet import posthog --posthog-project 12345 --posthog-key phx_… --token phc_…`
   copies events, identity merges, person properties and feature flags.
   Re-running resumes and never duplicates.
3. **Check the numbers.**
   `hoglet reconcile posthog …` compares events and unique people per event
   per day on both systems and flags any difference.
4. Turn forwarding off. Done.

## Configuration

| Env var | Default | Meaning |
|---|---|---|
| `HOGLET_ADDR` | `127.0.0.1:8000` | listen address |
| `HOGLET_DATA` | `./hoglet-data` | data directory |
| `HOGLET_RETENTION_DAYS` | keep all | delete events older than N days |
| `HOGLET_MAX_EVENTS_PER_SEC` | `10000` | per-project capture limit |
| `HOGLET_COOKIELESS_SALT` | off | cookieless device ids with this secret salt |
| `HOGLET_DEMO` | off | `1` on a fresh data dir: demo account + data |

`hoglet --help` lists everything.

## How it works

One process, two lanes. Ingest writes each batch to a write-ahead log and
acknowledges after one group fsync; a separate publisher turns the log into
day-partitioned Parquet files and applies identity, while a compactor keeps
partitions few and duplicate-free. Queries run DuckDB over exactly the files a
query needs, on a capped pool that can never starve ingest. Accounts, flags
and identity live in SQLite. Details: [`spec/README.md`](spec/README.md).

```
data/
  control.db      accounts, projects, flags, insights, dashboards
  projections.db  persons, identity, catalog, file catalog (back it up with the rest)
  events/         <project>/<YYYY-MM-DD>/*.parquet — query them with anything
  wal/            acknowledged events not yet in Parquet
```

## Testing

```sh
cargo test                          # unit, integration, crash and oracle tests
scripts/contract-test.sh all        # real PostHog SDKs against a real Hoglet
scripts/loadtest.sh                 # sustained ingest with a racing query
```

## License

AGPL-3.0.
