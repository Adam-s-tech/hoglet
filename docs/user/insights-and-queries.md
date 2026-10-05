# Insights and queries

What each number in Hoglet means. Everything is computed from your stored
events. All times are **UTC**, weeks start on **Monday**, and a date range is
half-open: it includes its start and excludes its end.

## People and identity

Person counts are counts of **people**, not of `distinct_id`s.

- A person starts as the first `distinct_id` Hoglet saw for them. The person id
  is that id.
- `$identify` with `$anon_distinct_id` (what `posthog.identify()` sends) merges
  the anonymous person into the identified one, unless the anonymous person has
  already been identified. Two logged-in users on one browser stay two people.
- `$create_alias` merges two ids unless both are already identified. If one is
  identified, that person survives, whichever side the SDK put the alias on.
- `$merge_dangerously` always merges. You are asserting they are one human.
- The surviving person keeps the earlier creation time and the older flag
  bucketing key (so a flag value seen while anonymous survives login) and gets
  the union of both people's properties; the survivor's value wins conflicts.
- `$set`, `$set_once` and `$unset` update person properties, in the order
  events are published.

Merges apply to history. When two ids are merged, past events of both count for
one person in every insight, so earlier numbers can go down after a merge.

Person properties used in filters and breakdowns are the person's **current**
properties, not the values at the time of the event. Results that use them are
cached for at most 30 seconds.

## Date ranges and intervals

`date_from` and `date_to` accept: `-24h`, `-7d`, `-4w`, `-3m`, `-1y`, `dStart`
(today), `mStart`, `yStart`, `all` (from the first event), an ISO date
(`2026-10-01`) or an ISO datetime. `date_to` empty means now.

Relative dates snap to the interval: with a day interval, `-7d` starts at 00:00
UTC seven days ago and the range ends at the end of today, so it covers eight
calendar days including today. Intervals are `hour`, `day`, `week` (Monday) and
`month`. A range may not produce more than 10,000 buckets.

## Filters and breakdowns

A filter is `{key, type, operator, value}`. `type` is `event` (default) or
`person`. Operators: `exact` and `is_not` (value may be an array: any of /
none of; comparison is case-sensitive), `icontains`, `not_icontains`, `regex`,
`not_regex`, `gt`, `gte`, `lt`, `lte`, `is_set`, `is_not_set`, `is_date_before`,
`is_date_after`.

A breakdown keeps the top N values (default 10, at most 300) by event count in
the range. Everything else folds into `$$_other`. Events without the property
land in `$$_none`.

## Trends

One value per series per interval bucket. A series is an event name (or all
events) with its own filters and a math:

