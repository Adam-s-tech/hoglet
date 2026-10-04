// Web analytics: one screen, Plausible-grade. Every row filters the page.

import { useMemo, useState } from "react";
import type { PropertyFilter } from "../types/PropertyFilter";
import type { WebDimension } from "../types/WebDimension";
import type { WebMetric } from "../types/WebMetric";
import type { WebQuery } from "../types/WebQuery";
import { api } from "../lib/api";
import { usePath, useProjectId } from "../lib/context";
import { autoInterval, fmtBucket, fmtCompact, fmtDuration, fmtNumber, fmtPercent } from "../lib/format";
import { useApi, useLocalStorage } from "../lib/hooks";
import { describeFilter } from "../lib/properties";
import { Link } from "../lib/router";
import { TimeSeriesChart } from "../charts/TimeSeries";
import { DateRangePicker, type RangeValue } from "../ui/DateRange";
import { Icon } from "../ui/icons";
import { Empty, ErrorState, LoadingBar, Modal, Skeleton } from "../ui/kit";
import { PropertyFilters } from "../insight/pickers";

const DIM_PROPERTY: Record<WebDimension, string> = {
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

const PANELS: { title: string; tabs: { dim: WebDimension; label: string; col: string }[] }[] = [
  {
    title: "Pages",
    tabs: [
      { dim: "page", label: "Top pages", col: "Page" },
      { dim: "entry_page", label: "Entry pages", col: "Entry page" },
      { dim: "exit_page", label: "Exit pages", col: "Exit page" },
    ],
  },
  {
    title: "Sources",
    tabs: [
      { dim: "referring_domain", label: "Referrers", col: "Referring domain" },
      { dim: "utm_source", label: "Source", col: "UTM source" },
      { dim: "utm_medium", label: "Medium", col: "UTM medium" },
      { dim: "utm_campaign", label: "Campaign", col: "UTM campaign" },
    ],
  },
  {
    title: "Devices",
    tabs: [
      { dim: "browser", label: "Browsers", col: "Browser" },
      { dim: "os", label: "OS", col: "Operating system" },
      { dim: "device_type", label: "Devices", col: "Device type" },
    ],
  },
  { title: "Locations", tabs: [{ dim: "country", label: "Countries", col: "Country" }] },
];

let regionNames: Intl.DisplayNames | null = null;
function countryName(code: string): string {
  try {
    regionNames ??= new Intl.DisplayNames(undefined, { type: "region" });
    return regionNames.of(code.toUpperCase()) ?? code;
  } catch {
    return code;
  }
}

function displayValue(dim: WebDimension, value: string): string {
  if (value === "" || value === "$$_none") return dim === "referring_domain" ? "Direct / none" : "(none)";
  if (dim === "country") return countryName(value);
  return value;
}

function Delta({ metric, invert }: { metric: WebMetric; invert?: boolean }) {
  if (metric.previous === null || metric.previous === 0) return <span className="delta flat">no prior data</span>;
  const pct = ((metric.value - metric.previous) / Math.abs(metric.previous)) * 100;
  if (Math.abs(pct) < 0.05) return <span className="delta flat">no change</span>;
  const good = invert ? pct < 0 : pct > 0;
  return (
    <span className={`delta ${good ? "up" : "down"}`} title={`Previous period: ${fmtNumber(metric.previous)}`}>
      <Icon name={pct > 0 ? "arrowUp" : "arrowDown"} size={11} strokeWidth={2.2} />
      {fmtPercent(Math.abs(pct))}
    </span>
  );
}

function BreakdownPanel({
  panel,
  query,
  onFilter,
}: {
  panel: (typeof PANELS)[number];
  query: WebQuery;
  onFilter: (dim: WebDimension, value: string) => void;
}) {
  const projectId = useProjectId();
  const [tab, setTab] = useState(0);
  const [all, setAll] = useState(false);
  const t = panel.tabs[tab];
  const json = JSON.stringify(query);
  const { data, error, loading, reload } = useApi(`web-bd:${projectId}:${t.dim}:${json}`, (s) => api.webBreakdown(projectId, query, t.dim, 10, s), { keepPrevious: false });
  const rows = data?.rows ?? [];
  const max = Math.max(1, ...rows.map((r) => r.visitors));
  const pageLike = t.dim === "page" || t.dim === "entry_page" || t.dim === "exit_page";

  return (
    <div className="card" style={{ display: "flex", flexDirection: "column", minHeight: 420 }}>
      <div className="card-head" style={{ paddingBottom: 0, borderBottom: 0, alignItems: "flex-end" }}>
        <h3 style={{ flex: "none", marginBottom: 10 }}>{panel.title}</h3>
        <span className="spacer" />
        {panel.tabs.length > 1 && (
          <div className="tabs" role="tablist" style={{ marginBottom: 0, borderBottom: 0 }}>
            {panel.tabs.map((x, i) => (
              <button key={x.dim} role="tab" aria-selected={i === tab} onClick={() => setTab(i)} style={{ height: 32, fontSize: 12.5 }}>
                {x.label}
              </button>
            ))}
          </div>
        )}
      </div>
      <div className="blist-head" style={{ borderTop: "1px solid var(--line)" }}>
        <span style={{ flex: 1 }}>{t.col}</span>
        <span style={{ width: 68, textAlign: "right" }}>Visitors</span>
        <span style={{ width: 68, textAlign: "right" }}>{pageLike ? "Views" : "Events"}</span>
      </div>
      <div className="blist" style={{ flex: 1, position: "relative" }}>
        <LoadingBar show={loading && !!data} />
        {error ? (
          <ErrorState error={error} retry={reload} compact />
        ) : !data ? (
          <div className="col" style={{ padding: "4px 16px", gap: 12 }}>
            {Array.from({ length: 8 }, (_, i) => (
              <Skeleton key={i} height={18} width={`${90 - i * 9}%`} />
            ))}
          </div>
        ) : rows.length === 0 ? (
          <div className="muted small" style={{ padding: "24px 16px", textAlign: "center" }}>
            No data for this range.
          </div>
        ) : (
          rows.map((r) => (
            <button key={r.value} className="brow" onClick={() => onFilter(t.dim, r.value)} title={`Filter by ${displayValue(t.dim, r.value)}`}>
              <span className="bar" style={{ width: `${(r.visitors / max) * 100}%` }} />
              <span className="v">{displayValue(t.dim, r.value)}</span>
              <span className="n strong" style={{ width: 60 }}>
                {fmtCompact(r.visitors)}
              </span>
              <span className="n" style={{ width: 60 }}>
                {fmtCompact(r.views)}
              </span>
            </button>
          ))
        )}
      </div>
      {rows.length >= 10 && (
        <div style={{ padding: "6px 8px 10px", textAlign: "center" }}>
          <button className="btn ghost small" onClick={() => setAll(true)}>
            View all <Icon name="arrowRight" size={12} />
          </button>
        </div>
      )}
      {all && <BreakdownAll dim={t.dim} title={t.label} query={query} onClose={() => setAll(false)} onFilter={onFilter} />}
    </div>
  );
}

function BreakdownAll({ dim, title, query, onClose, onFilter }: { dim: WebDimension; title: string; query: WebQuery; onClose: () => void; onFilter: (dim: WebDimension, value: string) => void }) {
  const projectId = useProjectId();
  const [search, setSearch] = useState("");
  const { data, error } = useApi(`web-bd:${projectId}:${dim}:all:${JSON.stringify(query)}`, (s) => api.webBreakdown(projectId, query, dim, 200, s));
  const rows = (data?.rows ?? []).filter((r) => !search || displayValue(dim, r.value).toLowerCase().includes(search.toLowerCase()));
  return (
    <Modal title={title} onClose={onClose} wide>
      <div className="col gap-12">
        <div className="search">
          <Icon name="search" size={14} />
          <input className="input" placeholder="Search…" value={search} onChange={(e) => setSearch(e.target.value)} aria-label="Search" />
        </div>
        {error ? <ErrorState error={error} compact /> : null}
        <table className="table compact">
          <thead>
            <tr>
              <th>{title}</th>
              <th className="r">Visitors</th>
              <th className="r">Views</th>
              {rows.some((r) => r.bounce_rate !== null) && <th className="r">Bounce rate</th>}
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr
                key={r.value}
                className="clickable"
                onClick={() => {
                  onFilter(dim, r.value);
                  onClose();
                }}
              >
                <td className="truncate" style={{ maxWidth: 420 }}>
                  {displayValue(dim, r.value)}
                </td>
                <td className="r">{fmtNumber(r.visitors)}</td>
                <td className="r">{fmtNumber(r.views)}</td>
                {rows.some((x) => x.bounce_rate !== null) && <td className="r">{fmtPercent(r.bounce_rate)}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Modal>
  );
}

export function WebPage() {
  const projectId = useProjectId();
  const path = usePath();
  const [range, setRange] = useLocalStorage<RangeValue>("hoglet.web.range", { date_from: "-7d", date_to: null });
  const [filters, setFilters] = useState<PropertyFilter[]>([]);
  const [metric, setMetric] = useState<"visitors" | "pageviews">("visitors");
  const complete = filters.filter((f) => f.key);
  const query: WebQuery = useMemo(
    () => ({ date_from: range.date_from, date_to: range.date_to, interval: autoInterval(range.date_from, range.date_to), properties: complete }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [range.date_from, range.date_to, JSON.stringify(complete)],
  );
  const { data, error, loading, reload } = useApi(`web-ov:${projectId}:${JSON.stringify(query)}`, (s) => api.webOverview(projectId, query, s), { pollMs: 30_000 });

  const addFilter = (dim: WebDimension, value: string) => {
    const key = DIM_PROPERTY[dim];
    const next = filters.filter((f) => f.key !== key);
    next.push(value === "" || value === "$$_none" ? { key, type: "event", operator: "is_not_set", value: null } : { key, type: "event", operator: "exact", value: [value] });
    setFilters(next);
  };

  const tiles: { key: string; label: string; value: string; metric: WebMetric | undefined; invert?: boolean; chart?: "visitors" | "pageviews" }[] = [
    { key: "visitors", label: "Visitors", value: fmtCompact(data?.visitors.value), metric: data?.visitors, chart: "visitors" },
    { key: "pageviews", label: "Pageviews", value: fmtCompact(data?.pageviews.value), metric: data?.pageviews, chart: "pageviews" },
    { key: "sessions", label: "Sessions", value: fmtCompact(data?.sessions.value), metric: data?.sessions },
    { key: "bounce", label: "Bounce rate", value: fmtPercent(data?.bounce_rate.value), metric: data?.bounce_rate, invert: true },
    { key: "duration", label: "Session duration", value: fmtDuration(data?.session_duration_s.value), metric: data?.session_duration_s },
  ];
  const noData = data && data.pageviews.value === 0 && data.pageviews.previous === null && complete.length === 0;

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>Web analytics</h1>
          <div className="sub">Visitors, sources and pages from your pageviews.</div>
        </div>
        <div className="actions">
          {data && (
            <span className="badge good" style={{ height: 26, padding: "0 10px", fontSize: 12.5 }} title="Persons with a pageview in the last 5 minutes">
              <span className="dot live" /> {fmtNumber(data.live_visitors)} live {data.live_visitors === 1 ? "visitor" : "visitors"}
            </span>
          )}
          <DateRangePicker value={range} onChange={setRange} />
        </div>
      </div>

      <div className="toolbar">
        <PropertyFilters value={filters} onChange={setFilters} sources={["event", "person"]} />
        {complete.length > 0 && (
          <button className="btn ghost small" onClick={() => setFilters([])}>
            Clear filters
          </button>
        )}
      </div>

      {error && !data ? (
        <div className="card">
          <ErrorState error={error} retry={reload} />
        </div>
      ) : noData ? (
        <div className="card">
          <Empty icon="globe" title="No pageviews yet" action={<Link className="btn primary" to={path("onboarding")}>Connect your site</Link>}>
            Add posthog-js to your site with <code>api_host</code> pointing at this server. Pageviews are captured automatically and appear here within seconds.
          </Empty>
        </div>
      ) : (
        <>
          <div className="card" style={{ position: "relative", marginBottom: 16 }}>
            <LoadingBar show={loading && !!data} />
            <div className="kpis">
              {tiles.map((t) => {
                const inner = (
                  <>
                    <span className="k-label">{t.label}</span>
                    {data ? <span className="k-value num">{t.value}</span> : <Skeleton height={30} width={90} style={{ margin: "1px 0" }} />}
                    {t.metric ? <Delta metric={t.metric} invert={t.invert} /> : <Skeleton height={12} width={60} />}
                  </>
                );
                return t.chart ? (
                  <button key={t.key} className="kpi" aria-pressed={metric === t.chart} onClick={() => setMetric(t.chart!)}>
                    {inner}
                  </button>
                ) : (
                  <div key={t.key} className="kpi">
                    {inner}
                  </div>
                );
              })}
            </div>
            <div style={{ padding: "18px 18px 12px" }}>
              {data ? (
                <TimeSeriesChart
                  kind="area"
                  height={260}
                  labels={data.days.map((d) => fmtBucket(d, data.interval))}
                  series={[
                    {
                      key: metric,
                      label: metric === "visitors" ? "Visitors" : "Pageviews",
                      color: "var(--s1)",
                      data: (metric === "visitors" ? data.visitors_series : data.pageviews_series).map(Number),
                    },
                  ]}
                  legend={false}
                />
              ) : (
                <Skeleton height={260} />
              )}
            </div>
          </div>

          <div className="grid-2">
            {PANELS.map((p) => (
              <BreakdownPanel key={p.title} panel={p} query={query} onFilter={addFilter} />
            ))}
          </div>
          {complete.length > 0 && (
            <p className="muted small mt-16">
              Filtered by {complete.map((f) => { const d = describeFilter(f); return `${d.key} ${d.op} ${d.value}`; }).join(", ")}.
            </p>
          )}
        </>
      )}
    </div>
  );
}
