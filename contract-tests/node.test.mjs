// posthog-node `latest`, unmodified, against a real Hoglet (claims.md
// claim 1). Every SDK-observable result is asserted through the SDK itself;
// every stored outcome through the dashboard API. A server that answers 200
// to everything fails almost every step here.

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { PostHog } from "posthog-node";

import {
  FLAG_PAYLOAD_ON,
  Hoglet,
  Proxy,
  VARIANT_PAYLOADS,
  createFlags,
  createPersonalKey,
  decodeBody,
  eventUuids,
  eventsIn,
  expectedVariant,
  poll,
  repeatedRequests,
  runId,
  setupWorkspace,
  waitForEvents,
} from "./lib/harness.mjs";
import { VIEWED_PROPS, storedOutcomeSteps } from "./lib/outcomes.mjs";
import { Report, eq, ok, subset } from "./lib/report.mjs";

const SDK_VERSION = JSON.parse(readFileSync(new URL("./node_modules/posthog-node/package.json", import.meta.url))).version;
const report = new Report("node");
report.meta.sdk = { "posthog-node": SDK_VERSION };
report.header(`node suite — posthog-node ${SDK_VERSION}`);

const hoglet = new Hoglet({ label: "node" });
await hoglet.start();
const proxy = await new Proxy(hoglet).listen();
report.info(`hoglet ${hoglet.url} (data ${hoglet.dataDir}), proxy ${proxy.url}`);

const ws = await report.step("setup: POST /api/auth/setup returns session cookie, project id and token", () =>
  setupWorkspace(hoglet.url, "node")
);
if (!ws) {
  console.log(hoglet.logTail());
  process.exit(report.finish());
}
const { api, projectId, token } = ws;
const personalKey = await report.step("setup: POST /api/auth/keys returns a phx_ personal API key", () => createPersonalKey(api));
await createFlags(report, api, projectId);

const run = runId();
const anon = `anon-${run}`;
const anon2 = `anon2-${run}`;
const user = `user-${run}@contract.test`;
const company = `acme-${run}`;
const noEvents = { sendFeatureFlagEvents: false };

// ── 1. capture / identify / alias / $set / $set_once ─────────────────────

const ph = new PostHog(token, { host: proxy.url, flushAt: 100, flushInterval: 0 });
const sentTs = new Date(Date.now() - 60_000);
const uuids = {
  viewed: randomUUID(),
  grouped: randomUUID(),
  anon2: randomUUID(),
  after: randomUUID(),
};
const viewedProps = VIEWED_PROPS(run);
const captureMark = proxy.mark();
ph.capture({ distinctId: anon, event: "ct_signup_viewed", properties: viewedProps, timestamp: sentTs, uuid: uuids.viewed });
ph.capture({ distinctId: anon, event: "ct_grouped", properties: { ct_run: run }, groups: { company }, uuid: uuids.grouped });
ph.identify({
  distinctId: user,
  properties: { $set: { plan: "enterprise", email: user }, $set_once: { first_plan: "free" }, $anon_distinct_id: anon },
});
ph.capture({ distinctId: anon2, event: "ct_anon2_event", properties: { ct_run: run }, uuid: uuids.anon2 });
ph.alias({ distinctId: user, alias: anon2 });
ph.setPersonProperties({ distinctId: user, properties: { seats: 5 }, propertiesOnce: { first_plan: "pro" } });
ph.capture({ distinctId: user, event: "ct_after_login", properties: { ct_run: run }, uuid: uuids.after });

await report.step("capture: flush() resolves and every /batch/ request is acknowledged 2xx exactly once", async () => {
  await ph.flush();
  const sent = proxy.since(captureMark).filter((r) => r.path.startsWith("/batch"));
  ok(sent.length >= 1, "batch requests sent", sent.length, ">= 1");
  eq(sent.map((r) => r.status), sent.map(() => 200), "batch response statuses");
  eq(repeatedRequests(sent), [], "retried batch payloads");
  const names = sent.flatMap((r) => eventsIn(decodeBody(r.body))).map((e) => e.event);
  eq(names.sort(), ["$create_alias", "$identify", "$set", "ct_after_login", "ct_anon2_event", "ct_grouped", "ct_signup_viewed"], "events on the wire");
});

