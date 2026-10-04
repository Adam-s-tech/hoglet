// posthog-python `latest`, unmodified, against a real Hoglet (claims.md
// claim 1). python/driver.py drives the SDK one scenario per process and
// reports what the SDK returned; this runner owns Hoglet, the proxy and every
// assertion, so all suites share one harness and one reporter.

import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { createInterface } from "node:readline";

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
  repeatedRequests,
  runId,
  setupWorkspace,
  waitForEvents,
} from "./lib/harness.mjs";
import { storedOutcomeSteps } from "./lib/outcomes.mjs";
import { Report, eq, fail, ok, subset } from "./lib/report.mjs";

const HERE = new URL(".", import.meta.url).pathname;
const VENV = process.env.CONTRACT_VENV ?? `${HERE}.venv`;
const PY = `${VENV}/bin/python`;

// Fresh `latest` every run — never pinned.
if (!existsSync(PY)) {
  const made = spawnSync(process.env.PYTHON ?? "python3", ["-m", "venv", VENV], { stdio: "inherit" });
  if (made.status !== 0) throw new Error("could not create the Python venv");
}
const pip = spawnSync(PY, ["-m", "pip", "install", "--quiet", "--disable-pip-version-check", "--upgrade", "posthog"], { stdio: "inherit" });
if (pip.status !== 0) throw new Error("pip install --upgrade posthog failed");
const SDK_VERSION = spawnSync(PY, ["-c", "import posthog; print(posthog.VERSION)"], { encoding: "utf8" }).stdout.trim();

const report = new Report("python");
report.meta.sdk = { "posthog-python": SDK_VERSION };
report.header(`python suite — posthog-python ${SDK_VERSION}`);

const hoglet = new Hoglet({ label: "python" });
await hoglet.start();
const proxy = await new Proxy(hoglet).listen();

const ws = await report.step("setup: POST /api/auth/setup returns session cookie, project id and token", () =>
  setupWorkspace(hoglet.url, "python")
);
if (!ws) process.exit(report.finish());
const { api, projectId, token } = ws;
const personalKey = await report.step("setup: POST /api/auth/keys returns a phx_ personal API key", () => createPersonalKey(api));
await createFlags(report, api, projectId);

const run = runId();
const ids = { anon: `anon-${run}`, anon2: `anon2-${run}`, user: `user-${run}@contract.test`, company: `acme-${run}`, run };
const uuids = { viewed: randomUUID(), grouped: randomUUID(), anon2: randomUUID(), after: randomUUID() };
const baseEnv = {
  ...process.env,
  CT_HOST: proxy.url,
  CT_TOKEN: token,
  CT_RUN: run,
  CT_PERSONAL_KEY: personalKey ?? "",
  ...Object.fromEntries(Object.entries(uuids).map(([k, v]) => [`CT_UUID_${k.toUpperCase()}`, v])),
};

/// Runs one driver scenario. `onPhase(name)` may return a promise; the
/// driver's stdin gets a newline after each phase handler resolves.
async function drive(scenario, { env = {}, onPhase } = {}) {
  const child = spawn(PY, [`${HERE}python/driver.py`, scenario], { env: { ...baseEnv, ...env }, stdio: ["pipe", "pipe", "pipe"] });
  let stderr = "";
  child.stderr.on("data", (d) => (stderr += d));
  let done = null;
  const timer = setTimeout(() => child.kill("SIGKILL"), 90_000);
  for await (const line of createInterface({ input: child.stdout })) {
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue;
    }
    if (msg.phase === "done") done = msg.results;
    else {
      await onPhase?.(msg.phase);
      child.stdin.write("\n");
    }
  }
  const code = await new Promise((r) => (child.exitCode !== null ? r(child.exitCode) : child.once("exit", r)));
  clearTimeout(timer);
  if (!done) throw new Error(`driver ${scenario} exited ${code} without results\n${stderr.slice(-2000)}`);
  return done;
}

