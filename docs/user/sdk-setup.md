# SDK setup

Hoglet speaks PostHog's wire protocol. Use the stock PostHog SDKs and change
one thing: the host. Nothing Hoglet-specific is installed in your app.

You need two values:

- the **project token** `phc_...` (Settings, Project; safe to ship to browsers)
- your Hoglet address, for example `https://analytics.example.com`

Contract tests run real posthog-js (in a browser), posthog-node and
posthog-python at their `latest` versions against Hoglet. The React and
Next.js wrappers are posthog-js underneath; they send the same requests but
are not tested as separate suites.

Hoglet declares these posthog-js features off in its remote config, so the SDK
never calls endpoints Hoglet does not have: session recording, surveys,
heatmaps, web performance capture and exception autocapture.

## posthog-js

npm:

```sh
npm install posthog-js
```

```js
import posthog from 'posthog-js'

posthog.init('phc_your_project_token', {
  api_host: 'https://analytics.example.com',
})

// After login, link this browser to the user:
posthog.identify(user.id, { email: user.email })

// On logout:
posthog.reset()

posthog.capture('order_placed', { total: 42 })
```

HTML snippet: Settings, Project, Install snippets, "HTML snippet" gives you the
standard PostHog loader. That loader fetches `array.js` from PostHog's CDN
(`us-assets.i.posthog.com`), not from Hoglet. If you do not want a
third-party request, serve the file yourself. It ships in the npm package:

```sh
npm install posthog-js
cp node_modules/posthog-js/dist/array.js public/posthog.js
```

```html
<script src="/posthog.js"></script>
<script>
  posthog.init('phc_your_project_token', { api_host: 'https://analytics.example.com' })
</script>
```

The SDK later loads small extension files from `<api_host>/static/`. Hoglet
serves a no-op `surveys.js` there and nothing else.

## React

```jsx
import posthog from 'posthog-js'
import { PostHogProvider, useFeatureFlagEnabled } from 'posthog-js/react'

posthog.init('phc_your_project_token', {
  api_host: 'https://analytics.example.com',
})

export function App() {
  return (
    <PostHogProvider client={posthog}>
      <Checkout />
    </PostHogProvider>
  )
}

function Checkout() {
  const enabled = useFeatureFlagEnabled('new-checkout')
  return enabled ? <NewCheckout /> : <OldCheckout />
}
```

## Next.js (App Router)

```jsx
// app/providers.jsx
'use client'
import { useEffect } from 'react'
import posthog from 'posthog-js'
import { PostHogProvider } from 'posthog-js/react'

export function Providers({ children }) {
  useEffect(() => {
    posthog.init(process.env.NEXT_PUBLIC_POSTHOG_KEY, {
      api_host: process.env.NEXT_PUBLIC_POSTHOG_HOST,
      capture_pageview: 'history_change', // route changes in a single-page app
    })
  }, [])
  return <PostHogProvider client={posthog}>{children}</PostHogProvider>
}
```

Wrap `{children}` in `app/layout.jsx` with `<Providers>`. Set
`NEXT_PUBLIC_POSTHOG_HOST` to your Hoglet address, or to a first-party path
(see below). `capture_pageview: 'history_change'` needs a recent posthog-js;
on older versions capture `$pageview` yourself on route change.

For server code (route handlers, server actions) use posthog-node below.

## posthog-node

```sh
npm install posthog-node
```

```js
import { PostHog } from 'posthog-node'

const posthog = new PostHog('phc_your_project_token', {
  host: 'https://analytics.example.com',
})

posthog.capture({ distinctId: 'user_123', event: 'order_placed', properties: { total: 42 } })
posthog.identify({ distinctId: 'user_123', properties: { email: 'a@example.com' } })

// Short-lived processes (scripts, serverless): flush before exit.
await posthog.shutdown()
```

Events are batched and posted to `/batch/`. Hoglet answers 4xx for requests
the client should not retry and 5xx only for retryable failures (see
[API](api.md)).

## posthog-python

```sh
pip install posthog
```

```python
from posthog import Posthog

posthog = Posthog('phc_your_project_token', host='https://analytics.example.com')

posthog.capture('order_placed', distinct_id='user_123', properties={'total': 42})
posthog.set(distinct_id='user_123', properties={'plan': 'pro'})

posthog.shutdown()  # flush before the process exits
```

## Any HTTP client

Every capture endpoint PostHog uses is served: `/e/`, `/i/v0/e/`, `/capture/`,
`/track/`, `/engage/` (single events or arrays) and `/batch/` (server SDK
batches). Trailing slashes are optional.

One event:

```sh
curl -X POST https://analytics.example.com/i/v0/e/ \
  -H 'Content-Type: application/json' \
  -d '{"api_key":"phc_your_project_token","event":"hello_hoglet","distinct_id":"user_123"}'
# {"status":1}
```

A batch, with an identify and an alias:

```sh
curl -X POST https://analytics.example.com/batch/ \
  -H 'Content-Type: application/json' \
  -d '{
    "api_key": "phc_your_project_token",
    "batch": [
      {"event": "$identify", "distinct_id": "user_123",
       "properties": {"$anon_distinct_id": "anon_abc", "$set": {"email": "a@example.com"}}},
      {"event": "$create_alias", "distinct_id": "user_123",
       "properties": {"alias": "old_id"}}
    ]
  }'
```

Event fields: `event` and `distinct_id` are required. `properties`,
`timestamp` (ISO 8601; default is now), `uuid` (default is generated) and
`offset` (milliseconds ago, wins over `timestamp`) are optional. The token goes
in `api_key` (batch level or per event) or `token`. Gzip bodies work too:

