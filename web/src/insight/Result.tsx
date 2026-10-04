// Renders any InsightResult; every number opens the persons behind it.

import { useMemo, useRef, useState, type ReactNode } from "react";
import type { ActorSelection } from "../types/ActorSelection";
import type { InsightQuery } from "../types/InsightQuery";
import type { InsightResult } from "../types/InsightResult";
import type { PersonSummary } from "../types/PersonSummary";
import type { QueryResponse } from "../types/QueryResponse";
import type { TrendSeries } from "../types/TrendSeries";
import { api, isAbort } from "../lib/api";
import { usePath, useProjectId } from "../lib/context";
import { fmtBucket, fmtCompact, fmtDuration, fmtNumber, fmtPercent, fmtRelative } from "../lib/format";
import { useApi, useDebounced } from "../lib/hooks";
import { eventLabel } from "../lib/properties";
import { navigate } from "../lib/router";
import { breakdownLabel, FunnelChart } from "../charts/Funnel";
import { HBarList, PieChart } from "../charts/Pie";
import { RetentionGrid } from "../charts/Retention";
import { PathsSankey } from "../charts/Sankey";
import { seriesColor } from "../charts/scale";
import { TimeSeriesChart, type ChartSeries } from "../charts/TimeSeries";
import { Icon } from "../ui/icons";
import { Avatar, Empty, ErrorState, Modal, Seg, Skeleton } from "../ui/kit";
import { incomplete, sanitize } from "./defaults";

// ── Running a query ─────────────────────────────────────────────────────

export function useInsightQuery(query: InsightQuery | null, debounceMs = 350) {
  const projectId = useProjectId();
  const ready = query ? incomplete(query) === null : false;
  const clean = useMemo(() => (query && ready ? sanitize(query) : null), [query, ready]);
  const json = clean ? JSON.stringify(clean) : null;
  const key = useDebounced(json, debounceMs);
  const refresh = useRef(false);
  const res = useApi<QueryResponse>(key ? `query:${projectId}:${key}` : null, (signal) => {
    const body = { query: JSON.parse(key as string) as InsightQuery, refresh: refresh.current };
    refresh.current = false;
    return api.query(projectId, body, signal);
  });
  return {
    ...res,
    pending: res.loading || key !== json,
    sent: key ? (JSON.parse(key) as InsightQuery) : null,
    hint: query ? incomplete(query) : null,
    refresh: () => {
      refresh.current = true;
      res.reload();
    },
  };
}

// ── Actors drill-down ───────────────────────────────────────────────────

export interface ActorsTarget {
  selection: ActorSelection;
  title: string;
}

