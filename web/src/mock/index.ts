// In-process mock of the Hoglet API (npm run dev). Every endpoint in the
// contract answers with generated, realistic data and plausible latency.
//
// URL switches: ?mock=fresh starts at first-run setup with no events;
// ?mock=logout starts signed out; ?mock=member / ?mock=admin sign in with that
// role (read-only UI, team management without owners). /invite/hgi_demo opens a
// sample invite.

import type { ActorsRequest } from "../types/ActorsRequest";
import type { CatalogEvent } from "../types/CatalogEvent";
import type { EventRow } from "../types/EventRow";
import type { FeatureFlag } from "../types/FeatureFlag";
import type { FeatureFlagInput } from "../types/FeatureFlagInput";
import type { InsightQuery } from "../types/InsightQuery";
import type { Invite } from "../types/Invite";
import type { Member } from "../types/Member";
import type { Role } from "../types/Role";
import type { PropertyFilter } from "../types/PropertyFilter";
import type { QueryRequest } from "../types/QueryRequest";
import type { WebDimension } from "../types/WebDimension";
import type { ForwardingConfig, RawRequest, RawResponse, Transport } from "../lib/api";
import { autoInterval } from "../lib/format";
import { EVENTS, EVENT_PROPS, PERSON_PROPS, VALUES, buckets, funnels, hash, lifecycle, makeEvent, makePersons, paths, retention, rng, sql, stickiness, trends, type MockPerson } from "./data";

const params = new URLSearchParams(window.location.search);
const FRESH = params.get("mock") === "fresh";

const PROJECTS = [
  { id: "0190f3a2-7c1e-7d4a-9b11-3f2a8c6d5e01", name: "Acme Web", token: "phc_acme_7Hq2xV9kLmR4tW8yZ3nB6cD1fG5jP0sA" },
  { id: "0190f3a2-7c1e-7d4a-9b11-3f2a8c6d5e02", name: "Acme Mobile", token: "phc_mobile_2Lk8Jd7Hs6Gf5Ds4Aq3Wz1Xc9Vb0Nm" },
];

interface State {
  setupDone: boolean;
  role: Role;
  members: Member[];
  invites: (Invite & { token: string })[];
  session: boolean;
  firstEventAt: number;
  user: { id: string; email: string; name: string };
  orgName: string;
  projects: typeof PROJECTS;
  persons: MockPerson[];
  events: EventRow[];
  lastTick: number;
  flags: FeatureFlag[];
  insights: Record<string, unknown>[];
  dashboards: { id: string; project_id: string; name: string; tiles: { insight_id: string; x: number; y: number; w: number; h: number }[]; created_by: string; created_at: number }[];
  shares: Record<string, unknown>[];
  keys: { id: string; name: string; scope: "read" | "write"; key_prefix: string; last_used: number | null; created_at: number }[];
  forwarding: { config: ForwardingConfig | null; forwarded: number; dropped: number; failed: number; queued: number; last_error: string | null };
  nextId: number;
}

const now = () => Date.now();
const sec = () => Math.floor(Date.now() / 1000);

