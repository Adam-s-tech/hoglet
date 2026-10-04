import type { EventNode } from "../types/EventNode";
import type { InsightQuery } from "../types/InsightQuery";
import type { Math as MathKind } from "../types/Math";
import type { PropertyFilter } from "../types/PropertyFilter";
import type { IconName } from "@/components/icons";
import { completeFilters, eventLabel } from "../lib/properties";

export type QueryKind = InsightQuery["kind"];

export const KINDS: { kind: QueryKind; slug: string; label: string; icon: IconName; blurb: string }[] = [
  { kind: "TrendsQuery", slug: "trends", label: "Trends", icon: "trends", blurb: "Events and users over time" },
  { kind: "FunnelsQuery", slug: "funnels", label: "Funnels", icon: "funnel", blurb: "Conversion through ordered steps" },
  { kind: "RetentionQuery", slug: "retention", label: "Retention", icon: "retention", blurb: "Who comes back, by cohort" },
  { kind: "LifecycleQuery", slug: "lifecycle", label: "Lifecycle", icon: "lifecycle", blurb: "New, returning, resurrecting, dormant" },
  { kind: "StickinessQuery", slug: "stickiness", label: "Stickiness", icon: "stickiness", blurb: "How many days users are active" },
  { kind: "PathsQuery", slug: "paths", label: "Paths", icon: "paths", blurb: "The routes users take" },
  { kind: "SqlQuery", slug: "sql", label: "SQL", icon: "sql", blurb: "Read-only SQL over events" },
];

export function kindInfo(kind: QueryKind) {
  return KINDS.find((k) => k.kind === kind) ?? KINDS[0];
}
export function kindFromSlug(slug: string | null): QueryKind {
  return KINDS.find((k) => k.slug === slug)?.kind ?? "TrendsQuery";
}

export function eventNode(event: string | null, math: MathKind = "total"): EventNode {
  return { event, custom_name: null, properties: [], math, math_property: null };
}

export function defaultQuery(kind: QueryKind): InsightQuery {
  const range = { date_from: "-7d", date_to: null };
  switch (kind) {
    case "TrendsQuery":
      return {
        kind,
        series: [eventNode("$pageview")],
        date_range: range,
        interval: "day",
        properties: [],
        breakdown: null,
        formula: null,
        compare: false,
        display: "ActionsLineGraph",
      };
    case "FunnelsQuery":
      return {
        kind,
        series: [eventNode("$pageview"), eventNode(null)],
        date_range: { date_from: "-14d", date_to: null },
        properties: [],
        breakdown: null,
        funnel_window: { interval: 14, unit: "day" },
        funnel_order: "ordered",
        exclusions: [],
      };
    case "RetentionQuery":
      return {
        kind,
        target: eventNode("$pageview"),
        returning: eventNode("$pageview"),
        period: "week",
        total_intervals: 8,
        retention_type: "retention_recurring",
        properties: [],
      };
    case "LifecycleQuery":
      return { kind, series: eventNode("$pageview"), date_range: { date_from: "-30d", date_to: null }, interval: "day", properties: [] };
    case "StickinessQuery":
      return { kind, series: [eventNode("$pageview")], date_range: { date_from: "-30d", date_to: null }, interval: "day", properties: [] };
    case "PathsQuery":
      return {
        kind,
        paths_type: "pageviews",
        start_point: null,
        end_point: null,
        step_limit: 5,
        edge_limit: 50,
        date_range: range,
        properties: [],
      };
    case "SqlQuery":
      return {
        kind,
        query: "SELECT event, count() AS events, count(DISTINCT person_id) AS persons\nFROM events\nWHERE timestamp > now_utc() - INTERVAL 7 DAY\nGROUP BY event\nORDER BY events DESC\nLIMIT 50",
      };
  }
}

export const PROPERTY_MATHS: MathKind[] = ["sum", "avg", "min", "max", "median", "p90", "p95", "p99"];

export const MATHS: { value: MathKind; label: string; group: "Events" | "Users" | "Sessions" | "Property" }[] = [
  { value: "total", label: "Total count", group: "Events" },
  { value: "dau", label: "Unique users", group: "Users" },
  { value: "weekly_active", label: "Weekly active users", group: "Users" },
  { value: "monthly_active", label: "Monthly active users", group: "Users" },
  { value: "unique_session", label: "Unique sessions", group: "Sessions" },
  { value: "sum", label: "Property sum", group: "Property" },
  { value: "avg", label: "Property average", group: "Property" },
  { value: "min", label: "Property minimum", group: "Property" },
  { value: "max", label: "Property maximum", group: "Property" },
  { value: "median", label: "Property median", group: "Property" },
  { value: "p90", label: "Property 90th percentile", group: "Property" },
  { value: "p95", label: "Property 95th percentile", group: "Property" },
  { value: "p99", label: "Property 99th percentile", group: "Property" },
];

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
export function letter(i: number): string {
  return LETTERS[i] ?? String(i + 1);
}

function cleanNode(n: EventNode): EventNode {
  return { ...n, properties: completeFilters(n.properties), custom_name: n.custom_name?.trim() || null };
}
function cleanFilters(f: PropertyFilter[]): PropertyFilter[] {
  return completeFilters(f);
}