export function ActorsModal({ query, target, onClose }: { query: InsightQuery; target: ActorsTarget; onClose: () => void }) {
  const projectId = useProjectId();
  const path = usePath();
  const [extra, setExtra] = useState<PersonSummary[]>([]);
  const [more, setMore] = useState<{ loading: boolean; hasMore: boolean | null; error: unknown }>({ loading: false, hasMore: null, error: null });
  const { data, error, loading, reload } = useApi(`actors:${projectId}:${JSON.stringify(query)}:${JSON.stringify(target.selection)}`, (signal) =>
    api.actors(projectId, { query: sanitize(query), selection: target.selection, offset: 0, limit: 100 }, signal),
  );
  const persons = [...(data?.persons ?? []), ...extra];
  const hasMore = more.hasMore ?? data?.has_more ?? false;
  const [filter, setFilter] = useState("");
  const shown = filter ? persons.filter((p) => `${p.display_name} ${p.distinct_ids.join(" ")}`.toLowerCase().includes(filter.toLowerCase())) : persons;

  return (
    <Modal title={target.title} onClose={onClose} wide>
      <div className="col gap-12">
        <div className="row">
          <div className="search grow">
            <Icon name="search" size={14} />
            <input className="input" placeholder="Filter loaded persons…" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter persons" />
          </div>
          <span className="muted small num">{data ? `${fmtNumber(persons.length)}${hasMore ? "+" : ""} persons` : ""}</span>
        </div>
        {error ? (
          <ErrorState error={error} retry={reload} />
        ) : !data && loading ? (
          <div className="col" style={{ gap: 10 }}>
            {Array.from({ length: 6 }, (_, i) => (
              <Skeleton key={i} height={30} />
            ))}
          </div>
        ) : persons.length === 0 ? (
          <Empty icon="users" title="No persons here">
            Nobody matches this data point.
          </Empty>
        ) : (
          <div className="card" style={{ boxShadow: "none", border: "1px solid var(--line)" }}>
            <table className="table compact">
              <thead>
                <tr>
                  <th>Person</th>
                  <th>Distinct ID</th>
                  <th className="r">Last seen</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((p) => (
                  <tr
                    key={p.id}
                    className="clickable"
                    onClick={() => {
                      onClose();
                      navigate(path(`persons/${encodeURIComponent(p.id)}`));
                    }}
                  >
                    <td>
                      <div className="row">
                        <Avatar name={p.display_name} id={p.id} />
                        <span className="truncate" style={{ maxWidth: 260 }}>
                          {p.display_name}
                        </span>
                        {p.is_identified && <span className="badge accent">identified</span>}
                      </div>
                    </td>
                    <td className="mono small muted truncate" style={{ maxWidth: 220 }}>
                      {p.distinct_ids[0]}
                    </td>
                    <td className="r muted small">{fmtRelative(p.last_seen)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {more.error ? <ErrorState error={more.error} compact /> : null}
        {hasMore && (
          <button
            className="btn"
            disabled={more.loading}
            onClick={async () => {
              setMore((m) => ({ ...m, loading: true }));
              try {
                const r = await api.actors(projectId, { query: sanitize(query), selection: target.selection, offset: persons.length, limit: 100 });
                setExtra((e) => [...e, ...r.persons]);
                setMore({ loading: false, hasMore: r.has_more, error: null });
              } catch (e) {
                if (!isAbort(e)) setMore({ loading: false, hasMore: true, error: e });
              }
            }}
          >
            {more.loading ? "Loading…" : "Load more"}
          </button>
        )}
      </div>
    </Modal>
  );
}

// ── Result view ─────────────────────────────────────────────────────────

function trendIdentity(series: TrendSeries[]) {
  const slots = new Map<string, number>();
  for (const s of series) {
    if (s.compare) continue;
    const id = `${s.series_index ?? "f"}|${s.breakdown_value ?? ""}`;
    if (!slots.has(id)) slots.set(id, slots.size);
  }
  return (s: TrendSeries) => slots.get(`${s.series_index ?? "f"}|${s.breakdown_value ?? ""}`) ?? 0;
}

const MATH_SHORT: Record<string, string> = {
  dau: "unique users",
  weekly_active: "weekly active",
  monthly_active: "monthly active",
  unique_session: "unique sessions",
  sum: "sum",
  avg: "average",
  min: "min",
  max: "max",
  median: "median",
  p90: "p90",
  p95: "p95",
  p99: "p99",
};

function trendLabel(s: TrendSeries, q?: Extract<InsightQuery, { kind: "TrendsQuery" }>): string {
  let label = s.label;
  const node = q && s.series_index !== null ? q.series[s.series_index] : undefined;
  if (node && !node.custom_name) {
    if (label === node.event || (node.event === null && label === "All events")) label = eventLabel(node.event);
    if (node.math !== "total") label += ` (${MATH_SHORT[node.math] ?? node.math}${node.math_property ? ` of ${node.math_property}` : ""})`;
  }
  if (s.breakdown_value !== null && !label.includes(breakdownLabel(s.breakdown_value))) label += ` · ${breakdownLabel(s.breakdown_value)}`;
  if (s.compare) label += " (previous)";
  return label;
}

function interval(q: InsightQuery): string {
  return "interval" in q ? q.interval : "day";
}

function TrendsView({
  query,
  series,
  compact,
  onSelect,
}: {
  query: Extract<InsightQuery, { kind: "TrendsQuery" }>;
  series: TrendSeries[];
  compact?: boolean;
  onSelect: (t: ActorsTarget) => void;
}) {
  const slot = trendIdentity(series);
  if (series.length === 0 || series.every((s) => s.data.length === 0)) {
    return (
      <Empty icon="trends" title="No matching events in this range">
        Try a wider date range, or check that the event name matches what your app sends.
      </Empty>
    );
  }
  const select = (s: TrendSeries, i: number | null) => {
    if (s.series_index === null) return;
    const day = i === null ? s.days[0] : s.days[i];
    if (!day) return;
    onSelect({
      selection: { type: "TrendsPoint", series_index: s.series_index, day, breakdown_value: s.breakdown_value },
      title: `${trendLabel(s, query)} · ${i === null ? "" : s.labels[i] ?? fmtBucket(day, interval(query))}`,
    });
  };
  const display = query.display;
  const height = compact ? 220 : 340;

  if (display === "BoldNumber") {
    const main = series.find((s) => !s.compare) ?? series[0];
    const prev = series.find((s) => s.compare && s.series_index === main.series_index && s.breakdown_value === main.breakdown_value);
    const delta = prev && prev.aggregated_value ? ((main.aggregated_value - prev.aggregated_value) / Math.abs(prev.aggregated_value)) * 100 : null;
    return (
      <div className="bold-number">
        <div className="value num" title={fmtNumber(main.aggregated_value)} onClick={() => select(main, main.days.length - 1)}>
          {fmtCompact(main.aggregated_value)}
        </div>
        <div className="label">{trendLabel(main, query)}</div>
        {delta !== null && (
          <span className={`delta ${delta > 0 ? "up" : delta < 0 ? "down" : "flat"}`}>
            {delta > 0 ? "▲" : delta < 0 ? "▼" : ""} {fmtPercent(Math.abs(delta))} vs previous period
          </span>
        )}
      </div>
    );
  }

  if (display === "ActionsPie" || display === "ActionsBarValue") {
    const rows = series
      .filter((s) => !s.compare)
      .map((s) => ({ key: `${s.series_index}|${s.breakdown_value}`, label: trendLabel(s, query), value: s.aggregated_value, color: seriesColor(slot(s)), s }));
    rows.sort((a, b) => b.value - a.value);
    const click = (i: number) => select(rows[i].s, rows[i].s.days.length - 1);
    return display === "ActionsPie" ? <PieChart slices={rows} size={compact ? 180 : 240} onSliceClick={click} /> : <HBarList rows={rows} onClick={click} />;
  }

  if (display === "ActionsTable") {
    const labels = series[0]?.labels ?? [];
    return (
      <div className="table-wrap">
        <table className="table compact">
          <thead>
            <tr>
              <th>Series</th>
              <th className="r">Total</th>
              {labels.map((l, i) => (
                <th key={i} className="r">
                  {l}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {series.map((s, si) => (
              <tr key={si}>
                <td>
                  <div className="row nowrap">
                    <span className="swatch" style={{ background: seriesColor(slot(s)), opacity: s.compare ? 0.5 : 1 }} />
                    {trendLabel(s, query)}
                  </div>
                </td>
                <td className="r">
                  <b>{fmtNumber(s.aggregated_value)}</b>
                </td>
                {s.data.map((v, i) => (
                  <td key={i} className="r">
                    <button className="link" style={{ border: 0, background: "none", padding: 0, font: "inherit", color: "inherit" }} onClick={() => select(s, i)}>
                      {fmtNumber(v)}
                    </button>
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }

  const kind = display === "ActionsAreaGraph" ? "area" : display === "ActionsBar" ? (query.breakdown ? "stacked" : "bar") : "line";
  const base = series.find((s) => !s.compare) ?? series[0];
  const labels = base.labels.length ? base.labels : base.days.map((d) => fmtBucket(d, interval(query)));
  const chartSeries: ChartSeries[] = series.map((s, i) => ({
    key: `${i}`,
    label: trendLabel(s, query),
    color: seriesColor(slot(s)),
    data: s.data,
    dashed: !!s.compare,
  }));
  return (
    <TimeSeriesChart
      kind={kind}
      height={height}
      labels={labels}
      series={chartSeries}
      tooltipTitle={(i) => (base.days[i] ? fmtBucket(base.days[i], interval(query)) + (interval(query) === "hour" ? "" : "") : labels[i])}
      onPointClick={(si, i) => select(series[si], i)}
      legend={!compact || series.length <= 6}
    />
  );
}

export function InsightResultView({
  query,
  result,
  compact = false,
}: {
  query: InsightQuery;
  result: InsightResult;
  compact?: boolean;
}) {
  const [target, setTarget] = useState<ActorsTarget | null>(null);
  const [funnelTab, setFunnelTab] = useState<"steps" | "time">("steps");
  let body: ReactNode = null;

  if (result.kind === "Trends" && query.kind === "TrendsQuery") {
    body = <TrendsView query={query} series={result.series} compact={compact} onSelect={setTarget} />;
  } else if (result.kind === "Funnels") {
    if (result.steps.length === 0 || result.steps[0].count === 0) {
      body = (
        <Empty icon="funnel" title="Nobody entered this funnel">
          No person performed the first step in this date range.
        </Empty>
      );
    } else {
      body = (
        <div className="col gap-16">
          {!compact && result.time_to_convert.length > 0 && (
            <Seg
              label="Funnel view"
              value={funnelTab}
              onChange={setFunnelTab}
              options={[
                { value: "steps", label: "Conversion steps" },
                { value: "time", label: "Time to convert" },
              ]}
            />
          )}
          {funnelTab === "steps" || compact ? (
            <FunnelChart
              compact={compact}
              steps={result.steps}
              breakdowns={result.breakdowns}
              onSelect={(step, converted, breakdown_value, title) => setTarget({ selection: { type: "FunnelStep", step, converted, breakdown_value }, title })}
            />
          ) : (
            <TimeSeriesChart
              kind="bar"
              height={300}
              labels={result.time_to_convert.map((b) => `${fmtDuration(b.from_s)}`)}
              series={[{ key: "ttc", label: "Persons converted", color: "var(--s1)", data: result.time_to_convert.map((b) => b.count) }]}
              tooltipTitle={(i) => `${fmtDuration(result.time_to_convert[i].from_s)} – ${fmtDuration(result.time_to_convert[i].to_s)}`}
              legend={false}
            />
          )}
        </div>
      );
    }
  } else if (result.kind === "Retention") {
    body =
      result.cohorts.length === 0 ? (
        <Empty icon="retention" title="No cohorts yet">
          Nobody performed the target event in this range.
        </Empty>
      ) : (
        <RetentionGrid
          compact={compact}
          period={result.period}
          cohorts={result.cohorts}
          onCellClick={(c, i) => setTarget({ selection: { type: "RetentionCell", cohort_date: c.date, interval: i }, title: `${c.label} cohort · period ${i}` })}
        />
      );
  } else if (result.kind === "Lifecycle") {
    const statuses = [
      { key: "new", label: "New", data: result.new },
      { key: "returning", label: "Returning", data: result.returning },
      { key: "resurrecting", label: "Resurrecting", data: result.resurrecting },
      { key: "dormant", label: "Dormant", data: result.dormant },
    ] as const;
    const labels = result.labels.length ? result.labels : result.days.map((d) => fmtBucket(d, interval(query)));
    body = (
      <TimeSeriesChart
        kind="stacked"
        height={compact ? 220 : 340}
        labels={labels}
        series={statuses.map((s, i) => ({ key: s.key, label: s.label, color: seriesColor(i), data: s.data.map(Number) }))}
        onPointClick={(si, i) => {
          const s = statuses[si];
          const day = result.days[i];
          if (day) setTarget({ selection: { type: "LifecycleCell", status: s.key, day }, title: `${s.label} · ${labels[i]}` });
        }}
      />
    );
  } else if (result.kind === "Stickiness") {
    const labels = result.series[0]?.labels ?? [];
    body =
      result.series.length === 0 ? (
        <Empty icon="stickiness" title="No active users in this range" />
      ) : (
        <TimeSeriesChart
          kind="bar"
          height={compact ? 220 : 340}
          labels={labels}
          series={result.series.map((s, i) => ({ key: String(i), label: s.label, color: seriesColor(i), data: s.data.map(Number) }))}
          onPointClick={(si, i) => {
            const s = result.series[si];
            setTarget({ selection: { type: "StickinessBar", series_index: s.series_index, intervals: i + 1 }, title: `${s.label} · active ${labels[i] ?? i + 1}` });
          }}
        />
      );
  } else if (result.kind === "Paths") {
    body =
      result.links.length === 0 ? (
        <Empty icon="paths" title="No paths found">
          No sequences of {query.kind === "PathsQuery" && query.paths_type === "custom_events" ? "custom events" : "pageviews"} in this range.
        </Empty>
      ) : (
        <PathsSankey links={result.links} />
      );
  } else if (result.kind === "Sql") {
    body = <SqlTable columns={result.columns} types={result.types} rows={result.rows} truncated={result.truncated} compact={compact} />;
  } else {
    body = <Empty icon="alert" title="Unexpected result shape" />;
  }

  return (
    <>
      {body}
      {target && <ActorsModal query={query} target={target} onClose={() => setTarget(null)} />}
    </>
  );
}

function cell(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function toCsv(columns: string[], rows: unknown[][]): string {
  const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
  return [columns.map(esc).join(","), ...rows.map((r) => r.map((v) => esc(cell(v))).join(","))].join("\n");
}

export function SqlTable({ columns, types, rows, truncated, compact }: { columns: string[]; types: string[]; rows: unknown[][]; truncated: boolean; compact?: boolean }) {
  if (columns.length === 0) return <Empty icon="table" title="The query returned no columns" />;
  return (
    <div className="col gap-12">
      {!compact && (
        <div className="row">
          <span className="muted small grow num">
            {fmtNumber(rows.length)} rows{truncated && " · truncated at the row cap"}
          </span>
          <button
            className="btn small"
            onClick={() => {
              const blob = new Blob([toCsv(columns, rows)], { type: "text/csv" });
              const a = document.createElement("a");
              a.href = URL.createObjectURL(blob);
              a.download = "hoglet-query.csv";
              a.click();
              URL.revokeObjectURL(a.href);
            }}
          >
            Export CSV
          </button>
        </div>
      )}
      <div className="table-wrap" style={{ maxHeight: compact ? 260 : 560, overflow: "auto", border: "1px solid var(--line)", borderRadius: 8 }}>
        <table className="table compact">
          <thead>
            <tr>
              {columns.map((c, i) => (
                <th key={i} className={/int|float|double|decimal|number/i.test(types[i] ?? "") ? "r" : undefined}>
                  {c}
                  <span className="muted" style={{ textTransform: "none", fontWeight: 400, marginLeft: 6 }}>
                    {types[i]}
                  </span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r, ri) => (
              <tr key={ri}>
                {r.map((v, ci) => (
                  <td key={ci} className={`${typeof v === "number" ? "r " : ""}mono small`} style={{ maxWidth: 360, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={cell(v)}>
                    {v === null ? <span className="muted">null</span> : typeof v === "number" ? fmtNumber(v) : cell(v)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** Skeleton sized like a chart so nothing shifts when results land. */
export function ChartSkeleton({ height = 340 }: { height?: number }) {
  return (
    <div className="col" style={{ height, justifyContent: "flex-end", gap: 8 }} aria-busy="true" aria-label="Loading result">
      <div className="row" style={{ alignItems: "flex-end", gap: 6, height: height - 40 }}>
        {Array.from({ length: 18 }, (_, i) => (
          <Skeleton key={i} height={`${30 + ((i * 37) % 60)}%`} style={{ flex: 1 }} />
        ))}
      </div>
      <Skeleton height={10} width="100%" />
    </div>
  );
}