function seedState(): State {
  const persons = makePersons(340);
  const r = rng("events");
  const events: EventRow[] = [];
  const t0 = now();
  for (let i = 0; i < 500; i++) {
    const ts = t0 - Math.floor((i + r()) * 14_000);
    events.push(makeEvent(r, persons[Math.floor(Math.pow(r(), 1.6) * persons.length)], ts));
  }
  const pid = PROJECTS[0].id;
  const iso = (d: number) => new Date(t0 - d * 86_400_000).toISOString();
  const q = (query: InsightQuery) => query;
  const insights = [
    {
      id: "ins_pageviews_browser",
      name: "Pageviews by browser",
      query_ir: q({ kind: "TrendsQuery", series: [{ event: "$pageview", custom_name: null, properties: [], math: "total", math_property: null }], date_range: { date_from: "-14d", date_to: null }, interval: "day", properties: [], breakdown: { property: "$browser", type: "event", limit: 5 }, formula: null, compare: false, display: "ActionsAreaGraph" }),
    },
    {
      id: "ins_signup_funnel",
      name: "Signup → first project → purchase",
      query_ir: q({ kind: "FunnelsQuery", series: ["$pageview", "signed_up", "project_created", "purchase_completed"].map((e) => ({ event: e, custom_name: null, properties: [], math: "total" as const, math_property: null })), date_range: { date_from: "-30d", date_to: null }, properties: [], breakdown: null, funnel_window: { interval: 14, unit: "day" }, funnel_order: "ordered", exclusions: [] }),
    },
    {
      id: "ins_weekly_retention",
      name: "Weekly retention",
      query_ir: q({ kind: "RetentionQuery", target: { event: "signed_up", custom_name: null, properties: [], math: "total", math_property: null }, returning: { event: "$pageview", custom_name: null, properties: [], math: "total", math_property: null }, period: "week", total_intervals: 8, retention_type: "retention_first_time", properties: [] }),
    },
    {
      id: "ins_dau",
      name: "Daily active users",
      query_ir: q({ kind: "TrendsQuery", series: [{ event: null, custom_name: "Active users", properties: [], math: "dau", math_property: null }], date_range: { date_from: "-30d", date_to: null }, interval: "day", properties: [], breakdown: null, formula: null, compare: true, display: "ActionsLineGraph" }),
    },
    {
      id: "ins_ai_cost",
      name: "AI spend by model",
      query_ir: q({ kind: "TrendsQuery", series: [{ event: "$ai_generation", custom_name: "AI cost (USD)", properties: [], math: "sum", math_property: "$ai_total_cost_usd" }], date_range: { date_from: "-30d", date_to: null }, interval: "day", properties: [], breakdown: { property: "$ai_model", type: "event", limit: 5 }, formula: null, compare: false, display: "ActionsPie" }),
    },
    {
      id: "ins_lifecycle",
      name: "User lifecycle",
      query_ir: q({ kind: "LifecycleQuery", series: { event: "$pageview", custom_name: null, properties: [], math: "total", math_property: null }, date_range: { date_from: "-30d", date_to: null }, interval: "day", properties: [] }),
    },
    {
      id: "ins_signups_number",
      name: "Signups this month",
      query_ir: q({ kind: "TrendsQuery", series: [{ event: "signed_up", custom_name: "Signups", properties: [], math: "total", math_property: null }], date_range: { date_from: "-30d", date_to: null }, interval: "day", properties: [], breakdown: null, formula: null, compare: true, display: "BoldNumber" }),
    },
  ].map((x, i) => ({ ...x, project_id: pid, description: "", created_by: "u1", created_at: sec() - (i + 2) * 86_400 * 3, updated_at: sec() - (i + 1) * 3600 * 7 }));

  const flagBase = (id: number, key: string, name: string, active: boolean, filters: FeatureFlag["filters"], days: number): FeatureFlag => ({
    id,
    key,
    name,
    active,
    filters,
    ensure_experience_continuity: false,
    created_at: iso(days + 10),
    updated_at: iso(days),
  });
  const flags = [
    flagBase(1, "new-onboarding", "Redesigned onboarding checklist", true, { groups: [{ properties: [], rollout_percentage: 50, variant: null }], multivariate: null, payloads: {} }, 2),
    flagBase(
      2,
      "pricing-v2",
      "Pricing page experiment",
      true,
      { groups: [{ properties: [{ key: "$geoip_country_code", type: "person", operator: "exact", value: ["US", "CA"] }], rollout_percentage: 100, variant: null }], multivariate: { variants: [{ key: "control", name: "Current", rollout_percentage: 50 }, { key: "annual-first", name: "Annual first", rollout_percentage: 50 }] }, payloads: { "annual-first": { discount: 20 } } },
      5,
    ),
    flagBase(3, "ai-summaries", "LLM-written insight summaries", true, { groups: [{ properties: [{ key: "plan", type: "person", operator: "exact", value: ["pro", "team", "enterprise"] }], rollout_percentage: 25, variant: null }], multivariate: null, payloads: { true: { model: "claude-haiku-4-5" } } }, 1),
    flagBase(4, "beta-sql-editor", "SQL editor for beta users", false, { groups: [{ properties: [{ key: "is_beta", type: "person", operator: "exact", value: [true] }], rollout_percentage: 100, variant: null }], multivariate: null, payloads: {} }, 12),
    flagBase(5, "kill-switch-exports", "Emergency off switch for CSV exports", true, { groups: [{ properties: [], rollout_percentage: 100, variant: null }], multivariate: null, payloads: {} }, 40),
  ];

  const roleParam = params.get("mock");
  const role: Role = roleParam === "member" || roleParam === "admin" ? roleParam : "owner";
  const joined = sec() - 86_400 * 30;
  return {
    setupDone: !FRESH,
    role,
    members: FRESH
      ? []
      : [
          { user_id: "u0", name: "Priya", email: "priya@acme.example", role: "owner", joined_at: joined - 86_400 * 20 },
          { user_id: "u1", name: "Dana", email: "demo@hoglet.dev", role: role === "owner" ? "owner" : role, joined_at: joined },
          { user_id: "u2", name: "Sam", email: "sam@acme.example", role: "admin", joined_at: joined + 86_400 * 4 },
          { user_id: "u3", name: "Lee", email: "lee@acme.example", role: "member", joined_at: joined + 86_400 * 9 },
        ],
    invites: FRESH ? [] : [{ id: "inv_1", email: "kim@acme.example", role: "member", created_by: "u0", created_at: sec() - 86_400, expires_at: sec() + 86_400 * 6, token: "hgi_demo" }],
    session: !FRESH && params.get("mock") !== "logout",
    firstEventAt: FRESH ? Infinity : 0,
    user: { id: "u1", email: "demo@hoglet.dev", name: "Dana" },
    orgName: "Acme",
    projects: FRESH ? [] : PROJECTS,
    persons,
    events,
    lastTick: t0,
    flags,
    insights: FRESH ? [] : insights,
    dashboards: FRESH
      ? []
      : [
          {
            id: "dash_product",
            project_id: pid,
            name: "Product health",
            tiles: [
              { insight_id: "ins_signups_number", x: 0, y: 0, w: 4, h: 3 },
              { insight_id: "ins_dau", x: 4, y: 0, w: 8, h: 3 },
              { insight_id: "ins_signup_funnel", x: 0, y: 3, w: 6, h: 4 },
              { insight_id: "ins_weekly_retention", x: 6, y: 3, w: 6, h: 4 },
              { insight_id: "ins_pageviews_browser", x: 0, y: 7, w: 6, h: 3 },
              { insight_id: "ins_ai_cost", x: 6, y: 7, w: 6, h: 3 },
            ],
            created_by: "u1",
            created_at: sec() - 86_400 * 9,
          },
          { id: "dash_growth", project_id: pid, name: "Growth weekly", tiles: [{ insight_id: "ins_lifecycle", x: 0, y: 0, w: 12, h: 4 }], created_by: "u1", created_at: sec() - 86_400 * 3 },
        ],
    shares: [],
    keys: FRESH ? [] : [{ id: "key_1", name: "Nightly export", scope: "read" as const, key_prefix: "phx_9f2c", last_used: sec() - 3600 * 5, created_at: sec() - 86_400 * 20 }],
    forwarding: { config: null, forwarded: 0, dropped: 0, failed: 0, queued: 0, last_error: null },
    nextId: 100,
  };
}