// ── 2. remote flag evaluation through the SDK ────────────────────────────

const flagIds = Array.from({ length: 12 }, (_, i) => `mv-${run}-${i}`);

await report.step("flags: isFeatureEnabled(ct-bool-on) === true", async () =>
  eq(await ph.isFeatureEnabled("ct-bool-on", user, noEvents), true, "isFeatureEnabled")
);
await report.step("flags: isFeatureEnabled(ct-bool-off) === false", async () =>
  eq(await ph.isFeatureEnabled("ct-bool-off", user, noEvents), false, "isFeatureEnabled")
);
// PostHog omits inactive flags from /flags, so the SDK may answer undefined.
await report.step("flags: isFeatureEnabled(ct-inactive) is not on", async () => {
  const v = await ph.isFeatureEnabled("ct-inactive", user, noEvents);
  ok(!v, "isFeatureEnabled", v, "false or undefined");
});
await report.step("flags: getFeatureFlag(ct-multivariate) matches PostHog's variant hash for 12 ids", async () => {
  const got = {};
  const want = {};
  for (const id of flagIds) {
    got[id] = await ph.getFeatureFlag("ct-multivariate", id, noEvents);
    want[id] = expectedVariant("ct-multivariate", id);
  }
  eq(got, want, "variants");
});
await report.step("flags: getFeatureFlagPayload(ct-bool-on) returns the JSON payload", async () =>
  eq(await ph.getFeatureFlagPayload("ct-bool-on", user), FLAG_PAYLOAD_ON, "payload")
);
await report.step("flags: getFeatureFlagPayload(ct-multivariate) returns the matched variant's payload", async () =>
  eq(await ph.getFeatureFlagPayload("ct-multivariate", user), VARIANT_PAYLOADS[expectedVariant("ct-multivariate", user)], "payload")
);
await report.step("flags: personProperties {plan: enterprise} turns ct-person-plan on", async () =>
  eq(await ph.isFeatureEnabled("ct-person-plan", `pp-${run}`, { ...noEvents, personProperties: { plan: "enterprise" } }), true, "isFeatureEnabled")
);
await report.step("flags: personProperties {plan: free} keeps ct-person-plan off", async () =>
  eq(await ph.isFeatureEnabled("ct-person-plan", `pp-${run}`, { ...noEvents, personProperties: { plan: "free" } }), false, "isFeatureEnabled")
);
const expectedAll = {
  "ct-bool-on": true,
  "ct-bool-off": false,
  "ct-multivariate": expectedVariant("ct-multivariate", user),
  "ct-person-plan": true,
};
await report.step("flags: getAllFlags returns every active flag's value", async () => {
  const all = await ph.getAllFlags(user, { personProperties: { plan: "enterprise" } });
  subset(all, expectedAll, "getAllFlags");
  ok(!all["ct-inactive"], "inactive flag is not on", all["ct-inactive"], "false or absent");
});
await report.step("flags: getAllFlagsAndPayloads returns values and payloads", async () => {
  const { featureFlags, featureFlagPayloads } = await ph.getAllFlagsAndPayloads(user, { personProperties: { plan: "enterprise" } });
  subset(featureFlags, expectedAll, "featureFlags");
  subset(
    featureFlagPayloads,
    { "ct-bool-on": FLAG_PAYLOAD_ON, "ct-multivariate": VARIANT_PAYLOADS[expectedAll["ct-multivariate"]] },
    "featureFlagPayloads"
  );
});
await report.step("flags: evaluateFlags snapshot agrees (isEnabled/getFlag/getFlagPayload)", async () => {
  const flags = await ph.evaluateFlags(user, { personProperties: { plan: "enterprise" } });
  eq(
    {
      on: flags.isEnabled("ct-bool-on"),
      off: flags.isEnabled("ct-bool-off"),
      mv: flags.getFlag("ct-multivariate"),
      payload: flags.getFlagPayload("ct-bool-on"),
    },
    { on: true, off: false, mv: expectedAll["ct-multivariate"], payload: FLAG_PAYLOAD_ON },
    "evaluateFlags"
  );
});
await report.step("flags: stored person property (identify $set plan=enterprise) enables ct-person-plan without personProperties", () =>
  poll(async () => eq(await ph.isFeatureEnabled("ct-person-plan", user, noEvents), true, "isFeatureEnabled"), { timeoutMs: 10_000 })
);

