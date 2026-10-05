# Deploy

One static binary, one data directory, no other services. This page covers a
small Linux VPS. Reference files live in the repository's `deploy/` directory
(and, after a `--systemd` install, in `/usr/local/share/hoglet/deploy/`).

## Sizing

The target machine is **1 vCPU, 1 GB RAM**.

What the repository records about that target (`ROADMAP.md`, `claims.md`):

- Emulated, not yet measured on a real VPS: under a cgroup cap of 1 CPU and
  1 GB, sustained ingest ran at about 18,000 events/s with a 343 MB peak RSS.
- Measured on a development machine, not the target: peak RSS about 146 MB
  across 200,000 events at 64 concurrent connections with a query racing the
  ingest. Throughput depends on concurrency because every acknowledgement
  waits for a group fsync.
- A measurement on a real 1 vCPU / 1 GB server is still open work.

Memory limits inside the process: DuckDB work is capped per use, not by one
shared budget (a single shared budget is on the roadmap). Dashboard queries
have a 512 MB pool and a separate 256 MB cap per SQL-tab query, the persons,
events and web analytics screens have a 256 MB pool, and background
compaction has 192 MB. Queries run at most two at a time per pool, queue
briefly, and answer `503` when busy. They run on their own threads and cannot
block ingest. The systemd unit adds a hard stop (below).