const state = seedState();

// ── Helpers ──────────────────────────────────────────────────────────────

function ok(body: unknown, status = 200): RawResponse {
  return { status, body: structuredClone(body) };
}
function err(status: number, code: string, message: string, field?: string): RawResponse {
  return { status, body: { error: { code, message, request_id: `mock-${Math.random().toString(16).slice(2, 10)}`, ...(field ? { field } : {}) } } };
}
function wait(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const t = window.setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => {
      window.clearTimeout(t);
      reject(new DOMException("Aborted", "AbortError"));
    });
  });
}
function workspace() {
  return {
    user: state.user,
    organizations: [{ id: "org_1", name: state.orgName, role: state.role, projects: state.projects }],
  };
}
function hasEvents(): boolean {
  return now() >= state.firstEventAt;
}

/** Advance the live event stream to now. */
function tick(): void {
  const t = now();
  const elapsed = t - state.lastTick;
  if (elapsed < 400) return;
  const r = rng(t);
  const n = Math.min(40, Math.floor((elapsed / 1000) * (0.6 + r() * 0.6)));
  for (let i = 0; i < n; i++) {
    const p = state.persons[Math.floor(Math.pow(r(), 1.8) * state.persons.length)];
    state.events.unshift(makeEvent(r, p, state.lastTick + Math.floor(((i + 1) / (n + 1)) * elapsed)));
  }
  state.lastTick = t;
  if (state.events.length > 3000) state.events.length = 3000;
}

function matches(props: Record<string, unknown>, f: PropertyFilter): boolean {
  const v = props[f.key];
  const vals = Array.isArray(f.value) ? f.value.map(String) : f.value === null ? [] : [String(f.value)];
  switch (f.operator) {
    case "exact":
      return vals.includes(String(v));
    case "is_not":
      return !vals.includes(String(v));
    case "icontains":
      return String(v ?? "").toLowerCase().includes((vals[0] ?? "").toLowerCase());
    case "not_icontains":
      return !String(v ?? "").toLowerCase().includes((vals[0] ?? "").toLowerCase());
    case "is_set":
      return v !== undefined && v !== null;
    case "is_not_set":
      return v === undefined || v === null;
    case "gt":
      return Number(v) > Number(vals[0]);
    case "gte":
      return Number(v) >= Number(vals[0]);
    case "lt":
      return Number(v) < Number(vals[0]);
    case "lte":
      return Number(v) <= Number(vals[0]);
    default:
      return true;
  }
}

function personsFor(seed: string, total: number, offset: number, limit: number) {
  const r = rng(seed);
  const start = Math.floor(r() * state.persons.length);
  const list = Array.from({ length: Math.min(limit, Math.max(0, total - offset)) }, (_, i) => state.persons[(start + (offset + i) * 7) % state.persons.length]);
  return { persons: list.map(summary), has_more: offset + limit < total };
}

function summary(p: MockPerson) {
  const { allIds: _a, events: _e, sessions: _s, ...rest } = p;
  void _a;
  void _e;
  void _s;
  return rest;
}

