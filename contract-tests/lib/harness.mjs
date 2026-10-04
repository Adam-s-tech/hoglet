// Shared harness for every SDK suite: a real Hoglet process on a free port
// with its own data dir, a counting/fault-injecting HTTP proxy in front of
// it, bootstrap through the real dashboard API, and the API reads used to
// assert stored outcomes.
//
// Nothing here talks to Hoglet internals or its files: setup and assertions
// go through the same HTTP surface a user and the dashboard use.

import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { closeSync, existsSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gunzipSync } from "node:zlib";

import { fail } from "./report.mjs";

const REPO = new URL("../../", import.meta.url).pathname;
export const HOGLET_BIN = process.env.HOGLET_BIN ?? join(REPO, "target/debug/hoglet");

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function freePort() {
  return await new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

/// Retries `fn` until it returns without throwing or the deadline passes,
/// then rethrows the last error. Errors marked `final` stop immediately
/// (a missing route will not appear by waiting).
export async function poll(fn, { timeoutMs = 15_000, intervalMs = 200 } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      return await fn();
    } catch (error) {
      if (error?.final || Date.now() >= deadline) throw error;
    }
    await sleep(intervalMs);
  }
}

// ── Hoglet process ───────────────────────────────────────────────────────

const cleanups = new Set();
let exiting = false;
function runCleanups() {
  for (const c of cleanups) {
    try {
      c();
    } catch {}
  }
  cleanups.clear();
}
process.on("exit", runCleanups);
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    if (exiting) return;
    exiting = true;
    runCleanups();
    process.exit(130);
  });
}

export class Hoglet {
  constructor({ label }) {
    this.label = label;
    this.dataDir = mkdtempSync(join(tmpdir(), `hoglet-contract-${label}-`));
    this.logPath = join(this.dataDir, "..", `${this.dataDir.split("/").pop()}.log`);
    this.child = null;
    this.port = null;
    this._cleanup = () => this.destroy();
    cleanups.add(this._cleanup);
  }

  get url() {
    return `http://127.0.0.1:${this.port}`;
  }

  async start() {
    if (!existsSync(HOGLET_BIN)) throw new Error(`hoglet binary not found at ${HOGLET_BIN} (run cargo build)`);
    this.port ??= await freePort();
    const log = openSync(this.logPath, "a");
    this.child = spawn(HOGLET_BIN, [], {
      env: { ...process.env, HOGLET_ADDR: `127.0.0.1:${this.port}`, HOGLET_DATA: this.dataDir, RUST_LOG: process.env.RUST_LOG ?? "hoglet=info" },
      stdio: ["ignore", log, log],
    });
    closeSync(log);
    const child = this.child;
    let exited = null;
    child.once("exit", (code, signal) => (exited = { code, signal }));
    const deadline = Date.now() + 30_000;
    for (;;) {
      if (exited) throw new Error(`hoglet exited during startup (${JSON.stringify(exited)})\n${this.logTail()}`);
      try {
        const res = await fetch(`${this.url}/ready`);
        if (res.status === 200) return;
      } catch {}
      if (Date.now() > deadline) throw new Error(`hoglet not ready within 30s\n${this.logTail()}`);
      await sleep(50);
    }
  }

  /// Graceful stop (SIGINT is the binary's shutdown signal); SIGKILL if it
  /// does not exit within the deadline.
  async stop({ timeoutMs = 10_000 } = {}) {
    const child = this.child;
    if (!child || child.exitCode !== null || child.signalCode !== null) return;
    const done = new Promise((r) => child.once("exit", r));
    child.kill("SIGINT");
    const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
    await done;
    clearTimeout(timer);
    this.child = null;
  }

  logTail(lines = 30) {
    try {
      return readFileSync(this.logPath, "utf8").trim().split("\n").slice(-lines).join("\n");
    } catch {
      return "(no log)";
    }
  }

  destroy() {
    if (this.child && this.child.exitCode === null) this.child.kill("SIGKILL");
    if (!process.env.CONTRACT_KEEP_DATA) {
      rmSync(this.dataDir, { recursive: true, force: true });
      rmSync(this.logPath, { force: true });
    }
    cleanups.delete(this._cleanup);
  }
}

// ── Counting proxy with fault injection ──────────────────────────────────

/// Best-effort decode of a PostHog request body: gzip (sniffed), form
/// `data=` (base64 or plain), base64, raw JSON.
export function decodeBody(buf) {
  if (!buf || buf.length === 0) return null;
  let bytes = buf;
  try {
    if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = gunzipSync(bytes);
  } catch {
    return null;
  }
  let text = bytes.toString("utf8");
  if (/^data=/.test(text)) {
    const v = decodeURIComponent(new URLSearchParams(text).get("data") ?? "");
    text = v;
  }
  for (const candidate of [text, () => Buffer.from(text, "base64").toString("utf8")]) {
    try {
      const s = typeof candidate === "function" ? candidate() : candidate;
      return JSON.parse(s);
    } catch {}
  }
  return null;
}

