// posthog-js `latest`, unmodified, in headless Chromium against a real
// Hoglet (claims.md claim 1). The page is served from a local origin
// (`localhost`) and talks cross-origin to `127.0.0.1`, so CORS is exercised.
// Requests go through the counting proxy so retries, beacons and statuses
// are observed at the wire, not inferred from the browser.

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import http from "node:http";
import { chromium } from "playwright";

import {
  FLAG_PAYLOAD_ON,
  Hoglet,
  Proxy,
  assertDauIsOne,
  createFlags,
  decodeBody,
  eventsIn,
  expectedVariant,
  isCapturePath,
  listEvents,
  poll,
  repeatedRequests,
  runId,
  setupWorkspace,
  waitForEvents,
  waitForSinglePerson,
} from "./lib/harness.mjs";
import { Report, eq, fail, ok } from "./lib/report.mjs";

const pkg = (name) => JSON.parse(readFileSync(new URL(`./node_modules/${name}/package.json`, import.meta.url))).version;
const SDK_VERSION = pkg("posthog-js");
const report = new Report("browser");
report.meta.sdk = { "posthog-js": SDK_VERSION, playwright: pkg("playwright") };
report.header(`browser suite — posthog-js ${SDK_VERSION} (playwright ${pkg("playwright")})`);

const hoglet = new Hoglet({ label: "browser" });
await hoglet.start();
const proxy = await new Proxy(hoglet).listen();

const ws = await report.step("setup: POST /api/auth/setup returns session cookie, project id and token", () =>
  setupWorkspace(hoglet.url, "browser")
);
if (!ws) process.exit(report.finish());
const { api, projectId, token } = ws;
await createFlags(report, api, projectId);

const run = runId();
const user = `browser-${run}@contract.test`;

// ── the host page ────────────────────────────────────────────────────────

const SDK_JS = readFileSync(new URL("./node_modules/posthog-js/dist/array.js", import.meta.url));
const page1 = `<!doctype html><html><head><title>contract</title><link rel="icon" href="data:,"></head>
<body><h1>contract</h1><button id="cta">Buy</button>
<script src="/posthog.js"></script>
<script>
  window.__flagCalls = [];
  posthog.init(${JSON.stringify(token)}, {
    api_host: ${JSON.stringify(proxy.url)},
    // posthog-js drops events from navigator.webdriver browsers (bot
    // filter); this is the SDK's own switch for automation, not a Hoglet option.
    opt_out_useragent_filter: true,
    loaded: () => { window.__loaded = true; },
  });
  posthog.register({ ct_run: ${JSON.stringify(run)} });
  posthog.onFeatureFlags((flags, variants) => window.__flagCalls.push({ flags, variants }));
</script></body></html>`;
const page2 = `<!doctype html><html><head><title>next</title><link rel="icon" href="data:,"></head><body>next</body></html>`;

const site = http.createServer((req, res) => {
  const path = new URL(req.url, "http://x").pathname;
  if (path === "/") return res.writeHead(200, { "content-type": "text/html" }).end(page1);
  if (path === "/next") return res.writeHead(200, { "content-type": "text/html" }).end(page2);
  if (path === "/posthog.js") return res.writeHead(200, { "content-type": "text/javascript" }).end(SDK_JS);
  res.writeHead(404).end();
});
const sitePort = await new Promise((r) => site.listen(0, "127.0.0.1", () => r(site.address().port)));
const origin = `http://localhost:${sitePort}`;

const browser = await chromium.launch({ headless: true });
// Chrome's Local Network Access gate: real sites target a routable host;
// granting it here is rig plumbing, not a weakened claim.
const context = await browser.newContext({ permissions: ["local-network-access"] }).catch(() => browser.newContext());
const page = await context.newPage();
const consoleErrors = [];
page.on("console", (m) => {
  if (m.type() === "error") consoleErrors.push(m.text());
});
page.on("pageerror", (e) => consoleErrors.push(`pageerror: ${e.message}`));

const wireEvents = (records) => records.filter((r) => isCapturePath(r.path)).flatMap((r) => eventsIn(decodeBody(r.body)).map((e) => ({ e, r })));
const flagCalls = () => page.evaluate(() => window.__flagCalls.length);