// ── 3. local evaluation with a personal API key ──────────────────────────

if (personalKey) {
  const localMark = proxy.mark();
  const local = new PostHog(token, { host: proxy.url, personalApiKey: personalKey, featureFlagsPollingInterval: 60_000, flushAt: 100 });
  const ready = await report.step("local eval: waitForLocalEvaluationReady() → true (definitions fetched)", async () => {
    eq(await local.waitForLocalEvaluationReady(10_000), true, "isLocalEvaluationReady");
    return true;
  });
  await report.step("local eval: GET /flags/definitions sent the personal key and got 200", async () => {
    const defs = proxy.since(localMark).filter((r) => r.path.startsWith("/flags/definitions") || r.path.includes("local_evaluation"));
    ok(defs.length >= 1, "definitions requests", defs.length, ">= 1");
    eq(defs[0].headers.authorization, `Bearer ${personalKey}`, "authorization header");
    eq(defs[0].status, 200, "definitions status");
  });
  if (ready) {
    const evalMark = proxy.mark();
    const pp = { personProperties: { plan: "enterprise" }, ...noEvents };
    await report.step("local eval: flags evaluate to the same values as PostHog's algorithm", async () => {
      const got = {
        on: await local.isFeatureEnabled("ct-bool-on", user, pp),
        off: await local.isFeatureEnabled("ct-bool-off", user, pp),
        inactive: await local.isFeatureEnabled("ct-inactive", user, pp),
        planOn: await local.isFeatureEnabled("ct-person-plan", user, pp),
        planOff: await local.isFeatureEnabled("ct-person-plan", user, { personProperties: { plan: "free" }, ...noEvents }),
        variants: {},
      };
      const want = { on: true, off: false, inactive: false, planOn: true, planOff: false, variants: {} };
      for (const id of flagIds) {
        got.variants[id] = await local.getFeatureFlag("ct-multivariate", id, pp);
        want.variants[id] = expectedVariant("ct-multivariate", id);
      }
      eq(got, want, "local evaluation");
    });
    await report.step("local eval: payloads resolve locally", async () => {
      eq(
        {
          on: await local.getFeatureFlagPayload("ct-bool-on", user, undefined, pp),
          mv: await local.getFeatureFlagPayload("ct-multivariate", user, undefined, pp),
        },
        { on: FLAG_PAYLOAD_ON, mv: VARIANT_PAYLOADS[expectedAll["ct-multivariate"]] },
        "payloads"
      );
    });
    await report.step("local eval: getAllFlags resolves locally", async () => {
      subset(await local.getAllFlags(user, pp), expectedAll, "getAllFlags");
    });
    await report.step("local eval: no POST /flags or /decide was made while evaluating", async () => {
      const remote = proxy.since(evalMark).filter((r) => r.method === "POST" && /^\/(flags|decide)\/?$/.test(r.path));
      eq(remote.map((r) => `${r.method} ${r.path}`), [], "remote flag requests during local evaluation");
    });
  } else {
    for (const n of ["flags evaluate locally", "payloads resolve locally", "getAllFlags resolves locally", "no POST /flags during local evaluation"])
      report.skip(`local eval: ${n}`, "definitions never became ready");
  }
  await local.shutdown();
} else {
  report.skip("local eval: all steps", "no personal API key");
}

// ── 4. stored outcomes through the dashboard API ─────────────────────────

await storedOutcomeSteps(report, api, projectId, { ids: { anon, anon2, user, company, run }, uuids, sentTs });

// ── 5. retry contract ────────────────────────────────────────────────────

const batchFor = (marker) => (r) => r.method === "POST" && r.path.startsWith("/batch") && r.body.length > 0 && JSON.stringify(decodeBody(r.body) ?? "").includes(marker);

async function flushQuietly(client) {
  try {
    await client.flush();
  } catch {}
}