/// The events carried by a capture request, whatever the envelope.
export function eventsIn(body) {
  if (!body) return [];
  if (Array.isArray(body)) return body;
  if (Array.isArray(body.batch)) return body.batch;
  if (typeof body.event === "string") return [body];
  return [];
}

const CAPTURE_PATHS = /^\/(e|capture|batch|track|engage|i\/v0\/e)\/?$/;
export const isCapturePath = (p) => CAPTURE_PATHS.test(p);

export class Proxy {
  constructor(upstream) {
    this.upstream = upstream; // { port }
    this.log = [];
    this.rules = [];
    this.server = http.createServer((req, res) => this._handle(req, res));
    this.server.keepAliveTimeout = 1000;
  }

  async listen() {
    this.port = await new Promise((resolve) =>
      this.server.listen(0, "127.0.0.1", () => resolve(this.server.address().port))
    );
    cleanups.add(() => this.server.close());
    return this;
  }

  get url() {
    return `http://127.0.0.1:${this.port}`;
  }

  /// Index into the log; pair with `since(mark)`.
  mark() {
    return this.log.length;
  }

  since(mark) {
    return this.log.slice(mark);
  }

  /// `match(rec) → bool`; `action`: `{ status }` answers without forwarding,
  /// `{ corrupt: true }` forwards a garbage body. Consumed after `times`.
  inject(match, action, times = 1) {
    this.rules.push({ match, action, remaining: times });
  }

  async close() {
    this.server.closeAllConnections?.();
    await new Promise((r) => this.server.close(r));
  }

  _handle(req, res) {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      const body = Buffer.concat(chunks);
      const url = new URL(req.url, "http://proxy");
      const rec = {
        seq: this.log.length,
        t: Date.now(),
        method: req.method,
        path: url.pathname,
        query: Object.fromEntries(url.searchParams),
        headers: req.headers,
        body,
        bodyHash: createHash("sha1").update(body).digest("hex"),
        status: null,
        injected: null,
      };
      this.log.push(rec);
      const rule = this.rules.find((r) => r.remaining > 0 && r.match(rec));
      if (rule) {
        rule.remaining -= 1;
        rec.injected = rule.action;
        if (rule.action.status) {
          rec.status = rule.action.status;
          res.writeHead(rule.action.status, {
            "content-type": "application/json",
            "access-control-allow-origin": req.headers.origin ?? "*",
            "access-control-allow-credentials": "true",
          });
          res.end(JSON.stringify({ error: "injected by contract-test proxy" }));
          return;
        }
      }
      let outBody = body;
      if (rule?.action.corrupt) outBody = Buffer.from([0x1f, 0x8b, 0x08, 0x00, 0xde, 0xad, 0xbe, 0xef, 0x00, 0x01, 0x02]);
      const headers = { ...req.headers, host: `127.0.0.1:${this.upstream.port}` };
      if (outBody !== body) headers["content-length"] = String(outBody.length);
      const up = http.request(
        { host: "127.0.0.1", port: this.upstream.port, method: req.method, path: req.url, headers },
        (upRes) => {
          rec.status = upRes.statusCode;
          const respChunks = [];
          upRes.on("data", (c) => respChunks.push(c));
          upRes.on("end", () => {
            rec.response = Buffer.concat(respChunks);
          });
          res.writeHead(upRes.statusCode, upRes.headers);
          upRes.pipe(res);
        }
      );
      up.on("error", (error) => {
        // Upstream down: surface it to the client as a network error, the
        // same thing it would see without the proxy.
        rec.status = error.code ?? "network_error";
        req.socket.destroy();
      });
      up.end(outBody);
    });
  }
}

/// Sorted uuids of the events a capture request carried.
export function eventUuids(rec) {
  return eventsIn(decodeBody(rec.body))
    .map((e) => e.uuid)
    .filter(Boolean)
    .sort();
}

/// Requests sent more than once — i.e. an SDK retry. Capture requests are
/// keyed by the event uuids they carry (SDKs may re-stamp `sent_at` on a
/// retry); everything else by method, path and exact body.
export function repeatedRequests(records) {
  const seen = new Map();
  for (const r of records) {
    if (r.method === "OPTIONS" || r.method === "GET" || r.body.length === 0) continue;
    const uuids = isCapturePath(r.path) ? eventUuids(r) : [];
    const key = uuids.length ? `${r.method} ${r.path} events ${uuids.join(",")}` : `${r.method} ${r.path} body ${r.bodyHash}`;
    seen.set(key, (seen.get(key) ?? 0) + 1);
  }
  return [...seen].filter(([, n]) => n > 1).map(([k, n]) => `${k.slice(0, 120)} ×${n}`);
}

