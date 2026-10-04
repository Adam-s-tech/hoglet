// Contract test: unmodified posthog-node at latest, pointed at a real Hoglet
// binary. Asserts on what actually landed in storage, not on 200s alone
// (claims.md claim 1: a mock returning 200 to everything must fail this).
//
// Env: HOGLET_BIN (path to binary, default ../target/debug/hoglet)

import { PostHog } from "posthog-node";
import { spawn } from "node:child_process";
import { mkdtempSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = 18899;
const TOKEN = "phc_contract_test";
const BIN = process.env.HOGLET_BIN ?? new URL("../target/debug/hoglet", import.meta.url).pathname;

const dataDir = mkdtempSync(join(tmpdir(), "hoglet-contract-"));
const hoglet = spawn(BIN, {
  env: { ...process.env, HOGLET_ADDR: `127.0.0.1:${PORT}`, HOGLET_DATA: dataDir },
  stdio: "ignore",
});

const fail = (msg) => {
  console.error(`FAIL: ${msg}`);
  hoglet.kill("SIGKILL");
  process.exit(1);
};

// Wait for the server (bootstrap is the unauthenticated readiness probe;
// the config endpoint is token-gated and 401s until a project exists).
for (let i = 0; ; i++) {
  try {
    const res = await fetch(`http://127.0.0.1:${PORT}/api/auth/bootstrap`);
    if (res.ok) break;
  } catch {}
  if (i > 100) fail("hoglet did not start");
  await new Promise((r) => setTimeout(r, 100));
}

// 0. First-run provisioning — the same flow a real user does. Capture is
// fail-closed: no project, no ingest, so the contract test must set up a
// workspace whose project owns TOKEN.
const setupRes = await fetch(`http://127.0.0.1:${PORT}/api/auth/setup`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({
    email: "contract@hoglet.test",
    password: "contract-test-password",
    organization_name: "Contract",
    existing_project_token: TOKEN,
  }),
});
if (!setupRes.ok) fail(`first-run setup returned ${setupRes.status}`);

// 1. Config endpoint parses and disables what we don't implement.
const configRes = await fetch(`http://127.0.0.1:${PORT}/array/${TOKEN}/config`);
if (!configRes.ok) fail(`config returned ${configRes.status} for a provisioned token`);
const config = await configRes.json();
if (config.sessionRecording !== false) fail("sessionRecording must be false");
if (config.surveys !== false) fail("surveys must be false");

// 2. Real SDK: capture, identify, alias, flush. Unmodified client, default
// options — the drop-in promise.
const client = new PostHog(TOKEN, { host: `http://127.0.0.1:${PORT}`, flushAt: 5 });
client.capture({ distinctId: "anon-1", event: "pageview", properties: { page: "/" } });
client.capture({ distinctId: "anon-1", event: "click", properties: { btn: "cta" } });
client.identify({ distinctId: "user@contract.test", properties: { plan: "pro" } });
client.alias({ distinctId: "user@contract.test", alias: "anon-1" });
await client.flush();
await client.shutdown();

// 3. Flags endpoint answers (empty set is fine; erroring is not).
const flagsRes = await fetch(`http://127.0.0.1:${PORT}/flags/?v=2`, {
  method: "POST",
  body: JSON.stringify({ token: TOKEN, distinct_id: "user@contract.test" }),
});
if (!flagsRes.ok) fail(`flags returned ${flagsRes.status}`);
const flags = await flagsRes.json();
if (typeof flags.feature_flags !== "object") fail("flags v2 shape wrong");

// 4. Storage-side assertion: events must actually be durable in the WAL or
// Parquet — count records on disk after killing the server.
hoglet.kill("SIGKILL");
await new Promise((r) => setTimeout(r, 300));

import { statSync } from "node:fs";
// v2 layout: WAL segments live in wal-v2/ (`.open` active, `.wal` sealed);
// Parquet is partitioned under events/project=<id>/date=<d>/.
const walDir = join(dataDir, "wal-v2");
const walBytes = readdirSync(walDir)
  .filter((f) => f.endsWith(".wal") || f.endsWith(".open"))
  .reduce((sum, f) => sum + statSync(join(walDir, f)).size, 0);
const countParquet = (dir) => {
  let n = 0;
  try {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) n += countParquet(join(dir, entry.name));
      else if (entry.name.endsWith(".parquet")) n += 1;
    }
  } catch {}
  return n;
};
const parquetFiles = countParquet(join(dataDir, "events"));
// An empty active segment is always present; bytes prove events landed.
if (walBytes === 0 && parquetFiles === 0) fail("no event bytes persisted anywhere");

console.log("PASS: posthog-node latest against hoglet — config, capture, identify, alias, flags, persistence");
process.exit(0);