| Math | Computes |
|---|---|
| `total` | Number of events. |
| `dau` | Unique people in each bucket (the name is PostHog's; it applies to any interval). |
| `weekly_active` | Unique people in the 7 days ending at each bucket's end. |
| `monthly_active` | Unique people in the 30 days ending at each bucket's end. |
| `unique_session` | Distinct non-empty `$session_id` values in each bucket. |
| `sum`, `avg`, `min`, `max`, `median`, `p90`, `p95`, `p99` | Over the numeric values of `math_property`. Non-numeric values are ignored; empty buckets are 0; percentiles interpolate linearly. |

`aggregated_value` is the same math over the whole range (for `total`, the sum
of the buckets). Extra options:

- **Formula:** arithmetic over series letters, such as `A / B * 100`.
- **Compare:** also return the previous period of equal length.
- Up to 20 series.

## Funnels

Counts people who do step 1, then step 2, and so on.

- **Window:** every step must happen within the conversion window of that
  attempt's first step, inclusive. Default 14 days; units minute, hour, day,
  week.
- **Order:**
  - `ordered` (default): steps in order, other events allowed in between.
  - `strict`: steps must be consecutive events of that person.
  - `unordered`: steps in any order; depth is how many distinct steps appear in
    the window.
- An event cannot fill two steps.
- **Attempts:** an attempt starts at any step-1 event. Each person counts once,
  with their deepest attempt (ties go to the earliest start).
- **Exclusions:** a person is dropped from an attempt if the excluded event
  happens strictly between the two named steps.
- **Breakdown:** by a property of the first step's event (first touch) or of
  the person.
- **Time to convert:** average and median time between steps, and a histogram
  from first to last step for people who finished.
- Up to 20 steps and 10 exclusions.

## Retention

Cohorts are the last `total_intervals` periods (default 8, at most 100) ending
with the current one. `period` is `day`, `week` or `month`.

- `retention_recurring` (default): a person is in the cohort of **every**
  period in which they did the target event.
- `retention_first_time`: a person is only in the cohort of the period of
  their **first ever** target event.
- Column 0 is the cohort size. Column k counts cohort members who did the
  returning event in the period k after the cohort's. Periods in the future are
  omitted.

## Lifecycle

For each bucket, people who did the event are:

- **new:** their first ever matching event is in this bucket.
- **returning:** active now and in the previous bucket.
- **resurrecting:** active now, not in the previous bucket, and not new.
- **dormant:** active in the previous bucket but not now. Shown as a negative
  count.

## Stickiness

How many people were active in exactly 1, 2, 3, ... intervals of the range. For
a 14-day range with a day interval, the bar at 3 is the people who were active
on exactly 3 of the 14 days.

## Paths

The most common transitions between consecutive steps, per person.

- Nodes are the `$pathname` of `$pageview` events (`pageviews`), the names of
  events that do not start with `$` (`custom_events`), or both (`all`).
- Repeats in a row collapse to one node.
- `start_point` begins each path at its first occurrence of that node (people
  without it are skipped); `end_point` ends it at the first occurrence after
  that (people who never reach it are skipped).
- Paths are cut to `step_limit` nodes (default 5, at most 20) and the strongest
  `edge_limit` links are returned (default 50, at most 1,000). A link counts
  people; its time is the mean seconds between the two nodes.

## Web analytics

The Web analytics screen is built on `$pageview` events.

- **Visitors:** unique people with a pageview.
- **Pageviews:** `$pageview` events.
- **Sessions:** a `$session_id`. Events without one are grouped per person, and
  a new session starts after more than 30 minutes of inactivity. Only sessions
  with at least one pageview count.
- **Bounce rate:** PostHog's definition: sessions with exactly one pageview,
  no `$autocapture` event, and a duration under 10 seconds, divided by all
  sessions. Custom events do not prevent a bounce.
- **Session duration:** the mean of (last event minus first event) per session.
- Each metric shows the change against the previous period of equal length,
  computed independently.
- **Live visitors:** people with a pageview in the last 5 minutes.
- **Entry and exit pages:** a session's first and last pageview. Every other
  breakdown (referrer, UTM, browser, OS, device, country) is attributed from the
  session's first pageview.

Countries need a `$geoip_country_code` property on the event. Hoglet does not
do GeoIP itself, so stock SDK events have no country unless you supply one.

## Actors: the people behind a number

Click a number in a result to see the people behind it. Over the API:
`POST /api/projects/{project_id}/query/actors` with the same `query` plus a
`selection`:

| `selection.type` | Opens |
|---|---|
| `TrendsPoint` (`series_index`, `day`, optional `breakdown_value`) | One point of a trend. |
| `FunnelStep` (`step`, `converted`) | Who reached the step, or who reached the previous step and not this one. |
| `RetentionCell` (`cohort_date`, `interval`) | One cohort cell. |
| `LifecycleCell` (`status`, `day`) | One lifecycle bucket. |
| `StickinessBar` (`series_index`, `intervals`) | One stickiness bar. |

`limit` defaults to 100 (at most 1,000) and `offset` pages through them.

## SQL tab

Insights, then SQL. Run read-only SQL (DuckDB dialect) over one project's
events. Ctrl or Cmd plus Enter runs the query.

The only table is `events`:

| Column | Type | Meaning |
|---|---|---|
| `uuid` | text | Event id. |
| `event` | text | Event name. |
| `distinct_id` | text | The id the SDK sent. |
| `person_id` | text | The person after identity merges. |
| `timestamp` | timestamptz | UTC. |
| `properties` | text (JSON) | All event properties. |
| `session_id`, `current_url`, `pathname`, `host`, `referrer`, `referring_domain`, `browser`, `os`, `device_type`, `country`, `utm_source`, `utm_medium`, `utm_campaign`, `lib` | text | Common properties lifted out of `properties` (`$session_id`, `$current_url`, and so on; `country` is `$geoip_country_code`). Null when missing. |

Rules and limits:

- One `SELECT` (or `WITH ... SELECT`). No other statements, no files, no
  network.
- At most 10,000 rows. If there are more, the result says it was truncated.
- At most 20,000 bytes of SQL, 256 MB of memory per query, and a 30 second
  deadline.
- Properties: `json_extract_string(properties, '$.plan')`, or
  `properties->>'$.plan'`. For a key with a `$` in it, quote it:
  `json_extract_string(properties, '$."$browser"')`.

**Time.** `timestamp` is a UTC `TIMESTAMP` (no time zone) and `now_utc()` is
the current UTC time, so the usual functions work:

```sql
-- events per UTC day
SELECT date_trunc('day', timestamp) AS day, count(*) AS events
FROM events GROUP BY 1 ORDER BY 1;

-- per hour of day
SELECT extract(hour FROM timestamp) AS hour, count(*) FROM events GROUP BY 1 ORDER BY 1;

-- last 7 days
SELECT event, count(*) AS events, count(DISTINCT person_id) AS people
FROM events
WHERE timestamp > now_utc() - INTERVAL 7 DAY
GROUP BY event ORDER BY events DESC LIMIT 50;

-- from a fixed instant
SELECT count(*) FROM events WHERE timestamp >= TIMESTAMP '2026-10-01 00:00:00';
```

Use `now_utc()`, not `now()`: `now()` is a time-zone-aware value and arithmetic
on it needs DuckDB's ICU extension, which the single binary does not ship.

Errors come back as `400 invalid_query` with DuckDB's message; a timeout is
`504`; too many concurrent queries is `503` (retry).

## Result freshness and caching

Results are cached per project and invalidated when new events are published,
so a repeat of the same query is free until data changes. Set `"refresh": true`
in the request to bypass the cache. The sidebar's freshness indicator and
`GET /api/projects/{project_id}/status` show how far behind ingestion is.

Queries are run through a small pool (two at a time, sixteen waiting); a busy
server answers `503` with `Retry-After: 1`.

## Run a query from the command line

```sh
curl -s -X POST https://analytics.example.com/api/projects/<project-id>/query \
  -H 'Authorization: Bearer phx_your_key' -H 'content-type: application/json' \
  -d '{"query":{"kind":"TrendsQuery","series":[{"event":"$pageview","math":"dau"}],"date_range":{"date_from":"-7d"},"interval":"day"}}'
```

The response has `result` (with `kind: "Trends"` and per-series `days`, `data`
and `aggregated_value`) and `meta` (`elapsed_ms`, `cached`, resolved dates,
`timezone: "UTC"`). Query kinds: `TrendsQuery`, `FunnelsQuery`,
`RetentionQuery`, `LifecycleQuery`, `StickinessQuery`, `PathsQuery`,
`SqlQuery`. Full shapes are in the instance's `/docs`.
