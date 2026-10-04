# API

Hoglet has two surfaces:

- **SDK wire endpoints**: PostHog's protocol. Used by the PostHog SDKs. Authenticated by the project token.
- **Dashboard API**: Hoglet's own JSON API, under `/api/...`. It is what the dashboard uses, and you can use it too.

Your running instance documents itself: `/docs` is an interactive reference and
`/openapi.json` is the OpenAPI 3.1 document, generated from the same code. Use
them for exact request and response shapes; this page covers the rules that
apply everywhere.

## Authentication

### Project token (`phc_...`)

Identifies a project for the wire endpoints. It goes in the request body
(`api_key` or `token`) or in the URL for `/array/{token}/config`. It can only
send events and read flags for that project. It is safe in browser code. A
personal API key sent where a token belongs is rejected with `401`.

### Session cookie

`POST /api/auth/login` with `{"email","password"}` sets the `hoglet_sid` cookie
(HttpOnly, SameSite=Lax, 7 days). `POST /api/auth/setup` does the same for
first-run setup and works once.

```sh
curl -s -c jar -X POST https://analytics.example.com/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"you@example.com","password":"your passphrase"}'
curl -s -b jar https://analytics.example.com/api/auth/me
```

`/api/auth/me` returns your organizations and projects, with project ids and
tokens.

### Personal API key (`phx_...`)

For scripts and server SDKs. Send it as `Authorization: Bearer phx_...`.
Create one in Settings, API keys, or with a session:

```sh
curl -s -b jar -X POST https://analytics.example.com/api/auth/keys \
  -H 'content-type: application/json' -d '{"name":"ci","scope":"write"}'
# {"key":{...},"secret":"phx_..."}
```

The secret is shown once; Hoglet stores only a hash. Revoke with
`DELETE /api/auth/keys/{key_id}`. A key acts as its user.

### Scopes and roles

| Action | Needs |
|---|---|
| Read project data (queries, persons, events, flags, status, insights, dashboards) | A session or any personal key, and membership in the project's organization. |
| Change project data or settings (flags, insights, dashboards, shares, forwarding, erase a person, demo data) | A session or a `write`-scoped key, **and** the `owner` or `admin` role. |
| Account management (keys, organizations, projects, logout) | A session. Keys cannot do this (`403`). |
| Local flag evaluation (`/flags/definitions`) | A personal key (`read` is enough) whose user can access the project named by `token=phc_...`. |

A key's scope is `read` (default) or `write`. A project you cannot access, or
that does not exist, gets `403`. A project id that is not a UUID gets `404`.

Hoglet currently has one account created at setup, who owns the organization.
There is no invite or user-management endpoint yet.

## Endpoints at a glance

Wire (no auth beyond the token): `POST /e`, `/i/v0/e`, `/capture`, `/track`,
`/engage`, `/batch`; `GET /array/{token}/config` and `/config.js`;
`POST /flags` and `/decide`; `GET /static/surveys.js` (a no-op). Trailing
slashes are optional. These answer CORS requests from any origin, which the
browser SDK needs.

Local evaluation: `GET /flags/definitions` and `GET /api/feature_flag/local_evaluation`.

Operations (no auth): `GET /health`, `GET /ready`, `GET /metrics`, `GET /docs`,
`GET /openapi.json`.

Dashboard API, session cookie or personal key. `{p}` is the project id:

| Area | Endpoints |
|---|---|
| Account | `GET /api/auth/bootstrap` (is setup needed), `POST /api/auth/setup`, `login`, `logout`, `GET /api/auth/me`, `GET`/`POST /api/auth/keys`, `DELETE /api/auth/keys/{id}` |
| Organizations and projects | `GET`/`POST /api/organizations`, `POST /api/organizations/{id}/projects` |
| Queries | `POST /api/projects/{p}/query`, `POST /api/projects/{p}/query/actors` ([Insights and queries](insights-and-queries.md)) |
| Web analytics | `GET /api/projects/{p}/web/overview`, `GET /api/projects/{p}/web/breakdown` |
| Persons and events | `GET /persons`, `GET /persons/{id}`, `GET /persons/{id}/events`, `POST /persons/{id}/erase`, `GET /events` (all under `/api/projects/{p}`) |
| Catalog | `GET /catalog/events`, `/catalog/properties`, `/catalog/values` |
| Flags | `GET`/`POST /feature_flags`, `GET`/`PATCH`/`DELETE /feature_flags/{id}`, `GET /feature_flags/{id}/evaluate` ([Feature flags](feature-flags.md)) |
| Saved work | `GET`/`POST /insights`, `GET`/`PUT`/`DELETE /insights/{id}`; `GET`/`POST /dashboards`, `GET`/`PUT`/`DELETE /dashboards/{id}`, `PUT /dashboards/{id}/tiles`; `GET`/`POST /shares`, `DELETE /shares/{id}` |
| Shadow mode | `GET`/`PUT /forwarding` ([Migrate from PostHog](migrate-from-posthog.md)) |
| Status and demo | `GET /status`, `POST /demo` |
| Public share links | `GET /api/shares/{token}` and `/shared/{token}`: no auth, read-only view of one shared insight or dashboard |

