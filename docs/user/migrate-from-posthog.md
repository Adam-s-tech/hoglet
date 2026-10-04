# Migrate from PostHog

The switch has four parts: bring history over, run both systems side by side
(shadow mode), check the numbers match, then cut over. You can stop at any
point and go back, because PostHog keeps receiving your events until you turn
forwarding off.

The commands below were run end to end against a stand-in PostHog API while
writing this page, not against PostHog Cloud. Try them on your real project
with a recent `--since` first.

## What you need

- Hoglet running with TLS (see [Deploy](deploy.md)) and a Hoglet account and project.
- A **PostHog personal API key** (`phx_...`) that can read queries and feature
  flags (scopes `query:read` and `feature_flag:read`), and your PostHog
  **project id** (PostHog, Settings, Project, ID).
- A **Hoglet personal API key** with `write` scope (Hoglet, Settings, API keys).
  Needed to import flags and to turn forwarding on from the command line.
- Your Hoglet **project token** and **project id** (Settings, Project).

PostHog API host: `https://us.posthog.com` (default) or
`https://eu.posthog.com`. PostHog ingestion host, used for forwarding:
`https://us.i.posthog.com` or `https://eu.i.posthog.com`.

### Optional: keep your PostHog project token

If Hoglet is brand new, you can create the first project with your existing
PostHog token so the only SDK change is the host:

```sh
curl -s -X POST https://analytics.example.com/api/auth/setup \
  -H 'content-type: application/json' \
  -d '{"email":"you@example.com","password":"a long passphrase","organization_name":"Acme","project_name":"Web","existing_project_token":"phc_your_posthog_token"}'
```

This is only available on first-run setup (the dashboard form does not expose
it). If you do this, shadow mode still needs a PostHog token to forward to, and
it can be the same value.

## 1. Import history

Run this while your SDKs still point at PostHog and forwarding is off. The
importer is an ordinary client of both systems: it reads PostHog through its
query API and writes to Hoglet's `/batch/` endpoint with your project token, so
every imported event takes the same durable path as live traffic.

```sh
hoglet import posthog \
  --posthog-project 12345 \
  --posthog-key phx_your_posthog_key \
  --token phc_your_hoglet_project_token \
  --hoglet-host https://analytics.example.com \
  --hoglet-key phx_your_hoglet_key \
  --hoglet-project <hoglet-project-id>
```

Add `--posthog-host https://eu.posthog.com` for the EU cloud. Without
`--hoglet-key` and `--hoglet-project`, flags are not imported.

What it does, in order:

1. **Events**, oldest first, 5,000 per page, sent to Hoglet in batches of 500.
   Each event keeps its PostHog `uuid` and timestamp. Hoglet drops repeated
   uuids when it compacts a day's files, so re-sending is harmless.
2. **People.** For every PostHog person, all their distinct ids are merged with
   `$merge_dangerously` and the person's current properties are applied with
   `$set`.
3. **Feature flags** (key, name, active, conditions, variants, payloads,
   experience continuity), created through Hoglet's flag API. A flag whose key
   already exists is skipped.

Options: `--since <ISO date>` (only events on or after), `--skip-events`,
`--skip-persons`, `--checkpoint <file>` (default
`.hoglet-import-<posthog-project>.json` in the current directory).

Output:

```
importing PostHog project 12345 from https://us.posthog.com into https://analytics.example.com
  events: 5000 imported
  ...
  persons: 1200 imported
  flags: 8 imported
done: 98765 events, 1200 persons, 8 flags
```

**Interrupted?** Run the same command again. Progress is saved after every
page in the checkpoint file and the import resumes there. HTTP 429 and 5xx
responses and network errors are retried with backoff (up to 8 attempts per
call, honoring `Retry-After`).

**Finished?** Once the checkpoint says events are done, running the command
again does not fetch newer events. To pull a later slice, use a new checkpoint
file and `--since` (see "Fill the gap" below).

Things to know before you run it:

- Hoglet's per-project capture limit (`HOGLET_MAX_EVENTS_PER_SEC`, default
  10,000) applies. The importer backs off on 429.
- If `HOGLET_RETENTION_DAYS` is set, events older than that are deleted by the
  background compactor soon after they arrive. Set retention after the import,
  or make sure it covers the history you want.
- Run the people step once. Running it again later re-applies PostHog's
  properties and would overwrite changes made in Hoglet since. For catch-up
  runs, pass `--skip-persons`.
- The importer adds synthetic identity events to Hoglet: one
  `$merge_dangerously` per extra distinct id and one `$set` per person with
  properties, tagged `$lib: hoglet-import` and dated the day you run the
  import. They show up in event counts (see step 3, Reconcile).

## 2. Shadow mode

Point your SDKs at Hoglet (`api_host`, see [SDK setup](sdk-setup.md)) and turn
on forwarding so PostHog still gets every event. Forwarding has no dashboard
screen; it is set through the API. `<hoglet-project-id>` is the id from
Settings, Project.

```sh
curl -s -X PUT https://analytics.example.com/api/projects/<hoglet-project-id>/forwarding \
  -H 'Authorization: Bearer phx_your_hoglet_key' \
  -H 'content-type: application/json' \
  -d '{"enabled":true,"host":"https://us.i.posthog.com","posthog_token":"phc_your_posthog_project_token"}'
```

`host` must be an `http(s)` URL and `posthog_token` a `phc_` key. The response
is the status; read it any time:

```sh
curl -s -H 'Authorization: Bearer phx_your_hoglet_key' \
  https://analytics.example.com/api/projects/<hoglet-project-id>/forwarding
# {"config":{...},"forwarded":1520,"dropped":0,"failed":0,"queued":0,"last_error":null}
```