// ── Dashboard API client ─────────────────────────────────────────────────

export class HttpError extends Error {
  constructor(method, path, status, text) {
    super(`${method} ${path} → HTTP ${status} ${JSON.stringify(text.slice(0, 200))}`);
    this.status = status;
    this.text = text;
    // Collection routes answering 404/405 are absent, not "not yet".
    this.final = [400, 401, 403, 404, 405, 415, 422].includes(status);
  }
}

export class Api {
  constructor(baseUrl) {
    this.baseUrl = baseUrl;
    this.cookie = null;
    this.bearer = null;
  }

  async request(method, path, body, { expect = [200, 201] } = {}) {
    const headers = { accept: "application/json" };
    if (body !== undefined) headers["content-type"] = "application/json";
    if (this.cookie) headers.cookie = this.cookie;
    else if (this.bearer) headers.authorization = `Bearer ${this.bearer}`;
    const res = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await res.text();
    if (!expect.includes(res.status)) throw new HttpError(method, path, res.status, text);
    const setCookie = res.headers.get("set-cookie");
    let json = null;
    try {
      json = text ? JSON.parse(text) : null;
    } catch {
      throw new HttpError(method, path, res.status, `non-JSON body: ${text}`);
    }
    return { status: res.status, json, setCookie };
  }

  get(path, opts) {
    return this.request("GET", path, undefined, opts);
  }
  post(path, body, opts) {
    return this.request("POST", path, body, opts);
  }
}

/// First-run setup through the real API. Returns the session-authenticated
/// API client plus the project id and capture token.
export async function setupWorkspace(baseUrl, label) {
  const api = new Api(baseUrl);
  const { json, setCookie } = await api.post("/api/auth/setup", {
    email: `${label}@contract.hoglet.test`,
    password: "contract-test-password",
    organization_name: `Contract ${label}`,
    project_name: `contract-${label}`,
  });
  const sid = setCookie?.match(/hoglet_sid=[^;]+/)?.[0];
  if (!sid) fail("setup response has no hoglet_sid session cookie", setCookie, "hoglet_sid=…");
  api.cookie = sid;
  const project = json?.organizations?.[0]?.projects?.[0];
  if (!project?.id || !project?.token) fail("setup response has no project id/token", json, "organizations[0].projects[0].{id,token}");
  return { api, projectId: project.id, token: project.token };
}

export async function createPersonalKey(api) {
  const { json } = await api.post("/api/auth/keys", { name: "contract-tests" });
  if (typeof json?.secret !== "string" || !json.secret.startsWith("phx_")) fail("personal key secret", json, "{ secret: 'phx_…' }");
  return json.secret;
}

// ── Flags under test ─────────────────────────────────────────────────────

export const FLAG_PAYLOAD_ON = { tier: "gold", limits: [1, 2, 3] };
export const VARIANT_PAYLOADS = { control: { color: "blue" }, test: { color: "red", beta: true } };

/// `FeatureFlagInput` bodies (src/contract/flags.rs).
export const FLAG_INPUTS = [
  {
    key: "ct-bool-on",
    name: "boolean, 100% rollout, with payload",
    active: true,
    filters: { groups: [{ properties: [], rollout_percentage: 100 }], payloads: { true: FLAG_PAYLOAD_ON } },
  },
  {
    key: "ct-bool-off",
    name: "boolean, 0% rollout",
    active: true,
    filters: { groups: [{ properties: [], rollout_percentage: 0 }] },
  },
  {
    key: "ct-multivariate",
    name: "multivariate 50/50 with payloads",
    active: true,
    filters: {
      groups: [{ properties: [], rollout_percentage: 100 }],
      multivariate: {
        variants: [
          { key: "control", rollout_percentage: 50 },
          { key: "test", rollout_percentage: 50 },
        ],
      },
      payloads: VARIANT_PAYLOADS,
    },
  },
  {
    key: "ct-person-plan",
    name: "on for person property plan = enterprise",
    active: true,
    filters: {
      groups: [
        {
          properties: [{ key: "plan", type: "person", operator: "exact", value: ["enterprise"] }],
          rollout_percentage: 100,
        },
      ],
    },
  },
  {
    key: "ct-inactive",
    name: "inactive flag",
    active: false,
    filters: { groups: [{ properties: [], rollout_percentage: 100 }] },
  },
];

// PostHog's bucketing (posthog/models/feature_flag/flag_matching.py),
// reimplemented as an independent oracle so a server agreeing with itself
// cannot pass.
const LONG_SCALE = 0xfffffffffffffff;
function phHash(key, distinctId, salt = "") {
  const hex = createHash("sha1").update(`${key}.${distinctId}${salt}`).digest("hex").slice(0, 15);
  return parseInt(hex, 16) / LONG_SCALE;
}

