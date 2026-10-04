// Number, time, and range formatting. One place, so every screen agrees.

const nf0 = new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 });
const nf1 = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });
const nf2 = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
const compact = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 });

/** Exact, grouped: 12,345 or 12.35. */
export function fmtNumber(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "–";
  if (Number.isInteger(n)) return nf0.format(n);
  return Math.abs(n) < 10 ? nf2.format(n) : nf1.format(n);
}

/** Short, for axes and tiles: 12.3K. */
export function fmtCompact(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "–";
  if (Math.abs(n) < 1000) return fmtNumber(Math.round(n * 100) / 100);
  return compact.format(n);
}

export function fmtPercent(n: number | null | undefined, digits = 1): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "–";
  return `${n.toFixed(n !== 0 && Math.abs(n) < 10 ? digits : Math.min(digits, 1))}%`;
}

export function fmtDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "–";
  const s = Math.round(seconds);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ${String(m % 60).padStart(2, "0")}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

export function fmtBytes(n: number): string {
  if (!Number.isFinite(n)) return "–";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 ? v : v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

/** Accepts RFC 3339 strings or unix seconds/millis. */
export function toDate(value: string | number | null | undefined): Date | null {
  if (value === null || value === undefined || value === "") return null;
  if (typeof value === "number") return new Date(value < 1e12 ? value * 1000 : value);
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d;
}

const dtf = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
const df = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
const dShort = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const hf = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });

export function fmtDateTime(value: string | number | null | undefined): string {
  const d = toDate(value);
  return d ? dtf.format(d) : "–";
}
export function fmtDate(value: string | number | null | undefined): string {
  const d = toDate(value);
  return d ? df.format(d) : "–";
}
/** Bucket label for an interval: "14:00", "Oct 3", "Oct 2026". */
export function fmtBucket(value: string, interval: string): string {
  const d = toDate(value);
  if (!d) return value;
  if (interval === "hour") return hf.format(d);
  if (interval === "month") return new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric" }).format(d);
  return dShort.format(d);
}

export function fmtRelative(value: string | number | null | undefined, now = Date.now()): string {
  const d = toDate(value);
  if (!d) return "never";
  const s = Math.round((now - d.getTime()) / 1000);
  if (s < 5) return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const days = Math.floor(h / 24);
  if (days < 30) return `${days}d ago`;
  return fmtDate(d.getTime());
}

// ── Date ranges ───────────────────────────────────────────────────────────

export interface RangePreset {
  label: string;
  short: string;
  date_from: string;
  date_to: string | null;
}

export const RANGE_PRESETS: RangePreset[] = [
  { label: "Last hour", short: "1h", date_from: "-1h", date_to: null },
  { label: "Last 24 hours", short: "24h", date_from: "-24h", date_to: null },
  { label: "Today", short: "Today", date_from: "dStart", date_to: null },
  { label: "Last 7 days", short: "7d", date_from: "-7d", date_to: null },
  { label: "Last 14 days", short: "14d", date_from: "-14d", date_to: null },
  { label: "Last 30 days", short: "30d", date_from: "-30d", date_to: null },
  { label: "Last 90 days", short: "90d", date_from: "-90d", date_to: null },
  { label: "This month", short: "MTD", date_from: "mStart", date_to: null },
  { label: "Last 12 months", short: "12m", date_from: "-12m", date_to: null },
  { label: "Year to date", short: "YTD", date_from: "yStart", date_to: null },
  { label: "All time", short: "All", date_from: "all", date_to: null },
];

export function rangeLabel(date_from: string, date_to: string | null | undefined): string {
  const preset = RANGE_PRESETS.find((p) => p.date_from === date_from && (p.date_to ?? null) === (date_to ?? null));
  if (preset) return preset.label;
  const rel = /^-(\d+)([hdwmy])$/.exec(date_from);
  if (rel && !date_to) {
    const unit = { h: "hour", d: "day", w: "week", m: "month", y: "year" }[rel[2]] ?? "day";
    return `Last ${rel[1]} ${unit}${rel[1] === "1" ? "" : "s"}`;
  }
  const a = toDate(date_from);
  const b = toDate(date_to ?? null);
  if (a && b) return `${fmtDate(a.getTime())} – ${fmtDate(b.getTime())}`;
  if (a) return `Since ${fmtDate(a.getTime())}`;
  return date_from;
}

/** Resolve a PostHog-style range to absolute dates (local time). */
export function resolveRange(date_from: string, date_to: string | null | undefined, now = new Date()): [Date, Date] {
  const end = date_to ? toDate(date_to) ?? now : now;
  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  let start: Date;
  const rel = /^-(\d+)([hdwmy])$/.exec(date_from);
  if (rel) {
    const n = Number(rel[1]);
    start = new Date(now);
    switch (rel[2]) {
      case "h":
        start.setHours(start.getHours() - n);
        break;
      case "d":
        start = startOfDay(now);
        start.setDate(start.getDate() - n + 1);
        break;
      case "w":
        start = startOfDay(now);
        start.setDate(start.getDate() - n * 7 + 1);
        break;
      case "m":
        start = startOfDay(now);
        start.setMonth(start.getMonth() - n);
        break;
      default:
        start = startOfDay(now);
        start.setFullYear(start.getFullYear() - n);
    }
  } else if (date_from === "dStart") start = startOfDay(now);
  else if (date_from === "mStart") start = new Date(now.getFullYear(), now.getMonth(), 1);
  else if (date_from === "yStart") start = new Date(now.getFullYear(), 0, 1);
  else if (date_from === "all") {
    start = startOfDay(now);
    start.setDate(start.getDate() - 89);
  } else start = toDate(date_from) ?? startOfDay(now);
  return [start, end];
}

/** A sensible interval for a range (hourly for ≤ 2 days, monthly past a year). */
export function autoInterval(date_from: string, date_to: string | null | undefined): "hour" | "day" | "week" | "month" {
  const [a, b] = resolveRange(date_from, date_to);
  const days = (b.getTime() - a.getTime()) / 86_400_000;
  if (days <= 2) return "hour";
  if (days <= 92) return "day";
  if (days <= 366) return "week";
  return "month";
}

export function initials(name: string): string {
  const clean = name.replace(/@.*/, "").replace(/[^\p{L}\p{N} ._-]/gu, " ");
  const parts = clean.split(/[ ._-]+/).filter(Boolean);
  if (parts.length === 0) return "?";
  return (parts[0][0] + (parts[1]?.[0] ?? "")).toUpperCase();
}