How forwarding behaves:

- It happens after Hoglet has durably stored the event, so it never delays or
  fails a capture response.
- Events wait in a bounded in-memory queue (100,000 events across all
  projects) and go out in batches of 500, each with up to 5 attempts.
- If the queue is full, events are dropped and counted in `dropped`. Failed
  batches are counted in `failed` and the last error is in `last_error`.
- The queue is in memory. Events not yet forwarded when Hoglet restarts are
  not forwarded afterwards. Hoglet itself keeps them.
- Forwarded events keep Hoglet's `uuid`, timestamp and properties.
- The PostHog token is stored in Hoglet's `control.db`.

Anything captured by Hoglet while forwarding is on is also forwarded. That
includes the importer's batches. If you run an import while forwarding is on,
history is sent back to PostHog with the same uuids, which PostHog may or may
not collapse. Turn forwarding off (`"enabled":false`) while an import runs and
back on after.

### Fill the gap

Events PostHog received between your history import and the SDK switch are not
in Hoglet. Fetch them with a second import, with forwarding off while it runs:

```sh
hoglet import posthog \
  --posthog-project 12345 --posthog-key phx_your_posthog_key \
  --token phc_your_hoglet_project_token \
  --hoglet-host https://analytics.example.com \
  --since 2026-10-01 --skip-persons --checkpoint gap.json
```

Use a `--since` date a day before you started step 1. Duplicates of events
already imported are dropped by uuid. While forwarding is off, live events
that Hoglet captures are not sent to PostHog. They exist in Hoglet only, and
reconcile will show that day as different.

## 3. Reconcile

Compare both systems on the same data. This runs the same aggregation on each
(events and unique people per event name per day) and prints the differences.

```sh
hoglet reconcile posthog \
  --posthog-project 12345 --posthog-key phx_your_posthog_key \
  --hoglet-project <hoglet-project-id> --hoglet-key phx_your_hoglet_key \
  --hoglet-host https://analytics.example.com \
  --days 7 --tolerance 0.5
```

```
day         event                            hoglet    posthog      Δ%  persons H persons P      Δ%
2026-10-01  $identify                             1          1    0.0          1         1    0.0
2026-10-01  $pageview                             1          1    0.0          1         1    0.0
2026-10-04  $merge_dangerously                    1          0  100.0!         1         0  100.0
2026-10-04  $set                                  1          0  100.0!         1         0  100.0

Rows marked ! differ beyond tolerance.
```

`--days` is up to 90 (default 7, counting today). `--tolerance` is a percent
(default 0.5). The exit status is `1` if any row exceeds the tolerance, `0`
otherwise, so you can script it. A Hoglet read key is enough here.

How to read it:

- `$merge_dangerously` and `$set` rows on the day you ran the import are the
  importer's synthetic identity events. They exist only in Hoglet. Ignore
  them. They leave the window as days pass.
- Days are UTC in Hoglet. If your PostHog project uses another timezone,
  expect small differences at day boundaries.
- Unique-people counts can differ slightly where PostHog and Hoglet resolve
  merges differently. Look at the event counts first.
- A recent day can differ because of events still in flight, forwarding
  drops, or the pause for a gap import. Check `forwarded`, `dropped` and
  `failed` from the forwarding status.

Do not cut over until several full days match.

## 4. Cut over

1. Reconcile is clean for the days you care about.
2. Turn forwarding off:

   ```sh
   curl -s -X PUT https://analytics.example.com/api/projects/<hoglet-project-id>/forwarding \
     -H 'Authorization: Bearer phx_your_hoglet_key' -H 'content-type: application/json' \
     -d '{"enabled":false,"host":"https://us.i.posthog.com","posthog_token":"phc_your_posthog_project_token"}'
   ```

3. Remove the PostHog host from your configuration. Leave the PostHog project
   untouched for a while as a read-only archive.

## Rollback

- **During shadow mode:** set `api_host` back to PostHog. PostHog has every
  event, so nothing is lost. Hoglet keeps what it captured.
- **After cut-over:** switch `api_host` back. Events captured only by Hoglet
  since forwarding went off are not in PostHog, and there is no command to
  send them back. They stay in Hoglet's Parquet files (see
  [Data and backup](data-and-backup.md)). To keep this option open longer,
  leave forwarding on for an extra week before turning it off.

## What is not imported

Only events, people and feature flags move. Not imported:

- Dashboards, saved insights, subscriptions and annotations. Rebuild what you
  need in Hoglet's Insights and Dashboards.
- Cohorts. Hoglet has no cohorts yet. A flag with a cohort condition is
  skipped, and the importer prints `flag <key>: skipped (HTTP 400: ...)`.
  The same happens to flags with group conditions and to any condition type
  or operator Hoglet does not support. Re-create them with person property
  conditions.
- Group analytics. Group data is accepted and stored in event properties, but
  group properties are not imported, and Hoglet has no group-based flags. The
  importer does not carry PostHog's group aggregation setting, so a flag that
  was group-based but has no group conditions would arrive as a person flag.
  Check the flag list.
- Session replay, surveys, experiments, heatmaps, data warehouse, CDP
  destinations and transformations, actions.
- Event columns other than `uuid`, `event`, `distinct_id`, `timestamp` and
  `properties` (for example `elements_chain` of autocaptured clicks).
- People with no distinct ids, and any person history other than their
  current properties.
- GeoIP. Imported events keep the `$geoip_*` properties PostHog added, but
  Hoglet does not add them to new events, so country breakdowns thin out after
  cut-over unless your SDK sends `$geoip_country_code`.

After the import, open the flag list and compare it with PostHog's: the flag
count, active states, rollout percentages and variants.
