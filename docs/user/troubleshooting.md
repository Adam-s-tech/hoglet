# Troubleshooting

## No events appearing

Work down this list. Each step tells you which side of the problem you are on.

1. **Is Hoglet up and ready?**

   ```sh
   curl -s -o /dev/null -w '%{http_code}\n' https://analytics.example.com/health   # 200
   curl -s -o /dev/null -w '%{http_code}\n' https://analytics.example.com/ready    # 200
   ```

   Hoglet opens its port only after it has recovered the write-ahead log, so
   during start-up the connection is refused. `/ready` returns `503` once
   shutdown begins. If the service is down, see
   [Hoglet will not start](#hoglet-will-not-start).

2. **Can your app's network reach it? Send one event by hand** from the machine
   or browser that is failing:

   ```sh
   curl -i -X POST https://analytics.example.com/i/v0/e/ \
     -H 'Content-Type: application/json' \
     -d '{"api_key":"phc_your_project_token","event":"debug_ping","distinct_id":"debug"}'
   ```

   `200 {"status":1}` means capture works; the problem is in your SDK setup.
   `401`: wrong token. `400`: malformed body. `413`: too big. `429`: rate
   limit. `502`/`504` from a proxy: the proxy cannot reach Hoglet.

3. **Read the counters.** `/metrics` needs no login (from the server itself,
   `curl http://127.0.0.1:8000/metrics`):

   ```
   hoglet_events_captured_total   events accepted into the pipeline
   hoglet_events_acked_total      events durably acknowledged
   hoglet_requests_rejected_total requests answered with a 4xx
   hoglet_sink_errors_total       retryable failures (503)
   ```

   - `captured` does not move when you send events: requests are not arriving.
     Check the host, token, proxy, DNS, TLS, and ad-blockers.
   - `rejected` goes up: they arrive and are refused. The browser network tab or
     a `curl -i` shows which `4xx`.
   - `sink_errors` goes up: Hoglet is answering `503`. Look at the disk (full?)
     and the logs.
   - `acked` goes up but you see nothing in the dashboard: go to step 4.

4. **Check ingestion.** With a personal API key:

   ```sh
   curl -s -H 'Authorization: Bearer phx_...' \
     https://analytics.example.com/api/projects/<project-id>/status
   ```

   - `has_events: false` and `ingestion_lag_seconds: 0`: nothing stored for this
     project. Are you looking at the right project? Each project has its own
     token; the sidebar's project switcher shows which one is open.
   - `ingestion_lag_seconds` above a few seconds: acknowledged events are waiting
     to be published. It normally returns to 0 by itself. If it keeps growing,
     check `journalctl -u hoglet` for errors and the free disk space. The
     sidebar indicator says "N behind" in the same situation.
   - `stored_events` is rising but your charts are empty: check the date range
     and filters. Everything is UTC, and `timestamp` can be wrong (see
     [Clock skew](#clock-skew-and-wrong-dates)).

5. **Browser SDK checks.** In the browser's network tab, look for requests to
   `<api_host>/i/v0/e/` (or `/e/`) and `<api_host>/array/<token>/config`.
   - Blocked (`net::ERR_BLOCKED_BY_CLIENT`): an ad-blocker. See
     [Ad-blockers](#ad-blockers).
   - No request at all: `posthog.init` did not run, or capturing is opted out.
     posthog-js also drops events from automated browsers (headless or
     `navigator.webdriver`) unless you pass `opt_out_useragent_filter: true`.
   - CORS errors: a proxy in front of Hoglet is stripping or replacing the
     `Access-Control-*` headers Hoglet sends on the SDK endpoints.

6. **Server SDK checks.** posthog-node and posthog-python batch events. A
   short-lived script must call `shutdown()` (or `flush()`) before it exits, or
   the queued events are never sent.

## 401 and 403

| Where | Cause |
|---|---|
| Capture (`/e/`, `/batch/`), `/flags`, `/array/{token}/config` | The token is missing, malformed, or not a project of this Hoglet. A `phx_` personal key is not accepted as a token. A project created a moment ago may need a few seconds: unknown tokens are remembered as unknown for up to 10 seconds. |
| Dashboard API | No credentials, an expired session (they last 7 days; log in again), a mistyped or revoked key, or a header that is not exactly `Authorization: Bearer phx_...`. |
| Local evaluation (`/flags/definitions`) | No `Authorization: Bearer phx_...` header, or `token=phc_...` missing or unknown (`401`); the key's user cannot access that project (`403`). |
| `403` on a write | A `read` key was used for a change, or the user is a `member`. Use a `write` key from an owner or admin. |
| `403` on `/api/auth/keys` and similar | Account management needs a session; keys cannot do it. |

## Ad-blockers

Symptoms: events from some visitors only, `ERR_BLOCKED_BY_CLIENT` in the
network tab, or requests to your Hoglet host that never reach the server (they
do not appear in `hoglet_events_captured_total`).

Fix: serve Hoglet from your own domain and an unremarkable path, and set
`api_host` to it. Setup for Next.js, Caddy and nginx is in
[SDK setup](sdk-setup.md#first-party-proxy-ad-blockers). Some visitors will
still block all analytics; Hoglet cannot see those.

## Clock skew and wrong dates

Hoglet places an event in time with this order of precedence:

1. `offset` (milliseconds in the past) if present.
2. Otherwise the event's `timestamp`, corrected for the sender's clock: the
   batch's `sent_at` (or the `_` query parameter) minus the server's time is
   subtracted. This is why events from a device with a wrong clock still land
   on the right day, provided the SDK sends `sent_at`. Set the event property
   `$ignore_sent_at: true` to skip the correction.
3. With no timestamp, the time Hoglet received it.

A timestamp more than 23 hours in the future becomes "now". Events are filed
by UTC day.

If events land on the wrong day: check the **server's** clock (`timedatectl`;
enable NTP); check that custom HTTP senders send an ISO 8601 `timestamp` in UTC
(or none at all); remember that dashboards use UTC, so a late-evening event in
your time zone is tomorrow in UTC.

## Duplicate persons and wrong person counts

Person counts follow merge rules (see
[Insights and queries](insights-and-queries.md#people-and-identity)). Common
reasons for two people where you expect one:

- **No identify.** Anonymous activity only merges into a user when `identify`
  (posthog-js) is called, or an `$identify` event carries `$anon_distinct_id`
  (server SDKs: pass it in the identify properties).
- **Different ids.** The web app identifies with a database id, the backend with
  an email. Pick one id per user everywhere.
- **Both sides already identified.** Two people who were each identified are
  never merged by `$identify` or `$create_alias`, by design (two users on one
  shared device must stay apart).
- **`posthog.reset()` not called on logout**, so the next user on that browser
  inherits the previous user's id.

To merge two people deliberately, send `$merge_dangerously`. It always merges
the `alias` id into `distinct_id`:

```sh
curl -X POST https://analytics.example.com/batch/ \
  -H 'Content-Type: application/json' \
  -d '{"api_key":"phc_your_project_token","batch":[{"event":"$merge_dangerously","distinct_id":"user_123","properties":{"alias":"other_id"}}]}'
```

Merges apply to past events and cannot be undone. Look at a person's `distinct_ids` on the Persons page before and after.

## Disk growth

Check how big events are and how fast they grow:

```sh
du -sh /var/lib/hoglet/*                      # which part is big
curl -s -H 'Authorization: Bearer phx_...' https://analytics.example.com/api/projects/<project-id>/status
```

`stored_bytes` and `stored_events` give bytes per event for your data.

- **`events/` is large:** that is your data. Set `HOGLET_RETENTION_DAYS` to
  drop old days, or add disk. Retention removes whole UTC days.
- **`wal/` is large (hundreds of MB):** publication is behind. Check
  `ingestion_lag_seconds` and the logs. Capture answers `503` once 1 GiB is
  waiting.
- **`tmp/` is large:** queries that spill to disk. It is scratch space. Very
  large time ranges in funnels or SQL use the most.
- **Backups piling up:** they are not in the data directory, but check that
  they are not on the same disk.
- A person erasure rewrites that person's day files and does not shrink the
  directory immediately.

## High memory

Hoglet's memory is bounded by explicit caps, but several pools each have their
own cap (dashboard queries, the persons/events/web screens, compaction), so
busy dashboards can add up on a 1 GB machine. Under systemd the unit throttles
at 640 MB (`MemoryHigh`) and restarts the service at 768 MB (`MemoryMax`).

- Is it queries? Memory rises when someone runs a heavy funnel, a long-range
  path analysis or a large SQL query, and falls afterwards. Narrow the date
  range, or ask people not to run several at once. A query that exceeds its cap
  fails with `400 query_too_large`.
- Is it restarts? `journalctl -u hoglet | grep -i -E 'killed|oom|memory'` and
  `systemctl status hoglet`. After a restart, acknowledged events are recovered
  from the write-ahead log, so nothing acknowledged is lost.
- There are no settings for these caps in this version. If the machine is too
  small for your query load, give it more RAM.

## Hoglet will not start

Run it in the foreground and read the message:

```sh
HOGLET_DATA=/var/lib/hoglet HOGLET_ADDR=127.0.0.1:8000 /usr/local/bin/hoglet
```

| Message | Fix |
|---|---|
| `HOGLET_ADDR must be a socket address` | Use `IP:port`, for example `127.0.0.1:8000`. Hostnames are not accepted. |
| `HOGLET_RETENTION_DAYS must be a number` (or another variable) | Fix the value of the named variable. |
| `Address already in use` | Another process has the port (`ss -ltnp`). |
| `Storage(Io { ... PermissionDenied ... })` or `Permission denied` on the data directory | The service user must own it. Under systemd: `chown -R hoglet:hoglet /var/lib/hoglet`. For Docker bind mounts: owner `65532:65532`. |
| `MigrationIncomplete { data_dir: ... }` | One of `control.db` and `projections.db` is missing: the directory is damaged or partly restored. Restore the whole directory from one backup. |
| Other storage errors (unknown schema, mismatched database pair) | Hoglet refuses to guess. Do not mix files from different backups. Restore a matching backup, or use the version that wrote the data. |

## Dashboard says "Server unreachable" or "N behind"

"Server unreachable" means the browser cannot reach Hoglet (proxy, network, or
Hoglet is down). "N behind" is ingestion lag: acknowledged events not yet
published; see step 4 above.

## SQL tab errors about ICU or INTERVAL

`date_trunc`, `extract`, casting `timestamp` to a date, and
`timestamp - INTERVAL ...` are not available in the SQL tab. Use `strftime` and
`epoch_ms`; examples are in
[Insights and queries](insights-and-queries.md#sql-tab).

## Feature flag looks wrong

Open the flag, "Test a user", and enter the `distinct_id`. It shows the value
and the reason (`no_condition_match`, `out_of_rollout_bound`, `disabled`,
`condition_match`). Things to check: the person's stored properties (Persons),
that the SDK passes `person_properties` for properties Hoglet has not seen yet,
and, with local evaluation, that the SDK's polling interval has elapsed.
See [Feature flags](feature-flags.md).