Paging: `GET /persons` takes `search`, `limit` and `cursor` (pass back
`next_cursor`). `GET /events` takes `event`, `person_id`, `limit` (default 100,
at most 200) and `before` (pass back `next_before`).

The dashboard API does not send CORS headers, so browsers can call it from the
dashboard's own origin only. Scripts and servers are not affected.

## Errors

Dashboard API errors are JSON with a matching HTTP status:

```json
{"error": {"code": "forbidden", "message": "You do not have access to this resource.", "request_id": "01a1..."}}
```

`field` is added when one input is at fault. The same id is in the
`x-request-id` response header; quote it when reporting a problem. Some routes
(status, forwarding, erase, demo) also accept your own `x-request-id`, up to 64
characters.

| Status | `code` | Meaning |
|---|---|---|
| 400 | `invalid_request`, `invalid_query`, `invalid_flag`, `query_too_large` | The input is wrong. Fix it; do not retry as is. |
| 401 | `unauthorized` | No or bad credentials. |
| 403 | `forbidden` | Authenticated but not allowed (role, scope, or project). |
| 404 | `not_found` | No such project, person, flag or resource. |
| 409 | `conflict` | Already exists (a flag key), or setup already done. |
| 503 | `unavailable`, `query_busy` | Temporary. Retry. `query_busy` carries `Retry-After: 1`. |
| 504 | `query_timeout` | The query hit its deadline. Narrow the range or the query. |
| 500 | `internal_error` | A bug or storage fault. Check the logs for the `request_id`. |

The wire endpoints keep PostHog's shapes instead. Capture errors have an empty
body. The flags endpoints answer with
`{"type","code","detail","attr"}` (for example `invalid_api_key`).

## Response codes SDKs depend on

PostHog SDKs retry `5xx` and network errors with backoff, and never retry
`4xx`. Hoglet keeps to that: **4xx for anything the client should not resend,
5xx only when a retry can succeed.**

Capture endpoints:

| Status | When | SDK retries? |
|---|---|---|
| `200` `{"status":1}` | Accepted and durably stored. | n/a |
| `204` | Accepted, when the request has `beacon=1`. | n/a |
| `400` | Undecodable or malformed body: invalid JSON, missing `event` or `distinct_id`, bad `timestamp` or `uuid`, empty body. | No |
| `401` | Missing, malformed or unknown project token, or a `phx_` key used as a token. | No |
| `413` | Body (or decompressed body) over the limit. | No |
| `429` | Over `HOGLET_MAX_EVENTS_PER_SEC` for that token. | Backs off |
| `503` | The write-ahead log cannot accept the events right now (queue full, or publication more than 1 GiB behind). | Yes |

An event is only acknowledged with `200` or `204` after it has been written to
the write-ahead log and fsynced. A batch that is empty after filtering (for
example only `$performance_event`, which Hoglet drops) still returns `200`, so
clients never retry it.

## Rate limits

- **Capture:** `HOGLET_MAX_EVENTS_PER_SEC` per project token, default 10,000
  events per second, counted in a fixed one-second window. A batch of N events
  uses N. Over the limit the whole request gets `429`. This is a guard against
  runaway clients, not a quota.
- **Queries:** two run at a time, sixteen wait; beyond that `503 query_busy`.
- There is no limit on dashboard API calls otherwise, and no login throttling
  in this version.

Limits that are not configurable are listed in
[Configuration](configuration.md#fixed-limits).