Disk: see [Data and backup](data-and-backup.md#disk-usage). Put the data
directory on a disk with room to grow; there is no separate database server.

## systemd (recommended on a VPS)

```sh
curl -fsSL https://raw.githubusercontent.com/debpalash/hoglet/main/scripts/install.sh | sudo sh -s -- --systemd
```

(While the repository is private, see the notes in the
[Quickstart](quickstart.md#1-get-the-binary).)

With `--systemd` the installer creates a `hoglet` system user, installs
`/etc/systemd/system/hoglet.service`, copies the proxy examples to
`/usr/local/share/hoglet/deploy/`, and starts the service. It listens on
`127.0.0.1:8000` with data in `/var/lib/hoglet`. Without `--systemd` only the
binary is installed.

The unit (`deploy/systemd/hoglet.service`) sets:

- `MemoryHigh=640M` and `MemoryMax=768M`: leaves room for the OS and the
  proxy on a 1 GB machine. If the process hits the hard limit systemd stops
  it and restarts it (`Restart=on-failure`); events already acknowledged are
  recovered from the write-ahead log.
- `TimeoutStopSec=30`, `LimitNOFILE=65536`, and a sandbox (no new privileges,
  read-only system, private `/tmp`, writes only to `/var/lib/hoglet`).

Day to day:

```sh
systemctl status hoglet
journalctl -u hoglet -f
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8000/ready
```

Change settings with a drop-in, never by editing the unit file:

```sh
sudo systemctl edit hoglet
```

```ini
[Service]
Environment=HOGLET_RETENTION_DAYS=365
Environment=HOGLET_MAX_EVENTS_PER_SEC=5000
```

```sh
sudo systemctl restart hoglet
```

Manual install, without the script:

```sh
sudo install -m 0755 hoglet /usr/local/bin/hoglet
sudo useradd --system --home-dir /var/lib/hoglet --shell /usr/sbin/nologin hoglet
sudo install -m 0644 deploy/systemd/hoglet.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now hoglet
```

## Docker

```sh
docker run -d --name hoglet --restart unless-stopped --stop-timeout 30 \
  -p 127.0.0.1:8000:8000 -v hoglet-data:/data \
  ghcr.io/<owner>/hoglet:<version>
```

`ghcr.io/<owner>/hoglet` is a placeholder; see the note in the
[Quickstart](quickstart.md#1-get-the-binary).

- The image listens on `0.0.0.0:8000` inside the container and keeps data in
  `/data`. It runs as user `65532`, so a bind-mounted data directory must be
  owned by `65532:65532`. A named volume needs nothing.
- Publish the port on `127.0.0.1` and put a TLS proxy in front. Docker's port
  publishing bypasses host firewalls such as `ufw`, so do not publish `8000`
  on all interfaces.
- `--stop-timeout 30` gives Hoglet time to drain and publish the write-ahead
  log on `docker stop` (Docker's default is 10 seconds). A hard stop is still
  safe.
- The image has no shell or `curl`, so it carries a built-in `HEALTHCHECK`
  (`hoglet healthcheck`, which calls `/ready`). From the outside use `/ready`
  (readiness) and `/health` (liveness).
- Memory: add `--memory 768m` to mirror the systemd limits.
- Upgrade: pull the new tag, remove the container, run it again with the same
  volume. Back up first.

## TLS with Caddy or nginx

Hoglet serves plain HTTP on loopback; terminate TLS in front of it. SDKs and
the dashboard should use `https://`. Open ports 80 and 443 and point DNS at the
machine first.

**Caddy** obtains and renews certificates by itself. Use
`deploy/caddy/Caddyfile`, replacing the hostname:

```
analytics.example.com {
	encode zstd gzip
	request_body {
		max_size 20MB
	}
	reverse_proxy 127.0.0.1:8000 {
		header_up X-Real-IP {remote_host}
	}
}
```

**nginx** with certbot: use `deploy/nginx/hoglet.conf`, replace the hostname
and run `certbot --nginx -d analytics.example.com`. Keep
`client_max_body_size 20m` (server SDK batches are up to 20 MiB) and
`proxy_request_buffering off`.

**Keep `/metrics` private.** `/metrics` (Prometheus counters), `/docs` and
`/openapi.json` are served without authentication, and the example proxy
configs forward every path. `/metrics` only has counters (events captured,
acknowledged, rejected, sink errors, uptime), but you probably do not want it
public. Block it at the proxy and scrape `127.0.0.1:8000` directly.

Caddy, inside the site block before `reverse_proxy`:

```
	@metrics path /metrics
	respond @metrics 404
```

nginx, inside the `server` block:

```nginx
    location = /metrics { return 404; }
```

## Client IP header

Hoglet reads the client address from the first value of `X-Forwarded-For`,
falling back to `X-Real-IP`. The shipped Caddy and nginx files **overwrite**
`X-Forwarded-For` with the connection's address (they never append), so a
client cannot forge it.

Only trust the header when Hoglet listens on loopback behind one of these
proxies. If Hoglet is reachable directly, anyone can set it.

Today the address is used for one thing: the daily cookieless device id when
`HOGLET_COOKIELESS_SALT` is set. Hoglet does not store `$ip` and does not do
GeoIP, so events carry only an `$ip` that the SDK sent itself.

## Firewall

Allow SSH, 80 and 443. Keep 8000 closed to the world (it is bound to loopback
by default).

```sh
sudo ufw allow OpenSSH
sudo ufw allow 80,443/tcp
sudo ufw enable
```

## Health and logs

- `GET /health`: `200` while the process is up.
- `GET /ready`: `200` once write-ahead-log recovery is done and the stores are
  open, and `503` as soon as shutdown starts. Hoglet opens its port only after
  recovery, so during start-up connections are refused. Point load balancers
  and uptime checks here.
- `GET /metrics`: Prometheus text, no authentication (see above).
- Logs go to stderr. `RUST_LOG` filters them; default `hoglet=info`.

## Upgrades

1. [Back up](data-and-backup.md#backup).
2. Install the new version:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/debpalash/hoglet/main/scripts/install.sh | sudo sh -s -- --systemd --version <new>
   ```

   The installer swaps the binary atomically and restarts the unit. By hand:
   `sudo install -m 0755 hoglet /usr/local/bin/hoglet && sudo systemctl restart hoglet`.
   Docker: pull the new tag and recreate the container with the same volume.

3. Expect a few seconds with capture unavailable during the restart. SDKs
   retry network errors and 5xx, so events are not lost.

Downgrades are not supported once a newer version has written to the data
directory. Hoglet refuses to start on a data directory it cannot validate
(unknown schema, mismatched database pair, incomplete migration) rather than
guessing; the old binary keeps working on your untouched backup.

## Graceful shutdown

`SIGTERM` (systemd, `docker stop`) and `SIGINT` (Ctrl-C) do the same thing:
`/ready` turns `503`, in-flight requests finish, the write-ahead log is
flushed and published to Parquet, then the process exits. A hard kill is also
safe: every event that was acknowledged with a 2xx is in the log, and the next
start publishes it before reporting ready. Only events that were never
acknowledged can be lost, and SDKs resend those.

## Clock

Keep the server clock correct (`systemd-timesyncd`, `chrony`). Hoglet stamps
events that have no timestamp with its own clock and partitions data by UTC
day.
