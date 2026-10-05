// Copy-paste install snippets: stock PostHog SDKs, pointed at this server.

export interface SnippetDef {
  id: string;
  label: string;
  language: string;
  install?: string;
  code: (token: string, host: string) => string;
}

export const SNIPPETS: SnippetDef[] = [
  {
    id: "web",
    label: "posthog-js",
    language: "js",
    install: "npm install posthog-js",
    code: (token, host) => `import posthog from 'posthog-js'

posthog.init('${token}', {
  api_host: '${host}',
})

// After login, link this browser to the user:
// posthog.identify(user.id, { email: user.email })`,
  },
  {
    id: "html",
    label: "HTML snippet",
    language: "html",
    code: (token, host) => `<script>
  !function(t,e){var o,n,p,r;e.__SV||(window.posthog=e,e._i=[],e.init=function(i,s,a){function g(t,e){var o=e.split(".");2==o.length&&(t=t[o[0]],e=o[1]),t[e]=function(){t.push([e].concat(Array.prototype.slice.call(arguments,0)))}}(p=t.createElement("script")).type="text/javascript",p.crossOrigin="anonymous",p.async=!0,p.src="https://us-assets.i.posthog.com/static/array.js",(r=t.getElementsByTagName("script")[0]).parentNode.insertBefore(p,r);var u=e;for(void 0!==a?u=e[a]=[]:a="posthog",u.people=u.people||[],u.toString=function(t){var e="posthog";return"posthog"!==a&&(e+="."+a),t||(e+=" (stub)"),e},u.people.toString=function(){return u.toString(1)+".people (stub)"},o="init capture register register_once unregister identify alias set_config reset opt_in_capturing opt_out_capturing getFeatureFlag getFeatureFlagPayload isFeatureEnabled reloadFeatureFlags onFeatureFlags".split(" "),n=0;n<o.length;n++)g(u,o[n]);e._i.push([i,s,a])},e.__SV=1)}(document,window.posthog||[]);
  posthog.init('${token}', { api_host: '${host}' })
</script>`,
  },
  {
    id: "node",
    label: "posthog-node",
    language: "js",
    install: "npm install posthog-node",
    code: (token, host) => `import { PostHog } from 'posthog-node'

const posthog = new PostHog('${token}', { host: '${host}' })

posthog.capture({ distinctId: 'user_123', event: 'order_placed', properties: { total: 42 } })

// Flush before the process exits:
await posthog.shutdown()`,
  },
  {
    id: "python",
    label: "posthog-python",
    language: "python",
    install: "pip install posthog",
    code: (token, host) => `from posthog import Posthog

posthog = Posthog('${token}', host='${host}')

posthog.capture('order_placed', distinct_id='user_123', properties={'total': 42})

posthog.shutdown()`,
  },
  {
    id: "curl",
    label: "curl",
    language: "sh",
    code: (token, host) => `curl -X POST ${host}/i/v0/e/ \\
  -H 'Content-Type: application/json' \\
  -d '{"api_key": "${token}", "event": "hello_hoglet", "distinct_id": "user_123"}'`,
  },
];

export function hostOrigin(): string {
  return window.location.origin;
}