function runQuery(query: InsightQuery): RawResponse {
  const seed = JSON.stringify(query);
  const t0 = performance.now();
  let result;
  switch (query.kind) {
    case "TrendsQuery":
      result = trends(query, seed);
      break;
    case "FunnelsQuery":
      result = funnels(query, seed);
      break;
    case "RetentionQuery":
      result = retention(query, seed);
      break;
    case "LifecycleQuery":
      result = lifecycle(query, seed);
      break;
    case "StickinessQuery":
      result = stickiness(query, seed);
      break;
    case "PathsQuery":
      result = paths(query, seed);
      break;
    case "SqlQuery": {
      const out = sql(query, seed);
      if (typeof out === "string") return err(400, "invalid_query", out, "query");
      result = out;
      break;
    }
  }
  if (!hasEvents() && result.kind === "Trends") result = { ...result, series: result.series.map((s) => ({ ...s, data: s.data.map(() => 0), aggregated_value: 0 })) };
  const range = "date_range" in query ? query.date_range : { date_from: "-7d", date_to: null };
  const bs = buckets(range.date_from, range.date_to, "day");
  return ok({
    result,
    meta: {
      kind: query.kind,
      elapsed_ms: Math.round(performance.now() - t0 + 18 + (hash(seed) % 140)),
      cached: hash(seed) % 4 === 0,
      data_version: Math.floor(state.lastTick / 10_000),
      date_from: bs[0]?.toISOString() ?? new Date().toISOString(),
      date_to: new Date().toISOString(),
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    },
  });
}

function webOverview(search: URLSearchParams) {
  const from = search.get("date_from") ?? "-7d";
  const to = search.get("date_to");
  const interval = (search.get("interval") as "hour" | "day" | "week" | "month" | null) ?? autoInterval(from, to);
  const filters = search.get("properties") ? (JSON.parse(search.get("properties") as string) as PropertyFilter[]) : [];
  const f = Math.pow(0.42, filters.length);
  const seed = `${from}:${to}:${interval}:${search.get("properties")}`;
  const t = trends(
    {
      series: [
        { event: "$pageview", custom_name: null, properties: [], math: "dau", math_property: null },
        { event: "$pageview", custom_name: null, properties: [], math: "total", math_property: null },
      ],
      date_range: { date_from: from, date_to: to },
      interval,
      properties: [],
      breakdown: null,
      formula: null,
      compare: false,
      display: "ActionsLineGraph",
    },
    seed,
  );
  if (t.kind !== "Trends") throw new Error("unreachable");
  const live = hasEvents();
  const visitors = t.series[0].data.map((v) => (live ? Math.round(v * 1.6 * f) : 0));
  const pageviews = t.series[1].data.map((v) => (live ? Math.round(v * f) : 0));
  const r = rng(seed);
  const sum = (a: number[]) => a.reduce((x, y) => x + y, 0);
  const metric = (value: number, drift: number) => ({ value, previous: live ? Math.round(value * (1 - drift) * 100) / 100 : null });
  const v = Math.round(sum(visitors) * 0.7);
  return {
    visitors: metric(v, 0.12 * (r() - 0.3)),
    pageviews: metric(sum(pageviews), 0.1 * (r() - 0.2)),
    sessions: metric(Math.round(v * 1.38), 0.08 * (r() - 0.3)),
    bounce_rate: metric(live ? Math.round((38 + r() * 14) * 10) / 10 : 0, 0.06 * (r() - 0.5)),
    session_duration_s: metric(live ? Math.round(90 + r() * 200) : 0, 0.1 * (r() - 0.4)),
    interval,
    days: t.series[0].days,
    visitors_series: visitors,
    pageviews_series: pageviews,
    live_visitors: live ? 3 + Math.floor(Math.random() * 30) : 0,
  };
}

const DIM_KEY: Record<WebDimension, string> = {
  page: "$pathname",
  entry_page: "$pathname",
  exit_page: "$pathname",
  referring_domain: "$referring_domain",
  utm_source: "utm_source",
  utm_medium: "utm_medium",
  utm_campaign: "utm_campaign",
  browser: "$browser",
  os: "$os",
  device_type: "$device_type",
  country: "$geoip_country_code",
};

function webBreakdown(search: URLSearchParams) {
  const dim = (search.get("dimension") ?? "page") as WebDimension;
  const limit = Number(search.get("limit") ?? 10);
  const ov = webOverview(search);
  const r = rng(`${dim}:${search.toString()}`);
  let table = VALUES[DIM_KEY[dim]] ?? [];
  if (dim === "entry_page") table = table.map(([v, w]) => [v, v === "/" || v.startsWith("/blog") ? w * 2.4 : w * 0.6]);
  if (dim === "exit_page") table = table.map(([v, w]) => [v, v.startsWith("/app") || v === "/pricing" ? w * 2 : w * 0.7]);
  const total = table.reduce((a, [, w]) => a + w, 0);
  const rows = table
    .map(([value, w]) => {
      const share = (w / total) * (0.85 + r() * 0.3);
      return {
        value,
        visitors: Math.round(ov.visitors.value * share),
        views: Math.round(ov.pageviews.value * share * (0.9 + r() * 0.4)),
        bounce_rate: dim.includes("page") ? Math.round((25 + r() * 50) * 10) / 10 : null,
      };
    })
    .filter((x) => x.visitors > 0)
    .sort((a, b) => b.visitors - a.visitors);
  const extra = dim === "page" && limit > 10 ? Array.from({ length: 40 }, (_, i) => ({ value: `/docs/guide-${i + 1}`, visitors: Math.round(ov.visitors.value * 0.004 * (1 + r())), views: Math.round(ov.pageviews.value * 0.005), bounce_rate: 40 })) : [];
  return { dimension: dim, rows: [...rows, ...extra].slice(0, limit) };
}