export function expectedVariant(flagKey, distinctId) {
  const flag = FLAG_INPUTS.find((f) => f.key === flagKey);
  const h = phHash(flagKey, distinctId, "variant");
  let min = 0;
  for (const v of flag.filters.multivariate.variants) {
    const max = min + v.rollout_percentage / 100;
    if (h >= min && h < max) return v.key;
    min = max;
  }
  return null;
}

/// Creates every flag through `POST /api/projects/{id}/feature_flags`.
/// Each creation is its own reported step.
export async function createFlags(report, api, projectId) {
  const created = {};
  for (const input of FLAG_INPUTS) {
    created[input.key] = await report.step(
      `setup: POST /api/projects/{id}/feature_flags creates ${input.key}`,
      async () => {
        const { json } = await api.post(`/api/projects/${projectId}/feature_flags`, input);
        if (json?.key !== input.key) fail("created flag key", json, { key: input.key });
        return json;
      }
    );
  }
  return created;
}

// ── Stored-outcome reads ─────────────────────────────────────────────────

export async function listEvents(api, projectId, params = {}) {
  const all = [];
  let before = null;
  for (let page = 0; page < 20; page++) {
    const q = new URLSearchParams({ limit: "200", ...params });
    if (before) q.set("before", before);
    const { json } = await api.get(`/api/projects/${projectId}/events?${q}`);
    if (!Array.isArray(json?.events)) fail("events response shape", json, "EventListResponse { events: [...] }");
    all.push(...json.events);
    if (!json.next_before || json.events.length === 0) break;
    before = json.next_before;
  }
  return all;
}

/// Polls the events feed until every uuid is present.
export async function waitForEvents(api, projectId, uuids, opts) {
  return await poll(async () => {
    const events = await listEvents(api, projectId);
    const missing = uuids.filter((u) => !events.some((e) => e.uuid === u));
    if (missing.length) fail(`events not stored yet`, { stored: events.length, missing }, { missing: [] });
    return events;
  }, opts);
}

export async function listPersons(api, projectId) {
  const all = [];
  let cursor = null;
  for (let page = 0; page < 20; page++) {
    const q = new URLSearchParams({ limit: "100" });
    if (cursor) q.set("cursor", cursor);
    const { json } = await api.get(`/api/projects/${projectId}/persons?${q}`);
    if (!Array.isArray(json?.persons)) fail("persons response shape", json, "PersonListResponse { persons: [...] }");
    all.push(...json.persons);
    if (!json.next_cursor) break;
    cursor = json.next_cursor;
  }
  return all;
}

/// The person-graph assertion: exactly one person owns all `ids`.
/// Returns that person's detail.
export async function waitForSinglePerson(api, projectId, ids, opts) {
  return await poll(async () => {
    const persons = await listPersons(api, projectId);
    const owners = persons.filter((p) => ids.some((id) => p.distinct_ids?.includes(id)));
    if (owners.length !== 1) {
      fail(
        "persons owning the ids",
        owners.map((p) => ({ id: p.id, distinct_ids: p.distinct_ids })),
        `exactly 1 person owning ${JSON.stringify(ids)}`
      );
    }
    const { json: detail } = await api.get(`/api/projects/${projectId}/persons/${owners[0].id}`);
    const missing = ids.filter((id) => !detail?.distinct_ids?.includes(id));
    if (missing.length) fail("person detail distinct_ids", detail?.distinct_ids, `superset of ${JSON.stringify(ids)}`);
    return detail;
  }, opts);
}

export async function trends(api, projectId, series, dateFrom = "-7d") {
  const { json } = await api.post(`/api/projects/${projectId}/query`, {
    query: { kind: "TrendsQuery", series, date_range: { date_from: dateFrom }, interval: "day" },
    refresh: true,
  });
  if (json?.result?.kind !== "Trends" || !Array.isArray(json.result.series)) {
    fail("query response shape", json, "QueryResponse { result: { kind: 'Trends', series: [...] } }");
  }
  return json.result.series;
}

/// DAU over all events must be exactly one person in every non-empty
/// bucket — identify merged the ids, so they count once.
export async function assertDauIsOne(api, projectId, opts) {
  return await poll(async () => {
    const series = await trends(api, projectId, [{ event: null, math: "dau" }]);
    const data = series[0]?.data ?? [];
    const max = Math.max(0, ...data);
    if (max !== 1) fail("max daily unique persons", { data, days: series[0]?.days }, "max(data) === 1");
    return data;
  }, opts);
}

export function runId() {
  return randomUUID().slice(0, 8);
}