const want = () => ({
  on: true,
  off: false,
  plan_on: true,
  plan_off: false,
  variants: Object.fromEntries(Array.from({ length: 12 }, (_, i) => `mv-${run}-${i}`).map((id) => [id, expectedVariant("ct-multivariate", id)])),
  payload_on: FLAG_PAYLOAD_ON,
  payload_mv: VARIANT_PAYLOADS[expectedVariant("ct-multivariate", ids.user)],
  all: {
    "ct-bool-on": true,
    "ct-bool-off": false,
    "ct-multivariate": expectedVariant("ct-multivariate", ids.user),
    "ct-person-plan": true,
  },
});

async function flagSteps(prefix, r) {
  const w = want();
  const step = (name, fn) => [`${prefix}: ${name}`, fn];
  const steps = [
    step("feature_enabled(ct-bool-on) is True", () => eq(r.on, w.on, "feature_enabled")),
    step("feature_enabled(ct-bool-off) is False", () => eq(r.off, w.off, "feature_enabled")),
    // PostHog omits inactive flags from /flags, so remote evaluation may answer None.
    step("feature_enabled(ct-inactive) is not on", () => ok(!r.inactive, "feature_enabled", r.inactive, "False or None")),
    step("person_properties {plan: enterprise} turns ct-person-plan on", () => eq(r.plan_on, true, "feature_enabled")),
    step("person_properties {plan: free} keeps ct-person-plan off", () => eq(r.plan_off, false, "feature_enabled")),
    step("get_feature_flag(ct-multivariate) matches PostHog's variant hash for 12 ids", () => eq(r.variants, w.variants, "variants")),
    step("get_feature_flag_payload(ct-bool-on) returns the JSON payload", () => eq(r.payload_on, w.payload_on, "payload")),
    step("get_feature_flag_payload(ct-multivariate) returns the matched variant's payload", () => eq(r.payload_mv, w.payload_mv, "payload")),
    step("get_all_flags returns every active flag's value", () => {
      subset(r.all, w.all, "get_all_flags");
      ok(!r.all?.["ct-inactive"], "inactive flag is not on", r.all?.["ct-inactive"], "false or absent");
    }),
    step("get_all_flags_and_payloads returns values and payloads", () => {
      subset(r.all_and_payloads?.featureFlags, w.all, "featureFlags");
      // posthog-python hands these payloads back as the raw JSON strings.
      const payloads = Object.fromEntries(
        Object.entries(r.all_and_payloads?.featureFlagPayloads ?? {}).map(([k, v]) => [k, typeof v === "string" ? JSON.parse(v) : v])
      );
      subset(payloads, { "ct-bool-on": w.payload_on, "ct-multivariate": w.payload_mv }, "featureFlagPayloads");
    }),
  ];
  for (const [name, fn] of steps) await report.step(name, fn);
}

