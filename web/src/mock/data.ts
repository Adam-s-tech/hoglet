// Deterministic, realistic synthetic analytics for the mock API.

import type { EventNode } from "../types/EventNode";
import type { EventRow } from "../types/EventRow";
import type { FunnelsQuery } from "../types/FunnelsQuery";
import type { FunnelStepResult } from "../types/FunnelStepResult";
import type { InsightResult } from "../types/InsightResult";
import type { Interval } from "../types/Interval";
import type { LifecycleQuery } from "../types/LifecycleQuery";
import type { PathLink } from "../types/PathLink";
import type { PathsQuery } from "../types/PathsQuery";
import type { PersonSummary } from "../types/PersonSummary";
import type { RetentionQuery } from "../types/RetentionQuery";
import type { SqlQuery } from "../types/SqlQuery";
import type { StickinessQuery } from "../types/StickinessQuery";
import type { TrendSeries } from "../types/TrendSeries";
import type { TrendsQuery } from "../types/TrendsQuery";
import { fmtBucket, resolveRange } from "../lib/format";

// ── Randomness ───────────────────────────────────────────────────────────

export function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export function rng(seed: number | string): () => number {
  let a = typeof seed === "string" ? hash(seed) : seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function pick<T>(r: () => number, items: T[]): T {
  return items[Math.floor(r() * items.length)];
}

function weighted<T>(r: () => number, table: [T, number][]): T {
  const total = table.reduce((a, [, w]) => a + w, 0);
  let x = r() * total;
  for (const [v, w] of table) {
    x -= w;
    if (x <= 0) return v;
  }
  return table[table.length - 1][0];
}

// ── Catalog ──────────────────────────────────────────────────────────────

export const EVENTS: [string, number][] = [
  ["$pageview", 1],
  ["$autocapture", 1.7],
  ["$pageleave", 0.72],
  ["$feature_flag_called", 0.38],
  ["insight_viewed", 0.31],
  ["$ai_generation", 0.09],
  ["$identify", 0.06],
  ["checkout_started", 0.045],
  ["$exception", 0.03],
  ["signed_up", 0.022],
  ["purchase_completed", 0.019],
  ["project_created", 0.015],
  ["invite_sent", 0.011],
];

export const VALUES: Record<string, [string, number][]> = {
  $browser: [
    ["Chrome", 58],
    ["Safari", 21],
    ["Firefox", 8],
    ["Edge", 7],
    ["Samsung Internet", 3],
    ["Opera", 2],
    ["Brave", 1],
  ],
  $os: [
    ["Mac OS X", 34],
    ["Windows", 29],
    ["iOS", 17],
    ["Android", 13],
    ["Linux", 6],
    ["Chrome OS", 1],
  ],
  $device_type: [
    ["Desktop", 68],
    ["Mobile", 28],
    ["Tablet", 4],
  ],
  $geoip_country_code: [
    ["US", 31],
    ["DE", 11],
    ["GB", 9],
    ["IN", 8],
    ["FR", 6],
    ["CA", 5],
    ["NL", 4],
    ["BR", 4],
    ["JP", 3],
    ["AU", 3],
    ["ES", 3],
    ["SE", 2],
    ["PL", 2],
    ["SG", 1],
  ],
  $referring_domain: [
    ["$$_none", 38],
    ["google.com", 27],
    ["news.ycombinator.com", 11],
    ["github.com", 8],
    ["twitter.com", 5],
    ["reddit.com", 4],
    ["duckduckgo.com", 3],
    ["linkedin.com", 2],
    ["bing.com", 2],
  ],
  $pathname: [
    ["/", 30],
    ["/pricing", 12],
    ["/docs", 11],
    ["/docs/getting-started", 8],
    ["/blog/one-binary-analytics", 7],
    ["/signup", 6],
    ["/login", 5],
    ["/app", 9],
    ["/app/insights", 5],
    ["/app/flags", 3],
    ["/changelog", 2],
    ["/checkout", 2],
  ],
  utm_source: [
    ["$$_none", 70],
    ["newsletter", 9],
    ["hn", 8],
    ["twitter", 5],
    ["google", 5],
    ["producthunt", 3],
  ],
  utm_medium: [
    ["$$_none", 70],
    ["email", 10],
    ["social", 9],
    ["cpc", 6],
    ["referral", 5],
  ],
  utm_campaign: [
    ["$$_none", 74],
    ["launch-week", 10],
    ["october-digest", 7],
    ["self-host-guide", 5],
    ["retargeting", 4],
  ],
  plan: [
    ["free", 61],
    ["pro", 24],
    ["team", 11],
    ["enterprise", 4],
  ],
  $ai_model: [
    ["claude-sonnet-4-5", 46],
    ["gpt-4.1-mini", 31],
    ["claude-haiku-4-5", 17],
    ["llama-3.3-70b", 6],
  ],
  $lib: [
    ["web", 74],
    ["posthog-node", 14],
    ["posthog-python", 9],
    ["posthog-ios", 3],
  ],
};

export const EVENT_PROPS: [string, string][] = [
  ["$current_url", "string"],
  ["$pathname", "string"],
  ["$host", "string"],
  ["$browser", "string"],
  ["$browser_version", "number"],
  ["$os", "string"],
  ["$device_type", "string"],
  ["$screen_width", "number"],
  ["$referrer", "string"],
  ["$referring_domain", "string"],
  ["$geoip_country_code", "string"],
  ["$session_id", "string"],
  ["$lib", "string"],
  ["utm_source", "string"],
  ["utm_medium", "string"],
  ["utm_campaign", "string"],
  ["plan", "string"],
  ["revenue", "number"],
  ["$ai_model", "string"],
  ["$ai_total_cost_usd", "number"],
  ["$ai_latency", "number"],
  ["$ai_input_tokens", "number"],
  ["$exception_type", "string"],
];

export const PERSON_PROPS: [string, string][] = [
  ["email", "string"],
  ["name", "string"],
  ["plan", "string"],
  ["company", "string"],
  ["seats", "number"],
  ["$initial_referring_domain", "string"],
  ["$geoip_country_code", "string"],
  ["$browser", "string"],
  ["is_beta", "boolean"],
];

// ── Persons ──────────────────────────────────────────────────────────────

const FIRST = ["Ava", "Noah", "Mia", "Liam", "Zoe", "Ethan", "Ines", "Kai", "Lena", "Omar", "Priya", "Jonas", "Sofia", "Mateo", "Hana", "Felix", "Aria", "Tariq", "Nora", "Elias", "Yuki", "Ravi", "Clara", "Theo", "Amara", "Lucas", "Freya", "Diego", "Maya", "Arjun"];
const LAST = ["Chen", "Okafor", "Schmidt", "Garcia", "Kowalski", "Tanaka", "Silva", "Novak", "Haddad", "Larsen", "Patel", "Moreau", "Rossi", "Kim", "Andersson", "Dubois", "Nakamura", "Fischer", "Costa", "Ibrahim"];
const DOMAINS = ["acme.io", "northwind.dev", "globex.com", "initech.co", "umbrella.app", "hooli.xyz", "piedpiper.net", "stark.industries", "wayne.tech", "tyrell.ai"];

function uuid(r: () => number): string {
  const hex = () => Math.floor(r() * 16).toString(16);
  const s = Array.from({ length: 32 }, hex).join("");
  return `${s.slice(0, 8)}-${s.slice(8, 12)}-7${s.slice(13, 16)}-a${s.slice(17, 20)}-${s.slice(20, 32)}`;
}

export interface MockPerson extends PersonSummary {
  allIds: string[];
  events: number;
  sessions: number;
}

export function makePersons(count: number): MockPerson[] {
  const r = rng("persons");
  const now = Date.now();
  const out: MockPerson[] = [];
  for (let i = 0; i < count; i++) {
    const identified = r() < 0.62;
    const anon = uuid(r);
    const first = pick(r, FIRST);
    const last = pick(r, LAST);
    const company = pick(r, DOMAINS);
    const email = `${first.toLowerCase()}.${last.toLowerCase()}@${company}`;
    const created = now - Math.floor(r() * 120 * 86_400_000);
    const lastSeen = Math.min(now, created + Math.floor(r() * (now - created)));
    const recent = r() < 0.2 ? now - Math.floor(r() * 3_600_000) : lastSeen;
    const ids = identified ? [email, anon, ...(r() < 0.35 ? [uuid(r)] : [])] : [anon];
    const properties: Record<string, unknown> = identified
      ? {
          email,
          name: `${first} ${last}`,
          plan: weighted(r, VALUES.plan),
          company: company.split(".")[0].replace(/^\w/, (c) => c.toUpperCase()),
          seats: 1 + Math.floor(r() * 40),
          is_beta: r() < 0.2,
          $initial_referring_domain: weighted(r, VALUES.$referring_domain).replace("$$_none", "$direct"),
          $geoip_country_code: weighted(r, VALUES.$geoip_country_code),
          $browser: weighted(r, VALUES.$browser),
        }
      : {
          $initial_referring_domain: weighted(r, VALUES.$referring_domain).replace("$$_none", "$direct"),
          $geoip_country_code: weighted(r, VALUES.$geoip_country_code),
          $browser: weighted(r, VALUES.$browser),
        };
    out.push({
      id: uuid(r),
      display_name: identified ? email : anon,
      distinct_ids: ids.slice(0, 10),
      allIds: ids,
      properties,
      is_identified: identified,
      created_at: new Date(created).toISOString(),
      last_seen: new Date(recent).toISOString(),
      events: 3 + Math.floor(r() * 900),
      sessions: 1 + Math.floor(r() * 60),
    });
  }
  return out.sort((a, b) => (b.last_seen ?? "").localeCompare(a.last_seen ?? ""));
}

// ── Events ───────────────────────────────────────────────────────────────

export function makeEvent(r: () => number, person: MockPerson, ts: number, forced?: string): EventRow {
  const event = forced ?? weighted(r, EVENTS);
  const path = weighted(r, VALUES.$pathname);
  const props: Record<string, unknown> = {
    $lib: event === "$ai_generation" ? "posthog-node" : weighted(r, VALUES.$lib),
    $session_id: uuid(rng(`${person.id}:${Math.floor(ts / 1_800_000)}`)),
  };
  if (props.$lib === "web") {
    Object.assign(props, {
      $current_url: `https://acme.io${path}`,
      $pathname: path,
      $host: "acme.io",
      $browser: person.properties.$browser ?? weighted(r, VALUES.$browser),
      $browser_version: 120 + Math.floor(r() * 12),
      $os: weighted(r, VALUES.$os),
      $device_type: weighted(r, VALUES.$device_type),
      $screen_width: pick(r, [1440, 1920, 390, 1280, 2560, 820]),
      $referring_domain: weighted(r, VALUES.$referring_domain).replace("$$_none", "$direct"),
      $geoip_country_code: person.properties.$geoip_country_code,
    });
  }
  if (event === "$autocapture") Object.assign(props, { $event_type: "click", $el_text: pick(r, ["Get started", "Pricing", "Sign up", "Save", "Create insight", "Docs"]) });
  if (event === "purchase_completed") Object.assign(props, { revenue: Math.round((19 + r() * 480) * 100) / 100, plan: weighted(r, VALUES.plan) });
  if (event === "$ai_generation")
    Object.assign(props, {
      $ai_model: weighted(r, VALUES.$ai_model),
      $ai_provider: "anthropic",
      $ai_input_tokens: Math.floor(200 + r() * 4000),
      $ai_output_tokens: Math.floor(50 + r() * 900),
      $ai_latency: Math.round((0.4 + r() * 6) * 100) / 100,
      $ai_total_cost_usd: Math.round(r() * 0.04 * 10000) / 10000,
      $ai_trace_id: uuid(r),
    });
  if (event === "$exception") Object.assign(props, { $exception_type: pick(r, ["TypeError", "NetworkError", "RangeError"]), $exception_message: "Cannot read properties of undefined (reading 'id')" });
  if (event === "$feature_flag_called") Object.assign(props, { $feature_flag: pick(r, ["new-onboarding", "ai-summaries", "pricing-v2"]), $feature_flag_response: r() < 0.5 });
  return {
    uuid: uuid(r),
    event,
    distinct_id: person.distinct_ids[0],
    person_id: person.id,
    timestamp: new Date(ts).toISOString(),
    properties: props,
  };
}

// ── Time buckets ─────────────────────────────────────────────────────────

export function buckets(date_from: string, date_to: string | null, interval: Interval): Date[] {
  const [a, b] = resolveRange(date_from, date_to);
  const start = new Date(a);
  if (interval === "hour") start.setMinutes(0, 0, 0);
  else start.setHours(0, 0, 0, 0);
  if (interval === "week") start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
  if (interval === "month") start.setDate(1);
  const out: Date[] = [];
  const cur = new Date(start);
  while (cur <= b && out.length < 800) {
    out.push(new Date(cur));
    if (interval === "hour") cur.setHours(cur.getHours() + 1);
    else if (interval === "day") cur.setDate(cur.getDate() + 1);
    else if (interval === "week") cur.setDate(cur.getDate() + 7);
    else cur.setMonth(cur.getMonth() + 1);
  }
  return out;
}

/** Daily-ish traffic level for a bucket: growth, weekly and daily rhythm, noise. */
function traffic(d: Date, interval: Interval, r: () => number): number {
  const ageDays = (Date.now() - d.getTime()) / 86_400_000;
  const growth = 1 + Math.max(0, 120 - ageDays) / 160;
  const weekday = [0.62, 1.04, 1.1, 1.08, 1.02, 0.94, 0.66][d.getDay()];
  let base = 2400 * growth * weekday;
  if (interval === "hour") {
    const h = d.getHours();
    base = (base / 24) * (0.25 + 1.5 * Math.exp(-((h - 14) ** 2) / 30));
  } else if (interval === "week") base *= 7;
  else if (interval === "month") base *= 30;
  const future = d.getTime() > Date.now() ? 0 : 1;
  return base * (0.86 + r() * 0.28) * future;
}

function eventWeight(event: string | null): number {
  if (event === null) return EVENTS.reduce((a, [, w]) => a + w, 0);
  return EVENTS.find(([e]) => e === event)?.[1] ?? 0.05 + (hash(event) % 100) / 1000;
}

function mathFactor(n: EventNode): number {
  switch (n.math) {
    case "dau":
      return 0.16;
    case "weekly_active":
      return 0.62;
    case "monthly_active":
      return 1.9;
    case "unique_session":
      return 0.34;
    default:
      return 1;
  }
}

function propertyValue(n: EventNode, r: () => number): number {
  const key = n.math_property ?? "";
  const base = key.includes("cost") ? 0.012 : key.includes("latency") ? 2.4 : key.includes("revenue") ? 96 : key.includes("tokens") ? 1800 : 42;
  const spread = { min: 0.15, max: 4.2, median: 0.9, p90: 2.1, p95: 2.7, p99: 3.6, avg: 1, sum: 1 }[n.math as string] ?? 1;
  return base * spread * (0.85 + r() * 0.3);
}

// ── Formula ──────────────────────────────────────────────────────────────

function evalFormula(formula: string, vars: Record<string, number>): number {
  const tokens = formula.toUpperCase().match(/\d+(\.\d+)?|[A-Z]|[-+*/()]/g) ?? [];
  let i = 0;
  const peek = () => tokens[i];
  const next = () => tokens[i++];
  const primary = (): number => {
    const t = next();
    if (t === "(") {
      const v = expr();
      next();
      return v;
    }
    if (t === "-") return -primary();
    if (t && /^[A-Z]$/.test(t)) return vars[t] ?? 0;
    return Number(t ?? 0);
  };
  const term = (): number => {
    let v = primary();
    while (peek() === "*" || peek() === "/") {
      const op = next();
      const w = primary();
      v = op === "*" ? v * w : w === 0 ? 0 : v / w;
    }
    return v;
  };
  const expr = (): number => {
    let v = term();
    while (peek() === "+" || peek() === "-") {
      const op = next();
      const w = term();
      v = op === "+" ? v + w : v - w;
    }
    return v;
  };
  const v = expr();
  return Number.isFinite(v) ? v : 0;
}

// ── Insight results ──────────────────────────────────────────────────────

export function trends(q: TrendsQuery, seed: string): InsightResult {
  const days = buckets(q.date_range.date_from, q.date_range.date_to, q.interval);
  const labels = days.map((d) => fmtBucket(d.toISOString(), q.interval));
  const r = rng(seed);
  const shares: [string | null, number][] = q.breakdown
    ? (VALUES[q.breakdown.property] ?? [["value-a", 50], ["value-b", 30], ["value-c", 20]]).slice(0, q.breakdown.limit).map(([v, w]) => [v, w / 100])
    : [[null, 1]];
  const filterFactor = Math.pow(0.55, q.properties.length);
  const make = (offsetMs: number, compare: string | null): TrendSeries[] => {
    const out: TrendSeries[] = [];
    q.series.forEach((n, si) => {
      const nf = Math.pow(0.6, n.properties.length) * filterFactor;
      for (const [bv, share] of shares) {
        const rr = rng(`${seed}:${si}:${bv}:${compare}`);
        const property = ["sum", "avg", "min", "max", "median", "p90", "p95", "p99"].includes(n.math);
        const data = days.map((d) => {
          const t = traffic(new Date(d.getTime() - offsetMs), q.interval, rr);
          if (property) return t === 0 ? 0 : Math.round(propertyValue(n, rr) * (n.math === "sum" ? t * eventWeight(n.event) * 0.02 : 1) * 100) / 100;
          return Math.round(t * eventWeight(n.event) * mathFactor(n) * share * nf * (compare ? 0.86 : 1));
        });
        const total = data.reduce((a, b) => a + b, 0);
        const agg = ["dau", "weekly_active", "monthly_active"].includes(n.math) ? Math.round(total * 0.42) : property && n.math !== "sum" ? data.reduce((a, b) => a + b, 0) / Math.max(1, data.filter((x) => x > 0).length) : total;
        out.push({
          label: n.custom_name || n.event || "All events",
          series_index: si,
          breakdown_value: bv,
          compare,
          days: days.map((d) => new Date(d.getTime() - offsetMs).toISOString()),
          labels,
          data,
          aggregated_value: Math.round(agg * 100) / 100,
        });
      }
    });
    return out;
  };
  let series = make(0, null);
  if (q.formula) {
    const groups = new Map<string, TrendSeries[]>();
    for (const s of series) {
      const k = s.breakdown_value ?? "";
      groups.set(k, [...(groups.get(k) ?? []), s]);
    }
    series = [...groups.entries()].map(([bv, ss]) => {
      const data = days.map((_, i) => {
        const vars: Record<string, number> = {};
        ss.forEach((s, j) => (vars[String.fromCharCode(65 + j)] = s.data[i]));
        return Math.round(evalFormula(q.formula as string, vars) * 100) / 100;
      });
      return { ...ss[0], label: q.formula as string, series_index: null, breakdown_value: bv || null, data, aggregated_value: Math.round(data.reduce((a, b) => a + b, 0) * 100) / 100 };
    });
  }
  if (q.compare) {
    const span = days.length > 1 ? days[days.length - 1].getTime() - days[0].getTime() + (days[1].getTime() - days[0].getTime()) : 86_400_000;
    series = [...series, ...make(span, "previous")];
  }
  void r;
  return { kind: "Trends", series };
}

export function funnels(q: FunnelsQuery, seed: string): InsightResult {
  const r = rng(seed);
  const start = Math.round(3200 * (0.7 + r() * 0.6) * Math.pow(0.6, q.properties.length));
  const rates = q.series.map((_, i) => (i === 0 ? 1 : 0.32 + r() * 0.42 * (q.funnel_order === "strict" ? 0.7 : 1)));
  const build = (n0: number, rr: () => number): FunnelStepResult[] => {
    let prev = n0;
    return q.series.map((s, i) => {
      const count = i === 0 ? n0 : Math.round(prev * Math.min(0.97, rates[i] * (0.9 + rr() * 0.2)));
      const res: FunnelStepResult = {
        order: i,
        name: s.custom_name || s.event || "All events",
        count,
        conversion_from_previous: i === 0 ? 100 : prev ? (count / prev) * 100 : 0,
        conversion_from_start: n0 ? (count / n0) * 100 : 0,
        dropped_off: i === 0 ? 0 : prev - count,
        average_conversion_time_s: i === 0 ? null : 600 + rr() * 86_400,
        median_conversion_time_s: i === 0 ? null : 300 + rr() * 30_000,
      };
      prev = count;
      return res;
    });
  };
  const steps = build(start, rng(`${seed}:all`));
  const breakdowns = q.breakdown
    ? (VALUES[q.breakdown.property] ?? [["a", 60], ["b", 40]]).slice(0, Math.min(q.breakdown.limit, 6)).map(([v, w]) => ({ breakdown_value: v, steps: build(Math.round((start * w) / 100), rng(`${seed}:${v}`)) }))
    : [];
  const edges = [0, 60, 300, 900, 3600, 10_800, 43_200, 86_400, 259_200, 604_800, 1_209_600];
  const conv = steps[steps.length - 1].count;
  const shape = [0.06, 0.14, 0.17, 0.2, 0.15, 0.11, 0.08, 0.05, 0.03, 0.01];
  const time_to_convert = shape.map((s, i) => ({ from_s: edges[i], to_s: edges[i + 1], count: Math.round(conv * s) }));
  return { kind: "Funnels", steps, breakdowns, time_to_convert };
}

export function retention(q: RetentionQuery, seed: string): InsightResult {
  const r = rng(seed);
  const n = q.total_intervals;
  const unit = q.period === "day" ? 1 : q.period === "week" ? 7 : 30;
  const now = new Date();
  now.setHours(0, 0, 0, 0);
  const base = q.period === "day" ? 420 : q.period === "week" ? 1600 : 5200;
  const r1 = 0.28 + r() * 0.2;
  const cohorts = Array.from({ length: n }, (_, i) => {
    const d = new Date(now);
    d.setDate(d.getDate() - (n - 1 - i) * unit);
    const size = Math.round(base * (0.8 + r() * 0.4) * (q.retention_type === "retention_first_time" ? 0.45 : 1));
    const values = Array.from({ length: n - i }, (_, k) => (k === 0 ? size : Math.round(size * r1 * Math.pow(k, -0.42) * (0.88 + r() * 0.24))));
    return {
      date: d.toISOString(),
      label: q.period === "month" ? d.toLocaleDateString(undefined, { month: "short", year: "numeric" }) : d.toLocaleDateString(undefined, { month: "short", day: "numeric" }),
      size,
      values,
    };
  });
  return { kind: "Retention", period: q.period, cohorts };
}

export function lifecycle(q: LifecycleQuery, seed: string): InsightResult {
  const days = buckets(q.date_range.date_from, q.date_range.date_to, q.interval);
  const r = rng(seed);
  const w = eventWeight(q.series.event) * 0.16;
  const mk = (f: number) => days.map((d) => BigInt(Math.round(traffic(d, q.interval, r) * w * f)));
  return {
    kind: "Lifecycle",
    days: days.map((d) => d.toISOString()),
    labels: days.map((d) => fmtBucket(d.toISOString(), q.interval)),
    new: mk(0.22),
    returning: mk(0.55),
    resurrecting: mk(0.12),
    dormant: mk(0.3).map((v) => -v),
  };
}

export function stickiness(q: StickinessQuery, seed: string): InsightResult {
  const n = Math.max(1, buckets(q.date_range.date_from, q.date_range.date_to, q.interval).length);
  const unit = q.interval === "hour" ? "hour" : q.interval === "week" ? "week" : q.interval === "month" ? "month" : "day";
  return {
    kind: "Stickiness",
    series: q.series.map((s, si) => {
      const r = rng(`${seed}:${si}`);
      const top = 900 * eventWeight(s.event) * (0.7 + r() * 0.5);
      return {
        label: s.custom_name || s.event || "All events",
        series_index: si,
        data: Array.from({ length: n }, (_, i) => Math.round(top * Math.pow(0.68, i) + (i === n - 1 ? top * 0.05 : 0))),
        labels: Array.from({ length: n }, (_, i) => `${i + 1} ${unit}${i === 0 ? "" : "s"}`),
      };
    }),
  };
}

export function paths(q: PathsQuery, seed: string): InsightResult {
  const r = rng(seed);
  const pages = q.paths_type === "custom_events" ? ["signed_up", "project_created", "insight_viewed", "invite_sent", "checkout_started", "purchase_completed"] : VALUES.$pathname.map(([p]) => p);
  const links: PathLink[] = [];
  let frontier: [string, number][] = q.start_point ? [[q.start_point, 2400]] : pages.slice(0, 4).map((p, i) => [p, Math.round(2600 / (i + 1))]);
  for (let step = 1; step < q.step_limit; step++) {
    const next = new Map<string, number>();
    for (const [node, value] of frontier) {
      const targets = pages.filter((p) => p !== node).sort(() => r() - 0.5).slice(0, 3 + Math.floor(r() * 2));
      let remaining = value * (0.55 + r() * 0.25);
      for (const t of targets) {
        const v = Math.round(remaining * (0.3 + r() * 0.35));
        if (v < 6) continue;
        remaining -= v;
        links.push({ source: `${step}_${node}`, target: `${step + 1}_${t}`, value: v, average_conversion_time_s: 20 + r() * 400 });
        next.set(t, (next.get(t) ?? 0) + v);
      }
    }
    frontier = [...next.entries()].sort((a, b) => b[1] - a[1]).slice(0, 5);
  }
  return { kind: "Paths", links: links.sort((a, b) => b.value - a.value).slice(0, q.edge_limit) };
}

export function sql(q: SqlQuery, seed: string): InsightResult | string {
  const text = q.query.trim().toLowerCase();
  if (!/^(select|with)\b/.test(text)) return "Only read-only SELECT queries are allowed.";
  if (/\b(insert|update|delete|drop|create|alter|attach|copy)\b/.test(text)) return "Only read-only SELECT queries are allowed.";
  const r = rng(seed);
  if (text.includes("group by event")) {
    const rows = EVENTS.map(([e, w]) => [e, Math.round(16_800 * w * (0.9 + r() * 0.2)), Math.round(2100 * Math.sqrt(w) * (0.9 + r() * 0.2))]);
    return { kind: "Sql", columns: ["event", "events", "persons"], types: ["VARCHAR", "BIGINT", "BIGINT"], rows, truncated: false };
  }
  const rows = Array.from({ length: 25 }, (_, i) => [
    uuid(r),
    weighted(r, EVENTS),
    `user_${100 + Math.floor(r() * 900)}`,
    new Date(Date.now() - i * 37_000).toISOString(),
    JSON.stringify({ $pathname: weighted(r, VALUES.$pathname) }),
  ]);
  return { kind: "Sql", columns: ["uuid", "event", "distinct_id", "timestamp", "properties"], types: ["UUID", "VARCHAR", "VARCHAR", "TIMESTAMP", "JSON"], rows, truncated: false };
}