function evaluateFlag(flag: FeatureFlag, distinctId: string) {
  if (!flag.active) return { key: flag.key, enabled: false, variant: null, reason: "disabled", condition_index: null, payload: null };
  const person = state.persons.find((p) => p.allIds.includes(distinctId));
  const props = (person?.properties ?? {}) as Record<string, unknown>;
  const bucket = (hash(`${flag.key}.${distinctId}`) % 10_000) / 100;
  for (let i = 0; i < flag.filters.groups.length; i++) {
    const g = flag.filters.groups[i];
    if (!g.properties.every((f) => matches(props, f))) continue;
    if (bucket >= (g.rollout_percentage ?? 100)) return { key: flag.key, enabled: false, variant: null, reason: "out_of_rollout_bound", condition_index: i, payload: null };
    let variant: string | null = g.variant ?? null;
    if (!variant && flag.filters.multivariate) {
      const vb = (hash(`${flag.key}.${distinctId}.variant`) % 10_000) / 100;
      let acc = 0;
      for (const v of flag.filters.multivariate.variants) {
        acc += v.rollout_percentage;
        if (vb < acc) {
          variant = v.key;
          break;
        }
      }
    }
    return { key: flag.key, enabled: true, variant, reason: "condition_match", condition_index: i, payload: flag.filters.payloads[variant ?? "true"] ?? null };
  }
  return { key: flag.key, enabled: false, variant: null, reason: "no_condition_match", condition_index: null, payload: null };
}

function catalogEvents(): CatalogEvent[] {
  if (!hasEvents()) return state.events.length && state.firstEventAt !== Infinity ? [] : [];
  return EVENTS.map(([name, w]) => ({ name, count: Math.round(48_000 * w), last_seen: new Date(now() - Math.floor(w * 1000)).toISOString() }));
}

// ── Router ───────────────────────────────────────────────────────────────