try {
  // ── capture, identify/alias, remote flags ──────────────────────────────
  const mainMark = proxy.mark();
  const main = await report.step("sdk: main scenario runs (capture, $identify, alias, set, set_once, flags, flush, shutdown)", () =>
    drive("main")
  );
  if (main) {
    report.meta.sdk["posthog-python"] = main.sdk_version;
    await report.step("capture: flush() completes and every /batch/ request is acknowledged 2xx exactly once", () => {
      eq(main.flush, null, "flush() result");
      const sent = proxy.since(mainMark).filter((r) => r.path.startsWith("/batch"));
      ok(sent.length >= 1, "batch requests sent", sent.length, ">= 1");
      eq(sent.map((r) => r.status), sent.map(() => 200), "batch response statuses");
      eq(repeatedRequests(sent), [], "retried batch payloads");
      const names = sent.flatMap((r) => eventsIn(decodeBody(r.body))).map((e) => e.event);
      for (const n of ["ct_signup_viewed", "ct_grouped", "$identify", "ct_anon2_event", "$create_alias", "$set", "ct_after_login"])
        ok(names.includes(n), `${n} on the wire`, names, n);
    });
    await flagSteps("flags", main);
    await report.step("flags: stored person property ($set plan=enterprise) enables ct-person-plan without person_properties", () =>
      eq(main.plan_from_stored_person, true, "feature_enabled")
    );
    await storedOutcomeSteps(report, api, projectId, { ids, uuids, sentTs: new Date(main.sent_ts) });
  } else {
    report.skip("python: flags and stored outcomes", "driver failed");
  }

  // ── local evaluation ───────────────────────────────────────────────────
  if (personalKey) {
    const loadMark = proxy.mark();
    let evalMark = null;
    const local = await report.step("local eval: scenario runs with personal_api_key", () =>
      drive("local", { onPhase: () => (evalMark = proxy.mark()) })
    );
    await report.step("local eval: GET /flags/definitions sent the personal key and got 200 with every flag", () => {
      const defs = proxy.since(loadMark).filter((r) => r.path.startsWith("/flags/definitions"));
      ok(defs.length >= 1, "definitions requests", defs.length, ">= 1");
      eq(defs[0].headers.authorization, `Bearer ${personalKey}`, "authorization header");
      eq(defs[0].status, 200, "definitions status");
      eq(local?.definition_keys, ["ct-bool-off", "ct-bool-on", "ct-inactive", "ct-multivariate", "ct-person-plan"], "definition keys loaded by the SDK");
    });
    if (local) await flagSteps("local eval", local);
    await report.step("local eval: no POST /flags or /decide was made while evaluating", () => {
      if (evalMark === null) fail("driver never reached evaluation", null, "evaluation phase");
      const remote = proxy.since(evalMark).filter((r) => r.method === "POST" && /^\/(flags|decide)\/?$/.test(r.path));
      eq(remote.map((r) => `${r.method} ${r.path}`), [], "remote flag requests during local evaluation");
    });
  } else report.skip("local eval: all steps", "no personal API key");

  // ── retry contract ─────────────────────────────────────────────────────
  const marked = (marker) => (r) => r.method === "POST" && r.path.startsWith("/batch") && JSON.stringify(decodeBody(r.body) ?? "").includes(marker);

  await report.step("retry: unknown project token → 4xx, SDK does not retry", async () => {
    const marker = randomUUID();
    const mark = proxy.mark();
    await drive("bad_token", { env: { CT_MARKER: marker, CT_UUID: randomUUID() } });
    const attempts = proxy.since(mark).filter(marked(marker));
    eq(attempts.map((r) => r.status >= 400 && r.status < 500), [true], "one attempt answered 4xx");
  });

  await report.step("retry: malformed body → 4xx, SDK does not retry", async () => {
    const marker = randomUUID();
    proxy.inject(marked(marker), { corrupt: true });
    const mark = proxy.mark();
    await drive("capture", { env: { CT_MARKER: marker, CT_UUID: randomUUID() } });
    const attempts = proxy.since(mark).filter(marked(marker));
    eq(attempts.map((r) => ({ corrupt: !!r.injected?.corrupt, is4xx: r.status >= 400 && r.status < 500 })), [{ corrupt: true, is4xx: true }], "attempts");
  });

  const marker = randomUUID();
  const uuid = randomUUID();
  const retried = await report.step("retry: 503 → SDK retries the identical payload until acknowledged", async () => {
    proxy.inject(marked(marker), { status: 503 });
    const mark = proxy.mark();
    await drive("capture", { env: { CT_MARKER: marker, CT_UUID: uuid } });
    const attempts = proxy.since(mark).filter(marked(marker));
    eq(attempts.map((r) => r.status), [503, 200], "attempt statuses");
    eq(eventUuids(attempts[1]), eventUuids(attempts[0]), "retried the same events");
    return true;
  });
  if (retried) {
    await report.step("retry: event retried after 503 is stored exactly once", async () => {
      const events = await waitForEvents(api, projectId, [uuid]);
      eq(events.filter((e) => e.uuid === uuid).length, 1, "stored copies");
    });
  } else report.skip("retry: event retried after 503 is stored exactly once", "503 retry did not complete");
} finally {
  await proxy.close();
  await hoglet.stop();
}

const code = report.finish();
if (code !== 0) report.info(`hoglet log tail:\n${hoglet.logTail(15)}`);
hoglet.destroy();
process.exit(code);