try {
  await report.step("init: posthog-js loads from the page and init's loaded callback fires", async () => {
    await page.goto(`${origin}/`);
    await page.waitForFunction(() => window.__loaded === true, null, { timeout: 10_000 });
  });

  await report.step("init: remote config /array/{token}/config fetched with 200 and parseable JSON", async () => {
    const cfg = await poll(() => {
      const r = proxy.log.find((r) => r.path.startsWith(`/array/${token}/config`) && r.status !== null && r.response);
      if (!r) fail("no config request completed", proxy.log.map((r) => `${r.method} ${r.path}`), `GET /array/${token}/config`);
      return r;
    }, { timeoutMs: 5_000 });
    eq(cfg.status, 200, "config status");
    const text = cfg.response.toString("utf8");
    const parsed = cfg.path.endsWith(".js") ? text.length > 0 : JSON.parse(text);
    ok(parsed, "config body", text.slice(0, 200), "JSON object");
  });

  const anonId = await page.evaluate(() => posthog.get_distinct_id());

  await report.step("capture: autocaptured $pageview reaches a capture endpoint and is acknowledged 200", () =>
    poll(() => {
      const hit = wireEvents(proxy.log).find(({ e }) => e.event === "$pageview");
      if (!hit) fail("no $pageview on the wire", wireEvents(proxy.log).map(({ e }) => e.event), "$pageview");
      eq(hit.r.status, 200, "capture status");
    }, { timeoutMs: 10_000 })
  );

  await report.step("flags: onFeatureFlags fires and flags resolve for the anonymous user", async () => {
    await page.waitForFunction(() => window.__flagCalls.length > 0, null, { timeout: 10_000 });
    const got = await page.evaluate(() => ({
      on: posthog.isFeatureEnabled("ct-bool-on"),
      off: posthog.isFeatureEnabled("ct-bool-off"),
      inactive: !!posthog.isFeatureEnabled("ct-inactive"),
      mv: posthog.getFeatureFlag("ct-multivariate"),
      payload: posthog.getFeatureFlagPayload("ct-bool-on"),
    }));
    eq(got, { on: true, off: false, inactive: false, mv: expectedVariant("ct-multivariate", anonId), payload: FLAG_PAYLOAD_ON }, "flags");
  });

  const callsBeforeIdentify = await flagCalls();
  await page.evaluate((u) => posthog.identify(u, { plan: "enterprise", email: u }), user);

  await report.step("identify: flags reload for the identified user (variant + person-property flag)", async () => {
    await page.waitForFunction((n) => window.__flagCalls.length > n, callsBeforeIdentify, { timeout: 10_000 });
    const got = await page.evaluate(() => ({
      id: posthog.get_distinct_id(),
      mv: posthog.getFeatureFlag("ct-multivariate"),
      plan: posthog.isFeatureEnabled("ct-person-plan"),
    }));
    eq(got, { id: user, mv: expectedVariant("ct-multivariate", user), plan: true }, "flags after identify");
  });

  await report.step("identify: $identify carries $anon_distinct_id and is acknowledged 200", () =>
    poll(() => {
      const hit = wireEvents(proxy.log).find(({ e }) => e.event === "$identify");
      if (!hit) fail("no $identify on the wire", wireEvents(proxy.log).map(({ e }) => e.event), "$identify");
      eq({ anon: hit.e.properties?.$anon_distinct_id, status: hit.r.status }, { anon: anonId, status: 200 }, "$identify");
    }, { timeoutMs: 10_000 })
  );

  await report.step("capture: send_instantly event leaves within 1.5s and is acknowledged 200", async () => {
    const t0 = Date.now();
    await page.evaluate(() => posthog.capture("ct_instant", { kind: "instant" }, { send_instantly: true }));
    await poll(() => {
      const hit = wireEvents(proxy.log).find(({ e }) => e.event === "ct_instant");
      if (!hit) fail("ct_instant not on the wire", null, "ct_instant");
      eq(hit.r.status, 200, "capture status");
      ok(hit.r.t - t0 <= 1500, "sent instantly", `${hit.r.t - t0}ms`, "<= 1500ms");
    }, { timeoutMs: 3_000, intervalMs: 50 });
  });

  // posthog-js flushes its queue with navigator.sendBeacon on pagehide. A
  // beacon is a no-cors request (no preflight, response unreadable), so it
  // must be accepted as sent: CORS-simple content type, base64 body.
  // spec: `beacon=1` in the query → 204.
  await report.step("beacon: queued event is flushed with sendBeacon on unload and acknowledged 2xx (204 if beacon=1)", async () => {
    const mark = proxy.mark();
    await page.evaluate(() => posthog.capture("ct_beacon", { kind: "beacon" }));
    await page.goto(`${origin}/next`);
    await poll(() => {
      const hit = wireEvents(proxy.since(mark)).find(({ e }) => e.event === "ct_beacon");
      if (!hit) fail("ct_beacon not on the wire", proxy.since(mark).map((r) => `${r.method} ${r.path}`), "ct_beacon");
      const r = hit.r;
      const observed = {
        transport: r.headers["sec-fetch-mode"] === "no-cors" ? "sendBeacon" : `other (sec-fetch-mode=${r.headers["sec-fetch-mode"]})`,
        status: r.status,
      };
      const expected = { transport: "sendBeacon", status: r.query.beacon === "1" ? 204 : r.status >= 200 && r.status < 300 ? r.status : "2xx" };
      eq(observed, expected, `beacon request ${r.path}?${new URLSearchParams(r.query)}`);
    }, { timeoutMs: 5_000 });
  });

  // Older posthog-js marked beacons with `?beacon=1` (spec: answer 204).
  // Latest no longer does, so send one the way it used to, from the page.
  await report.step("beacon: legacy sendBeacon to /e/?beacon=1 (form data=base64) answers 204", async () => {
    const mark = proxy.mark();
    const queued = await page.evaluate(
      ([host, tok, id]) => {
        const event = { event: "ct_legacy_beacon", properties: { distinct_id: id, token: tok, kind: "legacy-beacon" } };
        const body = "data=" + encodeURIComponent(btoa(JSON.stringify(event)));
        return navigator.sendBeacon(`${host}/e/?compression=base64&beacon=1`, new Blob([body], { type: "application/x-www-form-urlencoded" }));
      },
      [proxy.url, token, user]
    );
    ok(queued, "navigator.sendBeacon queued the request", queued, true);
    await poll(() => {
      const hit = proxy.since(mark).find((r) => r.query.beacon === "1");
      if (!hit || hit.status === null) fail("beacon=1 request not answered yet", null, "a response");
      eq(hit.status, 204, "status");
    }, { timeoutMs: 5_000, intervalMs: 50 });
  });

  await report.step("wire: no capture/flags/config request answered 4xx/5xx", () => {
    const bad = proxy.log.filter((r) => r.method !== "OPTIONS" && !r.path.startsWith("/static/") && (typeof r.status !== "number" || r.status >= 400));
    eq(bad.map((r) => `${r.method} ${r.path} → ${r.status}`), [], "failed requests");
  });

  // posthog-js lazy-loads extension bundles from `${api_host}/static/` once
  // remote config arrives (surveys.js even when `surveys: false`). Against
  // PostHog these exist; a 404 without CORS headers is a console error.
  await report.step("wire: extension assets the SDK requests from /static/ load", () => {
    const bad = proxy.log.filter((r) => r.method !== "OPTIONS" && r.path.startsWith("/static/") && (typeof r.status !== "number" || r.status >= 400));
    eq(bad.map((r) => `${r.method} ${r.path} → ${r.status}`), [], "failed asset requests");
  });

  await report.step("wire: no request was retried (no repeated method+path+payload)", () =>
    eq(repeatedRequests(proxy.log), [], "repeated requests")
  );

  await report.step("console: no errors logged by the page or SDK", () => eq(consoleErrors, [], "console errors"));

  // ── stored outcomes ────────────────────────────────────────────────────

  const names = ["$pageview", "$identify", "ct_instant", "ct_beacon", "ct_legacy_beacon"];
  const stored = await report.step("stored: $pageview, $identify, ct_instant, ct_beacon and ct_legacy_beacon are listed by GET /events", () =>
    poll(async () => {
      const events = await listEvents(api, projectId);
      const have = names.filter((n) => events.some((e) => e.event === n));
      eq(have, names, "stored event names");
      return events;
    })
  );
  await report.step("stored: $pageview has the anonymous distinct_id and the page URL", () => {
    const pv = stored?.find((e) => e.event === "$pageview") ?? fail("no stored $pageview", null, "$pageview");
    eq({ distinct_id: pv.distinct_id, url: pv.properties?.$current_url, run: pv.properties?.ct_run }, { distinct_id: anonId, url: `${origin}/`, run }, "$pageview");
  });
  await report.step("stored: identify merged the anonymous and identified ids into ONE person", () =>
    waitForSinglePerson(api, projectId, [anonId, user])
  );
  await report.step("stored: TrendsQuery math=dau over all events counts 1 person", () => assertDauIsOne(api, projectId));

  // ── retry contract (last: it deliberately creates a retry) ─────────────

  await page.goto(`${origin}/`);
  await page.waitForFunction(() => window.__loaded === true, null, { timeout: 10_000 });
  const marker = randomUUID();
  const isMarked = (r) => isCapturePath(r.path) && eventsIn(decodeBody(r.body)).some((e) => e.properties?.marker === marker);
  proxy.inject(isMarked, { status: 503 });
  const mark = proxy.mark();
  const retried = await report.step("retry: 503 → posthog-js retries until acknowledged", async () => {
    await page.evaluate((m) => posthog.capture("ct_retry", { marker: m }, { send_instantly: true }), marker);
    return await poll(() => {
      const attempts = proxy.since(mark).filter(isMarked);
      if (!(attempts.length >= 2 && attempts.at(-1).status === 200)) fail("attempts", attempts.map((r) => r.status), "[503, 200]");
      return attempts;
    }, { timeoutMs: 20_000, intervalMs: 100 });
  });
  if (retried) {
    await report.step("retry: event retried after 503 is stored exactly once", async () => {
      const uuid = eventsIn(decodeBody(retried.at(-1).body)).find((e) => e.properties?.marker === marker)?.uuid;
      ok(uuid, "retried event has a uuid", uuid, "uuid");
      const events = await waitForEvents(api, projectId, [uuid]);
      eq(events.filter((e) => e.uuid === uuid).length, 1, "stored copies");
    });
  } else report.skip("retry: event retried after 503 is stored exactly once", "retry did not complete");
} finally {
  await browser.close();
  site.close();
  await proxy.close();
  await hoglet.stop();
}

const code = report.finish();
if (code !== 0) report.info(`hoglet log tail:\n${hoglet.logTail(15)}`);
hoglet.destroy();
process.exit(code);
