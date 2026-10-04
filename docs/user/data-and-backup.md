# Data and backup

Everything Hoglet stores is in one directory (`HOGLET_DATA`, default
`./hoglet-data`; `/var/lib/hoglet` under systemd; `/data` in the Docker image).

## Data directory

```
hoglet-data/
  control.db            (+ -wal, -shm)  accounts and settings (SQLite)
  projections.db        (+ -wal, -shm)  people, identity, catalogs, file list (SQLite)
  events/<project-id>/<YYYY-MM-DD>/*.parquet   your events
  wal/                                  acknowledged events not yet in Parquet
  tmp/                                  scratch space for DuckDB
```

| Path | What it is |
|---|---|
| `control.db` | Users and password hashes, sessions, organizations, projects and their tokens, personal API key hashes, feature flags, saved insights, dashboards, share links, and the shadow-mode forwarding settings (including the PostHog token). |
| `projections.db` | Persons and their properties, `distinct_id` to person mapping (identity merges), groups, the event and property catalog, and the list of live Parquet files with the write-ahead-log position. |
| `events/` | Immutable Parquet files, one directory per project id and UTC day. File names are internal (you will see prefixes `g`, `c` and `e`); do not rename or delete files. |
| `wal/` | The write-ahead log. An event is acknowledged (HTTP 200) after it is in here and fsynced. Within about a second a background publisher turns it into Parquet and applies identity. Segments are removed once published. |
| `tmp/` | Scratch for compaction and for queries that spill to disk. |

`control.db` and `projections.db` are a matched pair (they share a `pair_id`).
Hoglet refuses to start if they do not match. There is no command that rebuilds
`projections.db`, so treat it as data, not as a cache.

## Backup

Back up the whole directory, all at once, from one point in time. Do not pick
files.

### Option 1: stop, copy, start

Always correct. Downtime is a few seconds; SDKs retry network errors and 5xx.

```sh
sudo systemctl stop hoglet
sudo tar -C /var/lib -czf hoglet-$(date +%F).tgz hoglet
sudo systemctl start hoglet
```

Docker (named volume `hoglet-data`; uses the `busybox` image; this variant was
not run for this page):

```sh
docker stop hoglet
docker run --rm -v hoglet-data:/data -v "$PWD":/backup busybox \
  tar -C /data -czf /backup/hoglet-$(date +%F).tgz .
docker start hoglet
```

A clean stop leaves `projections.db-wal` behind; it is part of the data, so
copy the whole directory, including every `-wal` and `-shm` file.

### Option 2: filesystem or VPS snapshot while running