```sh
printf '{"api_key":"phc_your_project_token","event":"gz","distinct_id":"u1"}' \
  | gzip | curl -X POST 'https://analytics.example.com/e/?compression=gzip' \
      -H 'Content-Type: text/plain' --data-binary @-
```

`?beacon=1` returns `204` instead of `{"status":1}`. Limits: 2 MiB per request
on `/e` and its aliases, 20 MiB on `/batch`, 64 MiB after decompression.
`distinct_id` is cut at 200 characters.

## First-party proxy (ad-blockers)

Browser ad-blockers match known analytics domains and paths. The reliable fix
is to serve Hoglet under your own domain and an unremarkable path, then set
`api_host` to that path. Hoglet cannot live under a path prefix itself, so the
proxy must strip the prefix.

```js
posthog.init('phc_your_project_token', {
  api_host: 'https://www.example.com/ingest',
})
```

Next.js (`next.config.js`):

```js
module.exports = {
  skipTrailingSlashRedirect: true, // keep the trailing slash on /e/ and /batch/
  async rewrites() {
    return [
      { source: '/ingest/static/:path*', destination: 'https://analytics.example.com/static/:path*' },
      { source: '/ingest/:path*', destination: 'https://analytics.example.com/:path*' },
    ]
  },
}
```

Caddy, on the site that serves your app:

```
handle_path /ingest/* {
	reverse_proxy https://analytics.example.com {
		header_up Host {upstream_hostport}
	}
}
```

nginx (trailing slashes on both lines strip the prefix):

```nginx
location /ingest/ {
    proxy_pass https://analytics.example.com/;
    proxy_set_header Host analytics.example.com;
    proxy_ssl_server_name on;
}
```

These proxy snippets follow standard Caddy and nginx behavior and were not run
against a live proxy while writing this page. Test with your SDK and watch
for `401`/`404` in the browser network tab. Pick a path that is not
`/analytics`, `/posthog`, `/track` or similar. If the proxy and Hoglet are on
the same machine, point the proxy at `http://127.0.0.1:8000` instead and drop
the `Host` lines.

## Identity in one minute

- Anonymous visitors get a random `distinct_id`.
- `posthog.identify(userId)` (or an `$identify` event with
  `$anon_distinct_id`) merges that anonymous history into the user. Person
  counts then see one person, not two.
- `alias` links two ids. `$merge_dangerously` forces a merge.
- Merge rules and what they change are in
  [Insights and queries](insights-and-queries.md#people-and-identity).

## Feature flags

Create flags in the dashboard (Feature flags) or through the
[API](feature-flags.md). The SDKs ask Hoglet at `/flags` (or `/decide`) and
get the same answers PostHog would give for the same flag definition.

### posthog-js and React

```js
posthog.onFeatureFlags(() => {
  if (posthog.isFeatureEnabled('new-checkout')) { /* ... */ }
})

posthog.getFeatureFlag('new-checkout')          // true, false, or a variant key
posthog.getFeatureFlagPayload('new-checkout')   // JSON payload, or undefined
posthog.reloadFeatureFlags()                    // after changing person properties
```

React hooks: `useFeatureFlagEnabled('key')`, `useFeatureFlagVariantKey('key')`,
`useFeatureFlagPayload('key')`. Flags reload on their own after
`posthog.identify`.

Flags that match on person properties use the properties Hoglet has stored for
the person. To use values the server has not seen yet, pass them:
`posthog.setPersonPropertiesForFlags({ plan: 'pro' })`.

### posthog-node

```js
const enabled = await posthog.isFeatureEnabled('new-checkout', 'user_123')
const variant = await posthog.getFeatureFlag('new-checkout', 'user_123')
const payload = await posthog.getFeatureFlagPayload('new-checkout', 'user_123')
const all = await posthog.getAllFlags('user_123', { personProperties: { plan: 'pro' } })
```

By default the SDK also sends a `$feature_flag_called` event per check. Pass
`{ sendFeatureFlagEvents: false }` as the last argument to turn that off.

### posthog-python

```python
posthog.feature_enabled('new-checkout', 'user_123')
posthog.get_feature_flag('new-checkout', 'user_123')
posthog.get_feature_flag_payload('new-checkout', 'user_123')
posthog.get_all_flags('user_123', person_properties={'plan': 'pro'})
```

### Local evaluation (server SDKs)

Without it, every flag check is a network call to Hoglet. With local
evaluation the SDK downloads the flag definitions once, refreshes them on an
interval, and decides in-process. It needs a **personal API key** (`phx_...`):

1. Dashboard, Settings, API keys, create a key. A `read` key is enough.
2. Give it to the SDK. The key is a secret; keep it on servers only, never in
   browser code.

```js
const posthog = new PostHog('phc_your_project_token', {
  host: 'https://analytics.example.com',
  personalApiKey: 'phx_your_personal_key',
  featureFlagsPollingInterval: 30000, // ms
})
await posthog.waitForLocalEvaluationReady()
```

```python
posthog = Posthog(
    'phc_your_project_token',
    host='https://analytics.example.com',
    personal_api_key='phx_your_personal_key',
    poll_interval=30,  # seconds
)
```

The SDK calls `GET /flags/definitions?token=phc_...` (or
`/api/feature_flag/local_evaluation`) with `Authorization: Bearer phx_...`.
Hoglet answers `401` without a personal key and `403` if the key's user has no
access to that project. Bucketing is PostHog's, so a local decision equals a
server decision for the same inputs. If a flag needs a person property the SDK
was not given, the SDK falls back to a `/flags` request. Details:
[Feature flags](feature-flags.md).
