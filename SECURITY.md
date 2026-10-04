# Security

## Reporting a vulnerability

Email **security@REPLACE-ME.example** (placeholder: set a real contact before
launch). Include the version (`hoglet --version`), what you did and what you
saw. Do not open a public issue for an unfixed vulnerability. There is no bug
bounty.

## Supported versions

Only the latest release receives fixes. Hoglet is pre-1.0; upgrade in place
(the data directory is forward-compatible).

## Scope

In scope: the `hoglet` binary and its embedded dashboard, the installer
(`scripts/install.sh`), and the deployment files in `deploy/`.

Threat model: an internet-exposed server run by a small team. Attackers are
anonymous clients (the capture, flags and config endpoints are public by
design), signed-in members and read-only API keys, malicious SDK payloads, a
malicious web page that targets the dashboard, and anyone who obtains a share
link. Out of scope: an attacker with shell access or the data directory on the
server, a compromised reverse proxy, and denial of service by raw bandwidth.

## Hardening in place

Accounts and sessions
- Passwords: Argon2id (19 MiB, 2 passes, 1 lane, per-password salt). Hashing
  runs on the blocking pool, two at a time, never on the thread that
  authorizes capture. Unknown and known emails cost the same to reject.
- Login throttling: after 5 failures an email is delayed 1 s doubling to 60 s;
  a source address that fails 30 times in 10 minutes is refused for the rest of
  the window. Tables are bounded.
- Sessions: 256-bit random id from the OS generator, 7-day absolute expiry,
  server side, at most 50 per user, `HttpOnly; SameSite=Lax; Path=/`, plus
  `Secure` when the proxy sends `X-Forwarded-Proto: https` or
  `HOGLET_SECURE_COOKIES=1`.
- Personal API keys: stored as SHA-256 only, shown once, `read` or `write`
  scope. Keys cannot create keys, organizations or projects.
- First-run setup closes after the first account. `HOGLET_SETUP_TOKEN`
  optionally gates it.

Authorization
- Every `/api/projects/{id}/...` handler authenticates, then authorizes the
  caller's membership of that project before reading a body or an object id.
  Objects (insights, dashboards, shares, flags) are always looked up by
  `(project_id, id)`.
- Mutations need an owner/admin role and a session or a `write` key. A
  `member` is read-only everywhere, including through a `write` key.
- People are managed by session only (never by a key), per organization:
  admins cannot create, change or remove owners, and an organization always
  keeps one owner. Invite tokens are 256 random bits, stored as SHA-256 and
  looked up by that hash, single use, 7-day expiry, at most 100 pending per
  organization. Accepting one is throttled like sign-in (per source address and
  per invited email), and failed guesses count against the source address.
  Role changes, invites and removals are logged (`hoglet::audit`) with the
  acting user id; tokens are never logged. Removing someone ends their
  sessions and, when they have no organization left, their personal keys.
- Cookie-authenticated state-changing requests are refused when the browser
  marks them cross-site (`Sec-Fetch-Site`, or `Origin` not equal to the host).
  Bearer-key requests are unaffected.
- A project token is a write-only credential for capture and flag evaluation;
  it is not accepted by any dashboard route. `/flags` and `/decide` evaluate
  only the flags of the token's own project.

Public edge
- Body limits: 2 MiB (`/e` family), 20 MiB (`/batch`), 1 MiB (`/flags`).
  Decoded size is capped (16 MiB, 32 MiB for `/batch`, 1 MiB for flags) with
  streaming gzip and bounded lz64. Parsing reserves memory from a shared
  512 MiB budget and at most two expanding decodes run at once; over budget
  answers 503 (retryable).
- JSON nesting is bounded, a batch authorizes every distinct token before any
  write, and `historical_migration` cannot be set from the wire.
- Per-token rate limit (`HOGLET_MAX_EVENTS_PER_SEC`), keyed on the authorized
  token, not on a header.
- CORS is permissive only on the SDK wire routes. Dashboard and API routes send
  no CORS headers.
- The listener enforces a 10 s header read timeout, a 120 s request timeout and
  a 4096 connection cap.
- No panics from request bytes: property tests cover the decoder, the capture
  parser, `/e`, `/batch`, `/flags`, `/decide` and flag regexes.

