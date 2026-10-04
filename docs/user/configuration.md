# Configuration

Hoglet is configured with environment variables only. There is no config file.
`hoglet --help` prints the same list.

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `HOGLET_ADDR` | `127.0.0.1:8000` (Docker image: `0.0.0.0:8000`) | Listen address as `IP:port`. Hostnames are not accepted. |
| `HOGLET_DATA` | `./hoglet-data` (systemd unit: `/var/lib/hoglet`; image: `/data`) | Data directory. Created if missing. |
| `HOGLET_RETENTION_DAYS` | unset: keep everything | Delete events older than N UTC days. See [Retention](data-and-backup.md#retention). |
| `HOGLET_MAX_EVENTS_PER_SEC` | `10000` | Per-project-token capture limit in events per second. A batch of N events counts N. Over the limit: `429`. |
| `HOGLET_COOKIELESS_SALT` | unset: off | Secret salt. When set, events that have no `$device_id` get one derived from the client IP, the salt and the UTC date, so it rotates daily. Needs the client IP header (see [Deploy](deploy.md#client-ip-header)). An empty value is the same as unset. |
| `HOGLET_NO_UA_PARSE` | unset: parsing on | Set to any value to stop parsing the User-Agent. By default Hoglet fills `$browser`, `$browser_version`, `$os`, `$os_version` and `$device_type` from the request's User-Agent when the event does not already have them. Server SDKs send none of these. |
| `HOGLET_DEMO` | unset | Exactly `1`, on a fresh data directory, creates the demo account `demo@hoglet.dev` / `hoglet-demo-1` with 90 days of generated data. Ignored if the directory already has an account. |
| `RUST_LOG` | `hoglet=info` | Log filter (standard `tracing` syntax, for example `hoglet=debug`). |

A number that does not parse (for example `HOGLET_RETENTION_DAYS=x`) stops
startup with a message naming the variable; a bad `HOGLET_ADDR` does the same.

Set them in the shell, in `systemctl edit hoglet`, or with `docker run -e`:

```sh
HOGLET_ADDR=0.0.0.0:8000 HOGLET_DATA=/srv/hoglet HOGLET_RETENTION_DAYS=365 hoglet
```

## Commands

```
hoglet                   start the server (same as `hoglet serve`)
hoglet import posthog    copy a PostHog project's history into Hoglet
hoglet reconcile posthog compare Hoglet's numbers with PostHog's
hoglet --version         print the version (also: -V, version)
hoglet --help            print help (also: -h, help)
```

Any other command exits with status 1 and a hint.

### `hoglet import posthog`

```
hoglet import posthog --posthog-project <id> --posthog-key <phx_...> --token <phc_...> [options]
```

| Flag | Default | Meaning |
|---|---|---|
| `--posthog-project <id>` | required | PostHog project id. |
| `--posthog-key <phx_...>` | required | PostHog personal API key with query read access. |
| `--token <phc_...>` | required | Hoglet project token to import into. |
| `--posthog-host <url>` | `https://us.posthog.com` | PostHog API host (EU: `https://eu.posthog.com`). |
| `--hoglet-host <url>` | `http://localhost:8000` | Hoglet to import into. |
| `--hoglet-key <phx_...>` | none | Hoglet personal API key with `write` scope. With `--hoglet-project`, enables flag import. |
| `--hoglet-project <id>` | none | Hoglet project id, for flags. |
| `--since <date>` | none | Only events on or after this ISO date. |
| `--checkpoint <file>` | `.hoglet-import-<posthog-project>.json` | Progress file; rerunning resumes. |
| `--skip-events` | off | Import people and flags only. |
| `--skip-persons` | off | Import events and flags only. |

Run `hoglet import posthog --help` to print this. Procedure and caveats:
[Migrate from PostHog](migrate-from-posthog.md).

### `hoglet reconcile posthog`

```
hoglet reconcile posthog --posthog-project <id> --posthog-key <phx_...> \
                         --hoglet-project <id> --hoglet-key <phx_...> [options]
```

| Flag | Default | Meaning |
|---|---|---|
| `--posthog-project <id>` | required | PostHog project id. |
| `--posthog-key <phx_...>` | required | PostHog personal API key. |
| `--hoglet-project <id>` | required | Hoglet project id. |
| `--hoglet-key <phx_...>` | required | Hoglet personal API key (read scope is enough). |
| `--posthog-host <url>` | `https://us.posthog.com` | PostHog API host. |
| `--hoglet-host <url>` | `http://localhost:8000` | Hoglet host. |
| `--days <n>` | `7` | Days to compare, 1 to 90, counting today. |
| `--tolerance <percent>` | `0.5` | Differences at or below this are reported as matching. |

Exit status `1` when any row differs beyond the tolerance, `0` when all match.

## Install script

`scripts/install.sh` (see [Quickstart](quickstart.md) and [Deploy](deploy.md)).

| Option | Meaning |
|---|---|
| `--version <v>` | Install this version. Default: the latest release. |
| `--prefix <dir>` | Install directory. Default: `/usr/local/bin` if root or writable, else `~/.local/bin`. |
| `--systemd` | Linux, root: create the `hoglet` user, install and start the service on `127.0.0.1:8000`. |
| `-h`, `--help` | Show help. |

| Variable | Meaning |
|---|---|
| `HOGLET_REPO` | `owner/name` on GitHub. Default `debpalash/hoglet`. |
| `HOGLET_DOWNLOAD_URL` | Base URL serving `<tag>/<asset>`; overrides GitHub (mirror). Requires `--version`. |
| `HOGLET_VERSION` | Same as `--version`. |
| `GITHUB_TOKEN` | For a private repository's release assets (needs `curl`). |

## Fixed limits

These are built in and not configurable. They are the numbers SDKs and
operators run into.

| Limit | Value |
|---|---|
| Capture body, `/e`, `/i/v0/e`, `/capture`, `/track`, `/engage` | 2 MiB (`413` above) |
| Capture body, `/batch` | 20 MiB |
| Capture body after decompression | 64 MiB |
| `/flags` and `/decide` body | 1 MiB |
| Dashboard API query body | 1 MiB |
| `distinct_id` length | truncated at 200 characters |
| Project token | at most 64 ASCII bytes; must not start with `phx_` |
| Event timestamps in the future | more than 23 hours ahead become "now" |
| Password | 12 to 1024 characters |
| Login session | 7 days, cookie `hoglet_sid` |
| Capture queue | 4096 batches or 128 MiB waiting for the log; above that `503` |
| Unpublished write-ahead log | 1 GiB; above that `503` |
| Insight query | 30 s deadline; 2 run at once, 16 may wait up to 10 s, else `503` with `Retry-After: 1` |
| Persons, events and web analytics queries | 10 s deadline (`504`); 2 at once, 16 waiting, else `503` |
| SQL tab | one `SELECT`, 20,000 bytes, 10,000 rows, 256 MB memory |
| Flags | 2,000 per project, 50 condition groups per flag, 50 variants |

Per-endpoint behavior and status codes: [API](api.md).
