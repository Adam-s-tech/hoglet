// Browser contract test: unmodified posthog-js at latest, loaded in a real
// page under Playwright, api_host pointed at a real Hoglet (claims.md
// claim 1). Run via `npm run test:browser` after `npx playwright install
// chromium`.

import { test, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { mkdtempSync, readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = 18898;
const TOKEN = "phc_browser_contract";
const BIN =
  process.env.HOGLET_BIN ?? new URL("../target/debug/hoglet", import.meta.url).pathname;

let hoglet;
let dataDir;

test.beforeAll(async () => {
  dataDir = mkdtempSync(join(tmpdir(), "hoglet-browser-"));
  hoglet = spawn(BIN, {
    env: { ...process.env, HOGLET_ADDR: `127.0.0.1:${PORT}`, HOGLET_DATA: dataDir },
    stdio: "ignore",
  });
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try {
      // Bootstrap is the unauthenticated readiness probe; config is
      // token-gated and 401s until a project exists.
      const res = await fetch(`http://127.0.0.1:${PORT}/api/auth/bootstrap`);
      up = res.ok;
    } catch {}
    if (!up) await new Promise((r) => setTimeout(r, 100));
  }
  if (!up) throw new Error("hoglet did not start");

  // First-run provisioning: capture is fail-closed, so create the workspace
  // whose project owns TOKEN — the same flow a real user does.
  const setupRes = await fetch(`http://127.0.0.1:${PORT}/api/auth/setup`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      email: "browser-contract@hoglet.test",
      password: "contract-test-password",
      organization_name: "Contract",
      existing_project_token: TOKEN,
    }),
  });
  if (!setupRes.ok) throw new Error(`first-run setup returned ${setupRes.status}`);
});

test.afterAll(() => hoglet?.kill("SIGKILL"));

// Chrome's Local Network Access rules gate requests to loopback addresses
// behind a permission (headless denies by default). Real deployments target a
// routable analytics host, so granting it here is test-rig plumbing, not a
// weakened claim.
test.use({ permissions: ["local-network-access"] });

test("posthog-js initializes, captures, and identifies against hoglet", async ({ page }) => {
  const failed = [];
  page.on("requestfailed", (req) => {
    const url = req.url();
    if (!url.includes(`127.0.0.1:${PORT}`)) return;
    // posthog-js fetches optional extension bundles (e.g. /static/surveys.js)
    // even when the remote config disables the feature, and degrades
    // gracefully when they 404 (posthog-surveys.ts _handleSurveyLoadError).
    // Surveys are explicitly out of scope; only wire endpoints must not fail.
    if (new URL(url).pathname.startsWith("/static/")) return;
    failed.push(url);
  });

  // Serve the host page on a real http origin (about:blank has no cookie
  // access). `localhost` vs the SDK's `127.0.0.1` api_host is cross-origin,
  // so Hoglet's CORS on the wire endpoints is exercised — while staying
  // loopback→loopback, which Chrome's Local Network Access rules permit
  // (a public test origin targeting 127.0.0.1 is blocked outright).
  await page.route("http://localhost:4173/", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<html><body><h1>contract</h1></body></html>`,
    })
  );
  await page.goto("http://localhost:4173/");
  await page.addScriptTag({ url: "https://unpkg.com/posthog-js@latest/dist/array.js" });
  await page.evaluate(
    ([token, port]) => {
      window.posthog.init(token, {
        api_host: `http://127.0.0.1:${port}`,
        loaded: () => (window.__loaded = true),
        // posthog-js bot detection sees navigator.webdriver=true under
        // Playwright and silently drops every event; this is the SDK's own
        // escape hatch for automated environments. Bot filtering is
        // client-side — the wire contract under test is unaffected.
        opt_out_useragent_filter: true,
      });
      window.posthog.capture("browser_pageview", { page: "/contract" });
      window.posthog.identify("browser-user@contract.test");
    },
    [TOKEN, PORT]
  );

  await page.waitForFunction(() => window.__loaded === true, null, { timeout: 10_000 });
  // Let the SDK's batching flush.
  await page.waitForTimeout(3_000);

  expect(failed, `SDK requests failed: ${failed.join(", ")}`).toHaveLength(0);

  // Events must be on disk, not merely 200'd. v2 layout: WAL segments in
  // wal-v2/ (`.open` active, `.wal` sealed); Parquet partitioned under
  // events/project=<id>/date=<d>/.
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
  expect(walBytes + countParquet(join(dataDir, "events"))).toBeGreaterThan(0);
});
