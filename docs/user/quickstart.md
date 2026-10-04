# Quickstart

From nothing to your first event in a few minutes.

## 1. Get the binary

Pick one.

**Install script** (Linux or macOS, amd64 or arm64). It downloads a release
tarball, checks its sha256, and installs `hoglet` to `/usr/local/bin`
(or `~/.local/bin` if you are not root).

```sh
curl -fsSL https://raw.githubusercontent.com/debpalash/hoglet/main/scripts/install.sh | sh
```

> The script needs a published GitHub release. While the repository is
> private, set `GITHUB_TOKEN`, or point `HOGLET_DOWNLOAD_URL` at a mirror and
> pass `--version`. Options: `--version <v>`, `--prefix <dir>`, `--systemd`
> (see [Deploy](deploy.md)).

**Docker.** The image is one static binary on an empty base, running as a
non-root user, with data in `/data`.

```sh
docker run -d --name hoglet -p 8000:8000 -v hoglet-data:/data ghcr.io/<owner>/hoglet:<version>
```

> `ghcr.io/<owner>/hoglet` is a placeholder. The registry name is not final;
> the release workflow publishes to `ghcr.io/<github-owner>/hoglet` with
> `<version>` and `<major>.<minor>` tags. Substitute the real image name.

**Build from source.** You need a Rust toolchain that supports edition 2024
and a C++ compiler. The dashboard is already built and committed, so Node is
not needed. The first build compiles DuckDB from source and is slow.

```sh
cargo build --release
./target/release/hoglet --version
```

## 2. First run

```sh
./hoglet
```

```
  hoglet 0.1.0 ready in <n> ms

  dashboard   http://127.0.0.1:8000
  api_host    http://127.0.0.1:8000   (point any PostHog SDK here)
  data        hoglet-data
```

Hoglet creates `./hoglet-data` (override with `HOGLET_DATA`) and listens on
`127.0.0.1:8000` (override with `HOGLET_ADDR`, an `IP:port`, for example
`0.0.0.0:8000`). Stop it with Ctrl-C; it publishes pending events first.

Check it is up:

```sh
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8000/ready   # 200
```

## 3. Create your account

Open `http://127.0.0.1:8000`. The first visit asks for an email, a password
(12 to 1024 characters), an organization name and a project name. This works
once; after that the page is a login. The account owns the organization.

Hoglet shows your **project token** (`phc_...`) and install snippets for each
SDK on the **Connect your app** page and in Settings, Project. The token only lets clients send events and
read flags. It cannot read data.

Prefer the command line? The same step is an API call:

```sh
curl -s -X POST http://127.0.0.1:8000/api/auth/setup \
  -H 'content-type: application/json' \
  -d '{"email":"you@example.com","password":"a long passphrase","organization_name":"Acme","project_name":"Web"}'
```

The response lists the project id and token. Migrating from PostHog and want
to keep your existing token? Add `"existing_project_token":"phc_..."` to this
call (see [Migrate from PostHog](migrate-from-posthog.md)).

## 4. Connect an app

```js
import posthog from 'posthog-js'

posthog.init('phc_your_project_token', {
  api_host: 'http://127.0.0.1:8000',
})
```

Other SDKs, React/Next.js and first-party proxying are in
[SDK setup](sdk-setup.md). To send one event with no SDK:

```sh
curl -X POST http://127.0.0.1:8000/i/v0/e/ \
  -H 'Content-Type: application/json' \
  -d '{"api_key":"phc_your_project_token","event":"hello_hoglet","distinct_id":"user_123"}'
# {"status":1}
```

A `401` means the token is wrong. A `400` means the body is malformed or has no
`distinct_id`.

## 5. See the first event

Open **Activity** in the dashboard. The event appears after the next
publish; the pipeline seals and publishes about once a second. The data
freshness indicator in the sidebar reads "No events yet", then "Live"; click
it for the ingestion lag, stored events and stored size.

From the command line, with a personal API key (Settings, API keys) and your
project id (Settings, Project):

```sh
curl -s -H 'Authorization: Bearer phx_...' \
  http://127.0.0.1:8000/api/projects/<project-id>/status
# {"has_events":true,"last_event_at":"...","ingestion_lag_seconds":0.0,"stored_events":1,...}
```

`stored_events` counts events that are already queryable. If it stays at 0,
see [Troubleshooting](troubleshooting.md).

## Demo mode

To look around with data before connecting anything:

```sh
HOGLET_DEMO=1 ./hoglet
# login: demo@hoglet.dev / hoglet-demo-1
```

`HOGLET_DEMO=1` only acts on a fresh data directory. It creates the demo
account and 90 days of generated product data ("Notably"). If the directory
already has an account it is ignored. Use a separate `HOGLET_DATA` for demos.

On an existing install, Settings, Project, **Demo data** adds the same
dataset to the current project. That mixes fake events into real ones, so
use a throwaway project for it.

## Next

- [SDK setup](sdk-setup.md)
- [Deploy](deploy.md): TLS, systemd, a real server
- [Migrate from PostHog](migrate-from-posthog.md)