An atomic snapshot of the filesystem that holds the data directory (LVM, ZFS,
btrfs, or your provider's disk snapshot) is safe while Hoglet runs. A
snapshot looks like a crash, and Hoglet's normal startup is crash recovery:
SQLite replays its journal and the write-ahead log is replayed from the last
published position. Copy the snapshot, not the live directory.

### What not to do

A plain `cp` or `rsync` of the live directory is **not safe**. The SQLite files
run in WAL mode, and a copy can capture `.db`, `-wal` and the Parquet file list
at different moments, or miss a file that compaction has just replaced. Ordering
the copy does not fix it. Stop first or snapshot.

### Restore

```sh
sudo systemctl stop hoglet
sudo mv /var/lib/hoglet /var/lib/hoglet.old
sudo tar -C /var/lib -xzf hoglet-2026-10-04.tgz
sudo chown -R hoglet:hoglet /var/lib/hoglet
sudo systemctl start hoglet
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8000/ready
```

Restore the whole directory from one backup. Never mix files from different
backups. This procedure (stop, tar, extract into an empty directory, start) was
run for this page; the restored instance came up ready with its account,
API keys and events intact. Delete `/var/lib/hoglet.old` once you are sure.

Keep backups off the machine, and test a restore now and then. Remember that a
backup made before a [GDPR erasure](#erase-a-person-gdpr) still holds the erased
data.

## Retention

By default Hoglet keeps all events. Set `HOGLET_RETENTION_DAYS` to delete old
ones:

```sh
HOGLET_RETENTION_DAYS=90 hoglet
```

- It works on whole UTC days. A day is removed once it is older than
  `today - N`, so `90` keeps today and the 90 days before it. `0` keeps only
  today.
- The background compactor applies it continuously, including right after
  startup. Expired Parquet files disappear as soon as no running query reads
  them. Empty day directories can remain.
- It is physical deletion of event files. It is not undoable except from a
  backup.
- It applies to events only. Persons, identity, flags, insights and settings
  are kept.
- If you import history from PostHog, make sure the retention window covers it
  (see [Migrate from PostHog](migrate-from-posthog.md)).

## Erase a person (GDPR)

`POST /api/projects/{project_id}/persons/{person_id}/erase` removes a person,
every distinct id that resolves to them, and every stored event of those ids.
Affected Parquet files are rewritten without them and swapped in; this is
physical deletion, not hiding.

It needs an owner or admin session, or a `write`-scoped personal API key. The
`person_id` is the id shown in the Persons list and on the person's page
(Persons, the person, Erase permanently), or `id` in `GET .../persons`.

```sh
curl -s -X POST \
  -H 'Authorization: Bearer phx_your_write_key' \
  https://analytics.example.com/api/projects/<project-id>/persons/user_123/erase
# {"distinct_ids":2,"events":3}
```

A second call returns `404 not_found` because the person is gone. A `503`
means nothing was erased; try again. Notes:

- If the same `distinct_id` sends events again later, Hoglet creates a new
  person.
- Backups taken before the erasure still contain the data, and so does any
  PostHog project you forwarded to (shadow mode).

## Query the Parquet files directly

The files are plain Parquet. Query them with DuckDB, or anything that reads
Parquet. Work on a restored copy or a snapshot, not the live directory: while
Hoglet runs, compaction replaces files, and a long external query can lose a
file it had listed.

Columns (all strings unless noted):

| Column | Meaning |
|---|---|
| `uuid` | Event id. |
| `event` | Event name. |
| `distinct_id` | The id the SDK sent. |
| `timestamp` | UTC, `TIMESTAMPTZ`. |
| `properties` | All event properties as a JSON string. |
| `session_id`, `current_url`, `pathname`, `host`, `referrer`, `referring_domain`, `browser`, `os`, `device_type`, `country`, `utm_source`, `utm_medium`, `utm_campaign`, `lib` | Common properties lifted out of `properties` (`$session_id`, `$current_url`, `$pathname`, `$host`, `$referrer`, `$referring_domain`, `$browser`, `$os`, `$device_type`, `$geoip_country_code`, `utm_*`, `$lib`). Null when the event has none. |

There is no `person_id` column. A person is a set of `distinct_id`s that
`projections.db` merged. Counting `distinct_id`s directly overcounts people
who were merged. To see the merges, read the identity table from a copy:

```sh
sqlite3 -readonly projections.db \
  "SELECT distinct_id, person_id FROM distinct_ids WHERE person_id != distinct_id LIMIT 10"
```

Rows listed there are the ids that were merged into another person. Ids not
listed map to themselves.

Examples (DuckDB, UTC set explicitly so days line up with Hoglet's):

```sql
SET TimeZone = 'UTC';

-- Events per day and name
SELECT date_trunc('day', timestamp) AS day, event, count(*) AS events
FROM read_parquet('hoglet-data/events/<project-id>/*/*.parquet', union_by_name = true)
GROUP BY 1, 2 ORDER BY 1, 2;

-- Top pages in the last 30 days
SELECT pathname, count(*) AS views
FROM read_parquet('hoglet-data/events/<project-id>/*/*.parquet', union_by_name = true)
WHERE event = '$pageview' AND timestamp >= now() - INTERVAL 30 DAY
GROUP BY 1 ORDER BY views DESC LIMIT 10;

-- Any property, from the JSON
SELECT json_extract_string(properties, '$.plan') AS plan, count(*) AS n
FROM read_parquet('hoglet-data/events/<project-id>/*/*.parquet', union_by_name = true)
GROUP BY 1 ORDER BY n DESC;
```

Always pass `union_by_name = true`: the schema only ever grows, so older files
can lack newer columns. The project id is in Settings, Project.

These queries ran against DuckDB 1.5 on files written by this version.
Hoglet's own SQL tab is easier for ad-hoc work and resolves people through
identity for you; see [Insights and queries](insights-and-queries.md#sql-tab).

## Disk usage

Events are stored compressed (Parquet, zstd). Size depends on your properties.
One data point: Hoglet's demo dataset (33,843 generated events over 90 days,
mostly pageviews and a few product events) stored as 2.4 MB of Parquet, about
70 bytes per event. Real traffic with long URLs, user-agent strings and
autocapture properties will be larger, possibly several times larger.

Measure your own: after a day of real traffic, divide `stored_bytes` by
`stored_events` from `GET /api/projects/<project-id>/status` (also shown in the
dashboard's freshness popover), then multiply by your expected volume.

Also budget for:

- `wal/`: unpublished events. Normally tiny. If publication falls far behind,
  capture answers `503` once 1 GiB is waiting, rather than risk the disk.
- `tmp/`: queries that exceed their memory cap spill here. The persons, events
  and web analytics screens allow up to 4 GB of spill.
- Backups, on a different disk.
- SQLite files: small next to the events.
