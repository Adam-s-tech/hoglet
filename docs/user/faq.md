# FAQ

## What is Hoglet?

A PostHog-compatible product analytics backend in one Rust binary. You point
PostHog's own SDKs at it by changing `api_host`. It stores events as Parquet
files, keeps accounts, people and flags in SQLite, and ships its dashboard
inside the binary. It needs no ClickHouse, Kafka, Redis, Postgres or
Zookeeper.

## Which SDKs work?

Contract tests run posthog-js (in a real browser), posthog-node and
posthog-python at their latest versions against a real Hoglet. Any other
PostHog SDK that speaks the same capture protocol should work, but only these
three are tested. Mobile and other native SDKs are out of scope for now.

## What is not supported?

Session replay, surveys, experiments, data warehouse, CDP, heatmaps, error
tracking and cohorts are not implemented. For posthog-js, Hoglet's remote config
declares recording, surveys, heatmaps, performance capture and exception
autocapture off, so the SDK does not try to use them. AI events
(`$ai_generation` and friends) and `$exception` events are accepted and stored
like any other event.

## Does my data leave my server?

Not unless you ask. Hoglet's server makes outbound requests in three cases,
all of which you start: shadow-mode forwarding to PostHog, `hoglet import
posthog`, and `hoglet reconcile posthog`. The dashboard and the API reference are
served from inside the binary with no CDN. The one exception is the optional
HTML snippet of posthog-js, which loads `array.js` from PostHog's CDN unless
you host that file yourself (see [SDK setup](sdk-setup.md#posthog-js)).

## Is there an upgrade path, and can I leave?

Your events are plain Parquet files, readable by DuckDB, Python, Spark and
others (see [Data and backup](data-and-backup.md#query-the-parquet-files-directly)).
Upgrades are in place (see [Deploy](deploy.md#upgrades)). Downgrades are not
supported once a newer version has written to the data directory, so back up
first.

## Can I run it highly available or on several nodes?

No. It is one process on one machine by design. Durability comes from the
local write-ahead log and your backups. Multi-node replication is not planned.

## Is the data encrypted at rest?

Hoglet does not encrypt files itself. Use disk or volume encryption. TLS is
terminated by the proxy in front of it. Passwords are stored as hashes and
personal API keys as hashes; the PostHog token for shadow mode is stored in
`control.db` in plain text, so protect the data directory.

## How many users and projects can I have?

Any number of organizations and projects, each with its own token and fully
separate events, people and flags. There is one login: the account created at
first run. There are no invitations or user roles to hand out yet, and no
password-reset flow in this version, so keep the passphrase safe. You can
create more personal API keys from the dashboard.

## Why do my numbers differ from PostHog's?

Common reasons: Hoglet uses UTC for days, weeks (Monday start) and months; person
counts follow Hoglet's merge rules and are recomputed through the current
identity graph; person properties in filters are current values; and the
importer adds synthetic `$set` and `$merge_dangerously` events dated the
import day. [Migrate from PostHog](migrate-from-posthog.md#3-reconcile) explains
how to compare the two systems and read the result.

## Why are countries empty?

Hoglet does not do GeoIP. Country comes from an event property
`$geoip_country_code`. PostHog adds that on its servers, so events imported
from PostHog have it, but new events from stock SDKs do not. Send the property
yourself if you need it (for example from your server SDK).

## What does cookieless mode do?

With `HOGLET_COOKIELESS_SALT` set, events that arrive with no `$device_id` get
one derived from the client IP, your secret salt and the UTC date. It rotates
daily, so it cannot follow a visitor across days. It needs the client IP, so
the proxy must send `X-Forwarded-For` (see
[Deploy](deploy.md#client-ip-header)). Events from SDKs that already send a
`$device_id` are untouched.

## Does Hoglet store IP addresses?

It does not add `$ip` to events. The client IP is used only for the cookieless
device id, and only in memory. An `$ip` property that your SDK sends itself is
stored like any other property.

## Can duplicates get in?

SDKs retry, so the same event can arrive twice. Events carry a `uuid`; Hoglet
removes repeated uuids when it compacts a day's files, which happens for past
days as soon as they have more than one file and for today once enough small
files pile up. A figure for today can therefore include a retry briefly. It
cannot lose an acknowledged event.

## What does "acknowledged" mean?

A `200` (or `204`) means the event is in the write-ahead log on disk. A `kill -9`
or power cut at any point loses nothing that was acknowledged. Events that were
not acknowledged (the request failed or timed out) can be lost, and the SDK
resends them.

## What are the limits I should know about?

See [Configuration](configuration.md#fixed-limits): request sizes, the
per-project rate limit (default 10,000 events per second), query deadlines and
the SQL tab limits.

## What hardware do I need?

The design target is 1 vCPU and 1 GB of RAM. The repository records an
emulated run under that cap and development-machine numbers; a measurement on a
real VPS is still open. Details and the exact figures are in
[Deploy](deploy.md#sizing).

## How do I report a bug?

Include the `request_id` from the error response or the `x-request-id` header,
the Hoglet version (`hoglet --version`) and the relevant lines of
`journalctl -u hoglet`.

## Which license?

AGPL-3.0.