/** Strip half-built UI state so only complete queries reach the server. */
export function sanitize(q: InsightQuery): InsightQuery {
  switch (q.kind) {
    case "TrendsQuery":
      return { ...q, series: q.series.map(cleanNode), properties: cleanFilters(q.properties), formula: q.formula?.trim() || null };
    case "FunnelsQuery":
      return { ...q, series: q.series.map(cleanNode), properties: cleanFilters(q.properties), exclusions: q.exclusions.filter((e) => e.event) };
    case "RetentionQuery":
      return { ...q, target: cleanNode(q.target), returning: cleanNode(q.returning), properties: cleanFilters(q.properties) };
    case "LifecycleQuery":
      return { ...q, series: cleanNode(q.series), properties: cleanFilters(q.properties) };
    case "StickinessQuery":
      return { ...q, series: q.series.map(cleanNode), properties: cleanFilters(q.properties) };
    case "PathsQuery":
      return { ...q, properties: cleanFilters(q.properties), start_point: q.start_point?.trim() || null, end_point: q.end_point?.trim() || null };
    case "SqlQuery":
      return q;
  }
}

/** Why a query can't run yet, in words; null when it's ready. */
export function incomplete(q: InsightQuery): string | null {
  switch (q.kind) {
    case "TrendsQuery":
      if (q.series.length === 0) return "Add a series to see a trend.";
      if (q.series.some((s) => PROPERTY_MATHS.includes(s.math) && !s.math_property)) return "Pick the property to aggregate.";
      return null;
    case "FunnelsQuery":
      if (q.series.length < 2) return "A funnel needs at least two steps.";
      return null;
    case "StickinessQuery":
      return q.series.length === 0 ? "Add a series." : null;
    case "SqlQuery":
      return q.query.trim() ? null : "Write a query.";
    default:
      return null;
  }
}

export function encodeQuery(q: InsightQuery): string {
  return encodeURIComponent(JSON.stringify(q));
}
export function decodeQuery(s: string): InsightQuery | null {
  try {
    return normalizeQuery(JSON.parse(decodeURIComponent(s)));
  } catch {
    return null;
  }
}

type Loose = Record<string, unknown>;
const isObject = (v: unknown): v is Loose => typeof v === "object" && v !== null && !Array.isArray(v);

/** Fill an event node's omitted fields (hand-written links, imported insights). */
function normalizeNode(raw: unknown): EventNode {
  const n = isObject(raw) ? raw : {};
  return {
    event: typeof n.event === "string" ? n.event : null,
    custom_name: typeof n.custom_name === "string" ? n.custom_name : null,
    properties: Array.isArray(n.properties) ? (n.properties as PropertyFilter[]) : [],
    math: typeof n.math === "string" ? (n.math as MathKind) : "total",
    math_property: typeof n.math_property === "string" ? n.math_property : null,
  };
}

/**
 * A complete query of a known kind from anything shaped like one: every
 * omitted field takes the kind's default, so the editor never meets a hole.
 * Returns null for unknown kinds.
 */
export function normalizeQuery(raw: unknown): InsightQuery | null {
  if (!isObject(raw) || typeof raw.kind !== "string") return null;
  const kind = raw.kind as QueryKind;
  if (!KINDS.some((k) => k.kind === kind)) return null;
  const base = defaultQuery(kind) as unknown as Loose;
  const merged: Loose = { ...base };
  for (const [key, value] of Object.entries(raw)) {
    if (value !== undefined && value !== null && key in base) merged[key] = value;
    else if (value === null && key in base) merged[key] = base[key] === null ? null : base[key];
  }
  if (Array.isArray(merged.series)) merged.series = (merged.series as unknown[]).map(normalizeNode);
  else if ("series" in base && !Array.isArray(base.series)) merged.series = normalizeNode(merged.series);
  for (const key of ["target", "returning"]) if (key in base) merged[key] = normalizeNode(merged[key]);
  if (isObject(merged.date_range)) {
    merged.date_range = { ...(base.date_range as Loose), ...merged.date_range };
  }
  for (const key of ["properties", "exclusions"]) {
    if (key in base && !Array.isArray(merged[key])) merged[key] = base[key];
  }
  return merged as unknown as InsightQuery;
}

/** Describe a query in a few words (insight list subtitle). */
export function summarize(q: InsightQuery | null): string {
  if (!q) return "Legacy query";
  const ev = (n: EventNode) => n.custom_name || eventLabel(n.event);
  switch (q.kind) {
    case "TrendsQuery":
      return q.series.map(ev).join(", ");
    case "FunnelsQuery":
      return q.series.map(ev).join(" → ");
    case "RetentionQuery":
      return `${ev(q.target)} then ${ev(q.returning)}`;
    case "LifecycleQuery":
      return ev(q.series);
    case "StickinessQuery":
      return q.series.map(ev).join(", ");
    case "PathsQuery":
      return q.start_point ? `Paths from ${q.start_point}` : "User paths";
    case "SqlQuery":
      return q.query.replace(/\s+/g, " ").slice(0, 80);
  }
}

/** Default title for an unnamed insight. */
export function defaultName(q: InsightQuery): string {
  if (q.kind === "SqlQuery") return "SQL query";
  const s = summarize(q);
  return s.length > 60 ? `${kindInfo(q.kind).label} insight` : s;
}

/** Apply a dashboard-level date override to any query with a range. */
export function withDateRange(q: InsightQuery, range: { date_from: string; date_to: string | null } | null): InsightQuery {
  if (!range) return q;
  if ("date_range" in q) return { ...q, date_range: range };
  return q;
}