Dashboard
- `Content-Security-Policy` (scripts from this origin plus the hash of the one
  inline theme script, no framing, connect only to self), `X-Frame-Options:
  DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
  `Cache-Control: no-store` on `/api`, HSTS when the proxy reports TLS. Share
  pages are covered too and cannot be embedded.
- The UI renders event data through React text nodes only: no
  `dangerouslySetInnerHTML`, no links built from event data.
- Static files are served from an embedded map, never from the filesystem.
- A public share link shows what a tile measures. The API response omits SQL
  text, internal ids and author ids. Tokens carry 122 random bits and can
  expire.
- The API reference page makes no third-party requests.

Data and outbound
- SQL tab: exactly one `SELECT`, a fresh in-memory DuckDB per query,
  `allowed_paths` limited to the project's own Parquet files, external access
  off, configuration locked, memory, thread, row and time limits.
  `read_text`, `glob`, `COPY`, `ATTACH`, `INSTALL`/`LOAD` and file table
  functions are refused.
- Shadow-mode forwarding: `http(s)` only, no credentials in the URL, redirects
  off, and the destination is resolved at connect time with loopback,
  link-local (cloud metadata), private, CGNAT and other internal addresses
  refused. `HOGLET_ALLOW_PRIVATE_FORWARDING=1` lifts this. `import` and
  `reconcile` are operator CLI tools and talk only to hosts you name.
- The data directory is forced to `0700` and the databases to `0600`. Paths
  are built from hex-encoded project ids, never from request text.
- Optional `HOGLET_METRICS_TOKEN` protects `/metrics`.
- Secrets are not logged. Request ids are restricted to `[A-Za-z0-9._-]`.
- `npm audit --omit=dev` for `web/` is clean. CI rejects Kafka, ClickHouse,
  Redis and ZooKeeper in the dependency tree and source.

## Known limitations

- Whoever reaches the port first on a fresh install creates the owner account.
  Finish setup before exposing the port, or set `HOGLET_SETUP_TOKEN` (the
  dashboard form has no token field; use `curl`). Startup warns when this is
  possible.
- A project token is public (it ships in web pages). Anyone with it can send
  events and evaluate flags for that project, up to the per-token rate limit;
  one abusive client can use that budget. A `distinct_id` is the only
  credential for flag evaluation, as in PostHog. There is no per-IP limit on
  the wire routes: use the reverse proxy's.
- `$ip` comes from `X-Forwarded-For`. It is only trustworthy behind a proxy
  that overwrites the header (the Caddy and nginx examples do).
- Login throttling can delay the real owner for up to a minute while someone
  guesses at their email; it cannot lock the account permanently.
- Raw request bodies (up to 20 MiB) are buffered per connection before
  limits that depend on content apply; cap concurrent uploads at the proxy.
- The SQL tab is one `SELECT` with limits, not a hardened multi-tenant
  sandbox: any member who can query a project can read all of that project's
  events. A result cell is not size-capped yet (open finding, test marked
  `ignore` in `tests/security_edge.rs`).
- Accounts have no second factor and no emailed password reset. A locked-out
  owner is recovered on the server with `hoglet user reset-password`, which
  needs shell access and a stopped server.
- An invite link is a bearer credential for one seat until it is used: anyone
  who reads it before the invitee can join with the invited role. It is shown
  once, single use and expires in 7 days; the inviter must send it over a
  channel only the invitee can read. The page URL (`/invite/{token}`) can reach
  reverse-proxy access logs; the API calls carry the token in the body only.
- Whoever holds a valid link learns whether the invited email already has an
  account (the page asks for a password instead of a name). Nothing is revealed
  without the link.
- The installer verifies a SHA-256 published beside the tarball, which detects
  corruption but not a compromised release host; releases are not signed yet.
- The bundled SQLite is 3.46.0 (via `rusqlite` 0.32). Hoglet only runs its own
  parameterized statements against it, so SQLite issues that need attacker
  SQL are not reachable.
- `cargo audit` (RustSec, 2026-10-05) is clean for everything in the build
  graph after updating `rustls` to 0.23.45. `Cargo.lock` still lists `rkyv`
  0.7.46 (RUSTSEC-2026-0235), which is not compiled into Hoglet. CI now runs
  `cargo audit` and `npm audit` nightly and on every push.
