# Feature flags

Hoglet implements PostHog's flag model and PostHog's exact bucketing, so a flag
at 30% selects the same people on Hoglet as it would on PostHog. Flags are
evaluated inside the Hoglet process, on their own path, so they keep answering
when dashboard queries are slow.

Create and edit flags in the dashboard (Feature flags), or through the API
below. SDK usage is in [SDK setup](sdk-setup.md#feature-flags).

## The flag model

| Field | Meaning |
|---|---|
| `key` | Unique per project. Letters, digits, `-` and `_`; up to 400 bytes. |
| `name` | Free-text description. |
| `active` | When false the flag is off for everyone and left out of SDK responses. |
| `filters.groups` | Release conditions, evaluated in order. |
| `filters.multivariate` | Optional variants. |
| `filters.payloads` | Optional JSON payloads. |
| `ensure_experience_continuity` | Keep a person's value stable across `identify`. |

### Release conditions

Each condition group has:

- `properties`: person property filters. **All** must match. An empty list
  matches everyone.
- `rollout_percentage`: 0 to 100. Empty means 100.
- `variant`: optional. Forces this variant for people the group matches.

Groups are checked in order and **the first group that matches** decides the
value. A group matches when all its filters match and the person falls inside
its rollout percentage. If no group matches the flag is off.

Property filters read the person's stored properties (from `identify`, `$set`,
`$set_once`), overridden by `person_properties` sent with the request. Operators:
`exact` and `is_not` (value or list of values; case-insensitive),
`icontains`, `not_icontains`, `regex`, `not_regex`, `gt`, `gte`, `lt`, `lte`,
`is_set`, `is_not_set`, `is_date_before`, `is_date_after` (dates like
`2026-01-31` or relative like `-7d`). A property the person does not have
matches nothing, except `is_not_set`. A regular expression that does not
compile never matches, for `regex` and `not_regex` alike.

### Variants

A multivariate flag has variants whose `rollout_percentage` values **sum to
100**, each with a unique key (same character rules as flag keys, up to 50
variants). A person who matches a condition gets one variant: the group's
forced `variant` if it names a real one, otherwise the variant their hash falls
into.

### Payloads

Payload keys are `"true"` for a boolean flag, or a variant key for a
multivariate flag. Values are any JSON up to 64 KiB. On the wire payloads are
sent as JSON strings, as PostHog does, and the SDKs parse them.

```json
{"key": "new-checkout",
 "filters": {
   "groups": [{"properties": [{"key": "plan", "type": "person", "operator": "exact", "value": ["pro"]}],
               "rollout_percentage": 50}],
   "multivariate": {"variants": [{"key": "control", "rollout_percentage": 50},
                                 {"key": "test", "rollout_percentage": 50}]},
   "payloads": {"test": {"color": "green"}}}}
```

## Bucketing and PostHog compatibility

A person's position is a number between 0 and 1:
`sha1("<flag key>.<bucketing id><salt>")`, the first 15 hex digits read as an
integer, divided by `0xFFFFFFFFFFFFFFF`. For the rollout the salt is empty, and
the person is in when the number is at most `rollout_percentage / 100`. For
variants the salt is `variant`, and variants take consecutive ranges in their
listed order. This is PostHog's algorithm and was checked against PostHog's own
consistency vectors, so Hoglet, PostHog and the SDKs' local evaluation agree.

The bucketing id is the `distinct_id`. For a flag with experience continuity it
is the person's first-seen key, which survives merges. A visitor who was
bucketed while anonymous keeps the same value after logging in, as long as
the SDK sends `$anon_distinct_id` (posthog-js does) or the identify has
already been applied.

## What is not supported

Flags are person-based. Not supported: cohort conditions, group-based flags
(group aggregation), flag dependencies, early access features, holdouts,
scheduled changes and experiments. Creating a flag with a cohort or group
condition is rejected (`400`), and the PostHog importer skips such flags
(see [Migrate from PostHog](migrate-from-posthog.md)).

## Evaluate a user

In the dashboard, open a flag and use "test a user". Over the API:

```sh
curl -s -H 'Authorization: Bearer phx_your_key' \
  'https://analytics.example.com/api/projects/<project-id>/feature_flags/<flag-id>/evaluate?distinct_id=user_123'
# {"key":"new-checkout","enabled":false,"variant":null,"reason":"out_of_rollout_bound","condition_index":0,"payload":null}
```

`reason` is `condition_match`, `no_condition_match`, `out_of_rollout_bound`
(a condition matched on properties but the person is outside the rollout), or
`disabled`. `condition_index` is the 0-based condition group behind the answer.
This uses the stored person properties only.

## What SDKs call

| Endpoint | Used by |
|---|---|
| `POST /flags` | Current posthog-js, posthog-node, posthog-python. `?v=2` or higher returns the detailed `flags` map; no `v` or `v=1` returns `featureFlags` and `featureFlagPayloads`. |
| `POST /decide` | Older SDKs. `v=1` list of enabled keys, `v=2` key-to-value, `v=3` like `/flags` v1, `v=4` and up like `/flags` v2. |
| `GET /flags/definitions?token=phc_...` and `GET /api/feature_flag/local_evaluation?token=phc_...` | Local evaluation. Needs `Authorization: Bearer phx_...`. |

Request bodies may be JSON, gzip, base64 or form-encoded, as PostHog's SDKs
send them. The request carries the `token`, `distinct_id`, optional
`person_properties`, `$anon_distinct_id`, and `flag_keys` /
`flag_keys_to_evaluate` to limit which flags are computed. `disable_flags: true`
returns none.

Errors: `400` for an undecodable body or a missing `distinct_id`, `401` for a
missing or unknown token (neither is retried by SDKs), `503` when flag
definitions cannot be read (retried). The response sets
`errorsWhileComputingFlags: true` when stored person properties could not be
read; flags are then evaluated on the request's `person_properties` alone.

## Local evaluation

Server SDKs can download the definitions and decide in-process. You need a
personal API key (`phx_...`, a `read` key is enough) given to the SDK as
`personalApiKey` / `personal_api_key`; setup is in
[SDK setup](sdk-setup.md#local-evaluation-server-sdks).

The definitions are PostHog's local-evaluation format. The endpoint supports
`If-None-Match` and returns `304` when nothing changed, so polling is cheap.
Hoglet's definitions contain only person-property conditions, so there are no
cohorts or group mappings in them (`cohorts` and `group_type_mapping` are
empty).

Responses: `401` without a personal key or with a bad `token`, `403` if the
key's user cannot access that project.

## Flags API

All under `/api/projects/{project_id}/feature_flags`, with a session or a
personal API key. Reads need project membership. Writes need an owner or admin
session or a `write`-scoped key.

| Request | Does |
|---|---|
| `GET /feature_flags` | List flags. |
| `POST /feature_flags` | Create. Body: `key`, optional `name`, `active` (default true), `filters`, `ensure_experience_continuity`. `409` if the key exists, `400 invalid_flag` with a message naming the field. |
| `GET /feature_flags/{id}` | One flag (numeric `id`). |
| `PATCH /feature_flags/{id}` | Change any of the fields. |
| `DELETE /feature_flags/{id}` | Delete (`204`). |
| `GET /feature_flags/{id}/evaluate?distinct_id=` | Evaluate one person. |

Each change bumps the flag's version and takes effect on the next SDK request.
Limits: 2,000 flags per project, 50 condition groups per flag, 50 property
filters per group.