async function handle({ method, url, body, signal }: RawRequest): Promise<RawResponse> {
  const u = new URL(url, window.location.origin);
  const path = u.pathname.replace(/\/+$/, "");
  const search = u.searchParams;
  const heavy = path.endsWith("/query") || path.endsWith("/query/actors") || path.includes("/web/");
  await wait(heavy ? 140 + Math.random() * 320 : 40 + Math.random() * 110, signal);
  tick();

  if (path === "/api/auth/bootstrap") return ok({ setup_required: !state.setupDone });
  if (path === "/api/auth/setup" && method === "POST") {
    if (state.setupDone) return err(409, "conflict", "Initial setup has already been completed.");
    const b = body as { email: string; organization_name: string; project_name: string };
    state.setupDone = true;
    state.session = true;
    state.user = { id: "u1", email: b.email, name: "" };
    state.orgName = b.organization_name;
    state.projects = [{ ...PROJECTS[0], name: b.project_name }];
    state.firstEventAt = Infinity;
    return ok(workspace());
  }
  if (path === "/api/auth/login" && method === "POST") {
    const b = body as { email: string; password: string };
    if (!b.password || b.password.length < 4) return err(401, "unauthorized", "Authentication is required.");
    state.session = true;
    return ok(workspace());
  }
  if (path === "/api/auth/logout") {
    state.session = false;
    return ok({ status: "ok" });
  }
  if (path.startsWith("/api/shares/")) return err(404, "not_found", "The shared resource was not found.");
  if (path === "/api/invites/preview" || path === "/api/invites/accept") {
    const b = body as { token: string; name?: string | null; password?: string };
    const invite = state.invites.find((i) => i.token === b.token);
    if (!invite) return err(404, "invite_invalid", "This invite link is invalid, expired or already used.");
    const exists = invite.email.startsWith("existing");
    if (path.endsWith("/preview")) return ok({ organization_name: state.orgName, email: invite.email, role: invite.role, expires_at: invite.expires_at, account_exists: exists });
    if (exists ? (b.password ?? "").length < 4 : (b.password ?? "").length < 12) return err(exists ? 401 : 400, exists ? "unauthorized" : "invalid_request", "The password must be at least 12 characters.");
    state.invites = state.invites.filter((i) => i !== invite);
    state.session = true;
    state.role = invite.role;
    state.user = { id: `u${state.nextId++}`, email: invite.email, name: b.name?.trim() || invite.email };
    return ok(workspace());
  }
  if (path === "/capture" && method === "POST") {
    if (state.firstEventAt === Infinity) state.firstEventAt = now() + 1500;
    return ok({ status: 1 });
  }
  if (!state.session) return err(401, "unauthorized", "Authentication is required.");

  if (path === "/api/auth/me") return ok(workspace());
  if (path === "/api/auth/keys") {
    if (method === "POST") {
      const id = `key_${state.nextId++}`;
      const secret = `phx_${Array.from({ length: 40 }, () => "abcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 36)]).join("")}`;
      const b = body as { name: string; scope?: "read" | "write" };
      const key = { id, name: b.name, scope: b.scope ?? ("read" as const), key_prefix: secret.slice(0, 8), last_used: null, created_at: sec() };
      state.keys.push(key);
      return ok({ key, secret }, 201);
    }
    return ok(state.keys);
  }
  let m: RegExpExecArray | null;
  if ((m = /^\/api\/auth\/keys\/([^/]+)$/.exec(path)) && method === "DELETE") {
    state.keys = state.keys.filter((k) => k.id !== m![1]);
    return ok(null, 204);
  }
  if (path === "/api/organizations") return ok(workspace().organizations);
  if ((m = /^\/api\/organizations\/[^/]+\/members(?:\/([^/]+))?$/.exec(path))) {
    const target = state.members.find((x) => x.user_id === m![1]);
    if (!m[1]) return ok(state.members);
    if (!target) return err(404, "not_found", "The requested resource was not found.");
    const owners = state.members.filter((x) => x.role === "owner").length;
    if (state.role === "member" || (state.role === "admin" && target.role === "owner")) return err(403, "forbidden", "You do not have access to this resource.");
    if (method === "PATCH") {
      const role = (body as { role: Role }).role;
      if (target.role === "owner" && role !== "owner" && owners <= 1) return err(409, "last_owner", "An organization needs at least one owner.");
      if (state.role === "admin" && role === "owner") return err(403, "forbidden", "You do not have access to this resource.");
      target.role = role;
      return ok(target);
    }
    if (method === "DELETE") {
      if (target.role === "owner" && owners <= 1) return err(409, "last_owner", "An organization needs at least one owner.");
      state.members = state.members.filter((x) => x !== target);
      return ok(null, 204);
    }
  }
  if ((m = /^\/api\/organizations\/[^/]+\/invites(?:\/([^/]+))?$/.exec(path))) {
    if (state.role === "member") return err(403, "forbidden", "You do not have access to this resource.");
    if (method === "GET") return ok(state.invites.map(({ token: _token, ...invite }) => invite));
    if (method === "DELETE") {
      state.invites = state.invites.filter((i) => i.id !== m![1]);
      return ok(null, 204);
    }
    const b = body as { email: string; role: Role };
    if (!/^\S+@\S+\.\S+$/.test(b.email)) return err(400, "invalid_request", "Enter a valid email address.");
    if (state.members.some((x) => x.email === b.email.toLowerCase())) return err(409, "already_member", "This person is already a member of the organization.");
    if (state.role === "admin" && b.role === "owner") return err(403, "forbidden", "You do not have access to this resource.");
    state.invites = state.invites.filter((i) => i.email !== b.email.toLowerCase());
    const token = `hgi_${Array.from({ length: 64 }, () => "0123456789abcdef"[Math.floor(Math.random() * 16)]).join("")}`;
    const invite = { id: `inv_${state.nextId++}`, email: b.email.trim().toLowerCase(), role: b.role, created_by: "u1", created_at: sec(), expires_at: sec() + 7 * 86_400, token };
    state.invites.push(invite);
    const { token: _t, ...listed } = invite;
    return ok({ invite: listed, token, path: `/invite/${token}` }, 201);
  }
  if ((m = /^\/api\/organizations\/[^/]+\/projects$/.exec(path)) && method === "POST") {
    const p = { id: `0190f3a2-7c1e-7d4a-9b11-${String(state.nextId++).padStart(12, "0")}`, name: (body as { name: string }).name, token: `phc_${Math.random().toString(36).slice(2, 14)}` };
    state.projects = [...state.projects, p];
    return ok(p, 201);
  }

  m = /^\/api\/projects\/([^/]+)(\/.*)?$/.exec(path);
  if (!m) return { status: 404, body: null };
  const pid = m[1];
  const rest = m[2] ?? "";
  if (!state.projects.some((p) => p.id === pid)) return err(404, "not_found", "The requested resource was not found.");
  const otherProject = pid !== state.projects[0]?.id;

  if (rest === "/demo" && method === "POST") {
    await wait(1800, signal);
    state.firstEventAt = 0;
    return ok({ events: 184_532 });
  }
  if ((m = /^\/persons\/([^/]+)\/erase$/.exec(rest)) && method === "POST") {
    const id = decodeURIComponent(m[1]);
    const p = state.persons.find((x) => x.id === id || x.allIds.includes(id));
    if (!p) return err(404, "not_found", "The requested resource was not found.");
    state.persons = state.persons.filter((x) => x !== p);
    const before = state.events.length;
    state.events = state.events.filter((e) => e.person_id !== p.id);
    return ok({ distinct_ids: p.allIds.length, events: p.events + (before - state.events.length) });
  }
  if (rest === "/status") {
    const has = hasEvents() && !otherProject;
    const lag = Math.random() < 0.03 ? 7 + Math.random() * 10 : Math.random() * 1.5;
    return ok({
      has_events: has,
      last_event_at: has ? state.events[0]?.timestamp ?? null : null,
      ingestion_lag_seconds: has ? lag : 0,
      stored_events: has ? 1_284_311 + state.events.length : 0,
      stored_bytes: has ? 187_400_000 + state.events.length * 210 : 0,
      first_day: has ? new Date(now() - 118 * 86_400_000).toISOString().slice(0, 10) : null,
      last_day: has ? new Date().toISOString().slice(0, 10) : null,
    });
  }
  if (rest === "/forwarding") {
    const f = state.forwarding;
    if (method === "PUT") {
      const c = body as ForwardingConfig;
      if (c.enabled && (!/^https?:\/\//.test(c.host) || !c.posthog_token.startsWith("phc_"))) {
        return err(400, "invalid_request", "host must be an http(s) URL and posthog_token a phc_ project key.");
      }
      f.config = c;
    } else if (f.config?.enabled) {
      // Shadow mode in motion: a few events a poll, an occasional retry.
      f.forwarded += 3 + Math.floor(Math.random() * 9);
      f.queued = Math.floor(Math.random() * 4);
      if (Math.random() < 0.15) f.failed += 1;
    }
    return ok(f);
  }
  if (rest === "/query" && method === "POST") return runQuery((body as QueryRequest).query);
  if (rest === "/query/actors" && method === "POST") {
    const b = body as ActorsRequest;
    return ok(personsFor(JSON.stringify(b.selection), 40 + (hash(JSON.stringify(b.selection)) % 260), b.offset, b.limit));
  }
  if (rest === "/web/overview") return ok(webOverview(search));
  if (rest === "/web/breakdown") return ok(hasEvents() ? webBreakdown(search) : { dimension: search.get("dimension"), rows: [] });

  if (rest === "/catalog/events") {
    const s = (search.get("search") ?? "").toLowerCase();
    return ok(catalogEvents().filter((e) => e.name.toLowerCase().includes(s)));
  }
  if (rest === "/catalog/properties") {
    const type = search.get("type") ?? "event";
    const list = type === "person" ? PERSON_PROPS : EVENT_PROPS;
    return ok(list.map(([key, property_type], i) => ({ key, type, property_type, count: Math.round(90_000 / (i + 1)) })));
  }
  if (rest === "/catalog/values") {
    const key = search.get("key") ?? "";
    const s = (search.get("search") ?? "").toLowerCase();
    const table = VALUES[key] ?? (key === "email" ? state.persons.filter((p) => p.is_identified).slice(0, 30).map((p): [string, number] => [String(p.properties.email), 1]) : []);
    return ok(table.filter(([v]) => v !== "$$_none" && v.toLowerCase().includes(s)).map(([value, w]) => ({ value, count: Math.round(w * 913) })));
  }

  if (rest === "/events") {
    if (!hasEvents() || otherProject) return ok({ events: [], next_before: null });
    const ev = search.get("event");
    const person = search.get("person_id");
    const before = search.get("before");
    const limit = Number(search.get("limit") ?? 100);
    const list = state.events.filter((e) => (!ev || e.event === ev) && (!person || e.person_id === person) && (!before || e.timestamp < before));
    const page = list.slice(0, limit);
    return ok({ events: page, next_before: list.length > limit ? page[page.length - 1].timestamp : null });
  }
  if (rest === "/persons") {
    if (!hasEvents() || otherProject) return ok({ persons: [], next_cursor: null });
    const s = (search.get("search") ?? "").toLowerCase();
    const offset = Number(search.get("cursor") ?? 0);
    const limit = Number(search.get("limit") ?? 50);
    const list = state.persons.filter((p) => !s || p.display_name.toLowerCase().includes(s) || p.allIds.some((d) => d.toLowerCase().includes(s)) || JSON.stringify(p.properties).toLowerCase().includes(s));
    return ok({ persons: list.slice(offset, offset + limit).map(summary), next_cursor: offset + limit < list.length ? String(offset + limit) : null });
  }
  if ((m = /^\/persons\/([^/]+)$/.exec(rest))) {
    const key = decodeURIComponent(m[1]);
    const p = state.persons.find((x) => x.id === key || x.allIds.includes(key));
    if (!p) return err(404, "not_found", "The requested resource was not found.");
    return ok({ person: summary(p), distinct_ids: p.allIds, event_count: p.events, first_seen: p.created_at, last_seen: p.last_seen, session_count: p.sessions });
  }
  if ((m = /^\/persons\/([^/]+)\/events$/.exec(rest))) {
    const id = decodeURIComponent(m[1]);
    const p = state.persons.find((x) => x.id === id);
    if (!p) return err(404, "not_found", "The requested resource was not found.");
    const before = search.get("before");
    const r = rng(`pe:${id}:${before}`);
    const start = before ? new Date(before).getTime() : new Date(p.last_seen ?? new Date().toISOString()).getTime();
    const own = state.events.filter((e) => e.person_id === id && (!before || e.timestamp < before));
    const synth = Array.from({ length: 60 }, (_, i) => makeEvent(r, p, start - (i + 1) * (40_000 + Math.floor(r() * 900_000))));
    const events = [...own, ...synth].slice(0, 100);
    return ok({ events, next_before: events.length ? events[events.length - 1].timestamp : null });
  }

  if (rest === "/feature_flags") {
    if (method === "POST") {
      const input = body as FeatureFlagInput;
      if (state.flags.some((f) => f.key === input.key)) return err(409, "conflict", `A flag with key “${input.key}” already exists.`, "key");
      const flag: FeatureFlag = { ...input, id: state.nextId++, created_at: new Date().toISOString(), updated_at: new Date().toISOString() };
      state.flags.push(flag);
      return ok(flag, 201);
    }
    return ok(otherProject ? [] : state.flags);
  }
  if ((m = /^\/feature_flags\/(\d+)(\/evaluate)?$/.exec(rest))) {
    const id = Number(m[1]);
    const flag = state.flags.find((f) => f.id === id);
    if (!flag) return err(404, "not_found", "The requested resource was not found.");
    if (m[2]) return ok(evaluateFlag(flag, search.get("distinct_id") ?? ""));
    if (method === "PATCH") {
      Object.assign(flag, body as Partial<FeatureFlagInput>, { updated_at: new Date().toISOString() });
      return ok(flag);
    }
    if (method === "DELETE") {
      state.flags = state.flags.filter((f) => f.id !== id);
      return ok(null, 204);
    }
    return ok(flag);
  }

  if (rest === "/insights") {
    if (method === "POST") {
      const b = body as { name: string; description: string; query_ir: unknown };
      const ins = { id: `ins_${state.nextId++}`, project_id: pid, name: b.name, description: b.description, query_ir: b.query_ir, created_by: "u1", created_at: sec(), updated_at: sec() };
      state.insights.push(ins);
      return ok(ins, 201);
    }
    return ok(otherProject ? [] : state.insights);
  }
  if ((m = /^\/insights\/([^/]+)$/.exec(rest))) {
    const ins = state.insights.find((x) => x.id === m![1]);
    if (!ins) return err(404, "not_found", "The requested resource was not found.");
    if (method === "PUT") {
      Object.assign(ins, body as object, { updated_at: sec() });
      return ok(ins);
    }
    if (method === "DELETE") {
      state.insights = state.insights.filter((x) => x !== ins);
      for (const d of state.dashboards) d.tiles = d.tiles.filter((t) => t.insight_id !== ins.id);
      return ok(null, 204);
    }
    return ok(ins);
  }
  const expand = (d: State["dashboards"][number]) => ({ ...d, tiles: d.tiles.map((t) => ({ ...t, insight: state.insights.find((i) => i.id === t.insight_id) })) });
  if (rest === "/dashboards") {
    if (method === "POST") {
      const d = { id: `dash_${state.nextId++}`, project_id: pid, name: (body as { name: string }).name, tiles: [], created_by: "u1", created_at: sec() };
      state.dashboards.push(d);
      return ok(expand(d), 201);
    }
    return ok(otherProject ? [] : state.dashboards.map(expand));
  }
  if ((m = /^\/dashboards\/([^/]+)(\/tiles)?$/.exec(rest))) {
    const d = state.dashboards.find((x) => x.id === m![1]);
    if (!d) return err(404, "not_found", "The requested resource was not found.");
    if (m[2] && method === "PUT") {
      d.tiles = body as typeof d.tiles;
      return ok(expand(d));
    }
    if (method === "PUT") {
      d.name = (body as { name: string }).name;
      return ok(expand(d));
    }
    if (method === "DELETE") {
      state.dashboards = state.dashboards.filter((x) => x !== d);
      return ok(null, 204);
    }
    return ok(expand(d));
  }
  if (rest === "/shares") {
    if (method === "POST") {
      const b = body as { object_type: string; object_id: string };
      const s = { id: `share_${state.nextId++}`, project_id: pid, object_type: b.object_type, object_id: b.object_id, token: Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2), created_at: sec(), expires_at: null };
      state.shares.push(s);
      return ok(s, 201);
    }
    return ok(state.shares);
  }
  if ((m = /^\/shares\/([^/]+)$/.exec(rest)) && method === "DELETE") {
    state.shares = state.shares.filter((s) => s.id !== m![1]);
    return ok(null, 204);
  }
  return { status: 404, body: null };
}

export const mockTransport: Transport = (request) => handle(request);
