# Deploying Hoglet

One static binary, one data directory. No other services.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/debpalash/hoglet/main/scripts/install.sh | sudo sh -s -- --systemd
```

`--systemd` creates a `hoglet` system user, installs `systemd/hoglet.service`
and starts it on `127.0.0.1:8000` with data in `/var/lib/hoglet`. Without it
the binary is installed and nothing else is touched. While the repo is
private, set `GITHUB_TOKEN` (or `HOGLET_DOWNLOAD_URL` for a mirror).

Docker instead:

```sh
docker run -d --name hoglet -p 8000:8000 -v hoglet-data:/data ghcr.io/debpalash/hoglet:<version>
```

Configuration is environment only:

| var | default | |
|---|---|---|
| `HOGLET_ADDR` | `127.0.0.1:8000` (image: `0.0.0.0:8000`) | listen address |
| `HOGLET_DATA` | `./hoglet-data` (unit: `/var/lib/hoglet`, image: `/data`) | data directory |
| `HOGLET_MAX_EVENTS_PER_SEC` | `10000` | per-project capture rate limit |
| `HOGLET_RETENTION_DAYS` | keep all | delete events older than N days |
| `HOGLET_SETUP_TOKEN` | unset | if set, creating the first account needs it (`X-Hoglet-Setup-Token` header or `setup_token` body field) |
| `HOGLET_TRUST_PROXY` | unset | `1` behind the proxies below: login throttling keys on the last `X-Forwarded-For` entry |
| `HOGLET_SECURE_COOKIES` | unset | `1` forces the `Secure` cookie flag (it is also set when `X-Forwarded-Proto: https`) |
| `HOGLET_METRICS_TOKEN` | unset | if set, `/metrics` needs `Authorization: Bearer <token>` |
| `HOGLET_ALLOW_PRIVATE_FORWARDING` | unset | `1` lets shadow-mode forwarding reach private and loopback addresses |
| `RUST_LOG` | `hoglet=info` | log filter |

Health: `GET /health` is liveness; `GET /ready` is 200 while serving and 503
during shutdown. The port opens only after WAL recovery finishes, so during a
slow start connections are refused. Point load balancers at `/ready`.

SIGTERM and SIGINT both drain in-flight requests, fsync and publish the WAL,
then exit. A hard kill is also safe: acknowledged events are in the WAL and
the next start publishes them before reporting ready.

## TLS and the client IP

Hoglet speaks plain HTTP on loopback; terminate TLS in front of it:

- `caddy/Caddyfile` — automatic certificates.
- `nginx/hoglet.conf` — certbot-managed certificates.

Both send the client address in **`X-Forwarded-For`**, overwritten (never
appended) with the TCP peer address, plus `X-Real-IP` with the same value.
`X-Forwarded-For` is the header Hoglet should trust, and only when it listens
on loopback behind one of these proxies — anywhere else it is client-forgeable.
Note: as of this release Hoglet does not yet derive `$ip` server-side; events
carry whatever `$ip` the SDK sends.

## Sizing (1 vCPU / 1 GB)

The unit sets `MemoryHigh=640M` / `MemoryMax=768M`, leaving room for the OS
and the proxy. DuckDB is memory-capped inside
the process; ingest is never starved by queries.

## Data directory and backups

```
/var/lib/hoglet/
  control.db (+ -wal, -shm)       users, orgs, projects, keys, flags (SQLite)
  projections.db (+ -wal, -shm)   lake catalog: which Parquet files are live (SQLite)
  events/                         Parquet segments, immutable once published
  wal/                         capture write-ahead log (acked, not yet published)
```

`control.db` and `projections.db` are a matched pair (shared `pair_id`); always
back them up together and restore them together with `events/` and `wal/`.

What is safe while running:

- **Stop, copy, start** — always correct: `systemctl stop hoglet && tar -C /var/lib -czf hoglet-$(date +%F).tgz hoglet && systemctl start hoglet`.
  Downtime is seconds; SDKs retry on network errors.
- **Filesystem or VPS snapshot** (LVM/ZFS/btrfs, provider snapshot) while
  running — safe. A snapshot is crash-consistent, and Hoglet's startup path is
  crash recovery: SQLite replays its journal, the WAL replays from the last
  published offset.
- **Plain `cp`/`rsync` of the live directory** — NOT safe. The SQLite files
  are in WAL mode and the copy can be torn across `db`, `-wal` and the Parquet
  catalog. Do not do this.

Restore: stop Hoglet, replace the whole directory, `chown -R hoglet:hoglet`,
start. Never mix files from different backups.

## Upgrades

```sh
curl -fsSL .../install.sh | sudo sh -s -- --systemd --version <new>
```

The installer swaps the binary atomically and restarts the unit. By hand:
`install -m 0755 hoglet /usr/local/bin/hoglet && systemctl restart hoglet`.
Docker: pull the new tag and recreate the container with the same volume.

- Take a backup first (above). Downgrades are not supported once a newer
  version has written to the data directory.
- Hoglet refuses to start on a data directory it cannot validate (unknown
  schema, mismatched db pair, incomplete migration) rather than guessing; the
  previous binary keeps working on the untouched backup.
- Expect a few seconds of capture downtime during the restart; posthog-js and
  the server SDKs retry, so no events are lost.