await report.step("retry: unknown project token → 4xx, SDK does not retry", async () => {
  const marker = randomUUID();
  const bad = new PostHog(`phc_unknown${run}`, { host: proxy.url, flushAt: 100, flushInterval: 0 });
  const mark = proxy.mark();
  bad.capture({ distinctId: user, event: "ct_bad_token", properties: { marker } });
  await flushQuietly(bad);
  await bad.shutdown().catch(() => {});
  const attempts = proxy.since(mark).filter(batchFor(marker));
  eq(attempts.map((r) => r.status >= 400 && r.status < 500), [true], "one attempt answered 4xx");
});

await report.step("retry: malformed body → 4xx, SDK does not retry", async () => {
  const marker = randomUUID();
  const client = new PostHog(token, { host: proxy.url, flushAt: 100, flushInterval: 0 });
  proxy.inject(batchFor(marker), { corrupt: true });
  const mark = proxy.mark();
  client.capture({ distinctId: user, event: "ct_malformed", properties: { marker } });
  await flushQuietly(client);
  await client.shutdown().catch(() => {});
  const attempts = proxy.since(mark).filter(batchFor(marker));
  eq(attempts.map((r) => ({ corrupt: !!r.injected?.corrupt, is4xx: r.status >= 400 && r.status < 500 })), [{ corrupt: true, is4xx: true }], "attempts");
});

{
  const marker = randomUUID();
  const uuid = randomUUID();
  const attempts = await report.step("retry: 503 → SDK retries the identical payload until acknowledged", async () => {
    const client = new PostHog(token, { host: proxy.url, flushAt: 100, flushInterval: 0 });
    proxy.inject(batchFor(marker), { status: 503 });
    const mark = proxy.mark();
    client.capture({ distinctId: user, event: "ct_retry_503", properties: { marker }, uuid });
    await flushQuietly(client);
    await client.shutdown().catch(() => {});
    const attempts = proxy.since(mark).filter(batchFor(marker));
    eq(attempts.map((r) => r.status), [503, 200], "attempt statuses");
    eq(eventUuids(attempts[1]), eventUuids(attempts[0]), "retried the same events");
    return attempts;
  });
  if (attempts) {
    await report.step("retry: event retried after 503 is stored exactly once", async () => {
      const events = await waitForEvents(api, projectId, [uuid]);
      eq(events.filter((e) => e.uuid === uuid).length, 1, "stored copies");
    });
  } else report.skip("retry: event retried after 503 is stored exactly once", "503 retry did not complete");
}

{
  const marker = randomUUID();
  const uuid = randomUUID();
  const attempts = await report.step("retry: Hoglet stopped then restarted → SDK retries until acknowledged", async () => {
    await hoglet.stop();
    const client = new PostHog(token, { host: proxy.url, flushAt: 100, flushInterval: 0 });
    const mark = proxy.mark();
    client.capture({ distinctId: user, event: "ct_retry_restart", properties: { marker }, uuid });
    const flushing = flushQuietly(client);
    try {
      await poll(() => ok(proxy.since(mark).some(batchFor(marker)), "first attempt seen"), { timeoutMs: 5_000, intervalMs: 20 });
    } finally {
      await hoglet.start();
    }
    await flushing;
    await client.shutdown().catch(() => {});
    const attempts = proxy.since(mark).filter(batchFor(marker));
    ok(attempts.length >= 2 && attempts.at(-1).status === 200, "retried until acknowledged", attempts.map((r) => r.status), "[ECONNRESET…, 200]");
    return attempts;
  });
  if (attempts) {
    await report.step("retry: event retried across restart is stored exactly once; earlier events survive restart", async () => {
      const events = await waitForEvents(api, projectId, [uuid, ...Object.values(uuids)], { timeoutMs: 20_000 });
      eq(events.filter((e) => e.uuid === uuid).length, 1, "stored copies");
    });
  } else report.skip("retry: event retried across restart is stored exactly once; earlier events survive restart", "restart retry did not complete");
}

await report.step("shutdown: shutdown() drains without error", async () => {
  ph.capture({ distinctId: user, event: "ct_final", properties: { ct_run: run } });
  await ph.shutdown(10_000);
});

await proxy.close();
await hoglet.stop();
const code = report.finish();
if (code !== 0) report.info(`hoglet log tail:\n${hoglet.logTail(15)}`);
hoglet.destroy();
process.exit(code);
