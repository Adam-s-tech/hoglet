// Renders any InsightResult; every number opens the persons behind it.

import { infiniteQueryOptions, keepPreviousData, useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ActorSelection } from "@/types/ActorSelection";
import type { InsightQuery } from "@/types/InsightQuery";
import type { InsightResult } from "@/types/InsightResult";
import type { PersonSummary } from "@/types/PersonSummary";
import type { QueryResponse } from "@/types/QueryResponse";
import type { TrendSeries } from "@/types/TrendSeries";
import { Avatar } from "@/components/avatar";
import { AppDialog } from "@/components/dialogs";
import { columnHelper, DataTable } from "@/components/data-table";
import { Empty, ErrorState, Skeleton } from "@/components/feedback";
import { Seg } from "@/components/controls";
import { SearchInput } from "@/components/page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { api, isAbort } from "@/lib/api";
import { usePath, useProjectId } from "@/lib/context";
import { fmtBucket, fmtCompact, fmtDuration, fmtNumber, fmtRelative } from "@/lib/format";
import { useDebounced } from "@/lib/hooks";
import { navigate } from "@/lib/nav";
import { eventLabel } from "@/lib/properties";
import { insightResultQuery, qk } from "@/lib/queries";
import { breakdownLabel, FunnelChart } from "@/charts/Funnel";
import { HBarList, PieChart } from "@/charts/Pie";
import { Delta, Swatch } from "@/charts/parts";
import { RetentionGrid } from "@/charts/Retention";
import { PathsSankey } from "@/charts/Sankey";
import { seriesColor } from "@/charts/scale";
import { TimeSeriesChart, type ChartSeries } from "@/charts/TimeSeries";
import { incomplete, sanitize } from "./defaults";

// ── Running a query ─────────────────────────────────────────────────────

/** Placeholder request for the disabled (nothing to run) state; never sent. */
const IDLE: InsightQuery = { kind: "SqlQuery", query: "" };

/**
 * Runs an insight on TanStack Query. The query is sanitized, then debounced
 * into the cache key, so typing never fires a request per keystroke and a
 * superseded request is aborted. While the next result loads the previous one
 * stays on screen (`pending` is true, `sent` is the query that produced `data`).
 * `refresh` recomputes server-side (bypassing its cache) and writes the answer
 * into the query cache; `reload` retries the current key (error state).
 */
export function useInsightQuery(query: InsightQuery | null, debounceMs = 350) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  const ready = query ? incomplete(query) === null : false;
  const clean = useMemo(() => (query && ready ? sanitize(query) : null), [query, ready]);
  const json = clean ? JSON.stringify(clean) : null;
  const key = useDebounced(json, debounceMs);
  const target = useMemo(() => (key ? (JSON.parse(key) as InsightQuery) : null), [key]);

  const options = insightResultQuery(projectId, { query: target ?? IDLE, refresh: false });
  const res = useQuery({ ...options, enabled: target !== null, placeholderData: keepPreviousData });

  // The query that produced `res.data`: differs from `target` only while a new
  // result is loading behind the previous one.
  const shown = useRef<InsightQuery | null>(null);
  if (res.data && !res.isPlaceholderData) shown.current = target;
  const sent = res.data ? (res.isPlaceholderData ? shown.current : target) : target;

  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<unknown>(null);
  const abort = useRef<AbortController | null>(null);
  useEffect(() => {
    setRefreshError(null);
    return () => abort.current?.abort();
  }, [key]);

  const queryKey = options.queryKey;
  const refresh = useCallback(() => {
    if (!target) return;
    abort.current?.abort();
    const ctl = new AbortController();
    abort.current = ctl;
    setRefreshing(true);
    setRefreshError(null);
    api
      .query(projectId, { query: target, refresh: true }, ctl.signal)
      .then((fresh: QueryResponse) => {
        queryClient.setQueryData(queryKey, fresh);
      })
      .catch((e: unknown) => {
        if (!isAbort(e)) setRefreshError(e);
      })
      .finally(() => {
        if (abort.current === ctl) {
          abort.current = null;
          setRefreshing(false);
        }
      });
  }, [projectId, queryClient, queryKey, target]);

  const { refetch } = res;
  const reload = useCallback(() => {
    setRefreshError(null);
    void refetch();
  }, [refetch]);

  return {
    data: res.data,
    error: refreshError ?? res.error,
    loading: res.isFetching || refreshing,
    pending: res.isFetching || refreshing || key !== json,
    sent,
    hint: query ? incomplete(query) : null,
    refresh,
    reload,
  };
}

// ── Actors drill-down ───────────────────────────────────────────────────

export interface ActorsTarget {
  selection: ActorSelection;
  title: string;
}

const ACTORS_PAGE = 100;
/** Hard cap on persons loaded into the dialog: ACTORS_PAGE x ACTORS_MAX_PAGES. */
const ACTORS_MAX_PAGES = 10;
/** Past this many matching rows the table virtualizes. */
const ACTORS_VIRTUALIZE_AT = 30;

const actorsPagesQuery = (pid: string, query: InsightQuery, selection: ActorSelection) =>
  infiniteQueryOptions({
    queryKey: [...qk.query(pid), "actors-pages", JSON.stringify(query), JSON.stringify(selection)] as const,
    queryFn: ({ signal, pageParam }) => api.actors(pid, { query, selection, offset: pageParam, limit: ACTORS_PAGE }, signal),
    initialPageParam: 0,
    getNextPageParam: (last, all) => (last.has_more && all.length < ACTORS_MAX_PAGES ? all.length * ACTORS_PAGE : undefined),
    staleTime: 60_000,
  });

function PersonName({ p, onClose }: { p: PersonSummary; onClose: () => void }) {
  const path = usePath();
  return (
    <div className="flex items-center gap-2">
      <Avatar name={p.display_name} id={p.id} />
      <Link
        to={path(`persons/${encodeURIComponent(p.id)}`)}
        className="max-w-65 truncate font-medium hover:text-brand-foreground hover:underline"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        {p.display_name}
      </Link>
      {p.is_identified && <Badge variant="secondary">identified</Badge>}
    </div>
  );
}

const personCol = columnHelper<PersonSummary>();

export function ActorsModal({ query, target, onClose }: { query: InsightQuery; target: ActorsTarget; onClose: () => void }) {
  const projectId = useProjectId();
  const clean = useMemo(() => sanitize(query), [query]);
  const q = useInfiniteQuery(actorsPagesQuery(projectId, clean, target.selection));
  const [filter, setFilter] = useState("");

  const persons = useMemo(() => {
    const seen = new Set<string>();
    return (q.data?.pages.flatMap((pg) => pg.persons) ?? []).filter((p) => !seen.has(p.id) && seen.add(p.id));
  }, [q.data]);
  const needle = filter.trim().toLowerCase();
  const shown = useMemo(
    () => (needle ? persons.filter((p) => `${p.display_name} ${p.distinct_ids.join(" ")}`.toLowerCase().includes(needle)) : persons),
    [persons, needle],
  );
  const lastPage = q.data?.pages[q.data.pages.length - 1];
  const capped = !!lastPage?.has_more && !q.hasNextPage;

  const columns = useMemo(
    () => [
      personCol.accessor("display_name", { header: "Person", cell: ({ row }) => <PersonName p={row.original} onClose={onClose} /> }),
      personCol.accessor((p) => p.distinct_ids[0] ?? "", {
        id: "distinct_id",
        header: "Distinct ID",
        cell: (c) => <span className="block max-w-55 truncate font-mono text-xs text-muted-foreground">{c.getValue()}</span>,
      }),
      personCol.accessor((p) => (p.last_seen ? Date.parse(p.last_seen) : 0), {
        id: "last_seen",
        header: "Last seen",
        cell: ({ row }) => <span className="text-xs text-muted-foreground">{row.original.last_seen ? fmtRelative(row.original.last_seen) : "–"}</span>,
        meta: { align: "right" },
      }),
    ],
    [onClose],
  );

  return (
    <AppDialog title={target.title} onClose={onClose} wide>
      <div className="flex flex-col gap-3">
        <div className="flex items-center gap-3">
          <SearchInput
            wrapperClassName="flex-1"
            placeholder="Filter loaded persons…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label="Filter persons"
          />
          <span className="num text-xs text-muted-foreground" aria-live="polite">
            {q.data ? `${fmtNumber(needle ? shown.length : persons.length)}${q.hasNextPage || capped ? "+" : ""} persons` : ""}
          </span>
        </div>
        {q.error && !q.data ? (
          <ErrorState error={q.error} retry={() => void q.refetch()} />
        ) : q.isPending ? (
          <div className="flex flex-col gap-2.5" aria-busy="true" aria-label="Loading persons">
            {Array.from({ length: 6 }, (_, i) => (
              <Skeleton key={i} className="h-7" />
            ))}
          </div>
        ) : persons.length === 0 ? (
          <Empty icon="users" title="No persons here">
            Nobody matches this data point.
          </Empty>
        ) : shown.length === 0 ? (
          <Empty icon="search" title="No loaded person matches">
            {q.hasNextPage ? "Load more persons, or clear the filter." : "Clear the filter to see everyone."}
          </Empty>
        ) : (
          <div className="overflow-hidden rounded-lg ring-1 ring-foreground/10">
            <DataTable
              label="Persons behind this data point"
              columns={columns}
              data={shown}
              getRowId={(p) => p.id}
              dense
              sortable
              virtualize={shown.length > ACTORS_VIRTUALIZE_AT ? { maxHeight: 380 } : undefined}
              onRowClick={(p) => {
                onClose();
                navigate(`/project/${encodeURIComponent(projectId)}/persons/${encodeURIComponent(p.id)}`);
              }}
            />
          </div>
        )}
        {q.error && q.data ? <ErrorState error={q.error} compact /> : null}
        {capped && <p className="text-xs text-muted-foreground">Showing the first {fmtNumber(ACTORS_PAGE * ACTORS_MAX_PAGES)} persons.</p>}
        {q.hasNextPage && (
          <div>
            <Button variant="outline" disabled={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()}>
              {q.isFetchingNextPage ? "Loading…" : "Load more"}
            </Button>
          </div>
        )}
      </div>
    </AppDialog>
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

type TrendsQ = Extract<InsightQuery, { kind: "TrendsQuery" }>;

function trendLabel(s: TrendSeries, q?: TrendsQ): string {
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

const seriesCol = columnHelper<{ s: TrendSeries; si: number }>();
type SeriesRow = { s: TrendSeries; si: number };

/** Trends as a table: one row per series, one column per bucket. Every cell opens its persons. */
function TrendsTable({
  query,
  series,
  slot,
  onSelect,
}: {
  query: TrendsQ;
  series: TrendSeries[];
  slot: (s: TrendSeries) => number;
  onSelect: (s: TrendSeries, i: number) => void;
}) {
  const labels = useMemo(() => series[0]?.labels ?? [], [series]);
  const rows = useMemo<SeriesRow[]>(() => series.map((s, si) => ({ s, si })), [series]);
  const columns = useMemo(
    () => [
      seriesCol.accessor((r) => trendLabel(r.s, query), {
        id: "series",
        header: "Series",
        cell: ({ row }) => (
          <span className="flex items-center gap-2 whitespace-nowrap">
            <Swatch color={seriesColor(slot(row.original.s))} faded={!!row.original.s.compare} />
            {trendLabel(row.original.s, query)}
          </span>
        ),
      }),
      seriesCol.accessor((r) => r.s.aggregated_value, {
        id: "total",
        header: "Total",
        sortFn: "basic",
        cell: (c) => <b className="num font-semibold">{fmtNumber(c.getValue())}</b>,
        meta: { align: "right" },
      }),
      ...labels.map((l, i) =>
        seriesCol.accessor((r) => r.s.data[i] ?? 0, {
          id: `b${i}`,
          header: l,
          sortFn: "basic",
          cell: ({ row, getValue }) => (
            <button
              type="button"
              className="num rounded-sm hover:text-brand-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
              title="See persons"
              onClick={() => onSelect(row.original.s, i)}
            >
              {fmtNumber(getValue())}
            </button>
          ),
          meta: { align: "right" },
        }),
      ),
    ],
    [labels, query, slot, onSelect],
  );
  return (
    <div className="overflow-hidden rounded-lg ring-1 ring-foreground/10">
      <DataTable label="Trends by period" columns={columns} data={rows} getRowId={(r) => String(r.si)} dense sortable />
    </div>
  );
}

function TrendsView({ query, series, compact, onSelect }: { query: TrendsQ; series: TrendSeries[]; compact?: boolean; onSelect: (t: ActorsTarget) => void }) {
  const slot = trendIdentity(series);
  const select = useCallback(
    (s: TrendSeries, i: number | null) => {
      if (s.series_index === null) return;
      const day = i === null ? s.days[0] : s.days[i];
      if (!day) return;
      onSelect({
        selection: { type: "TrendsPoint", series_index: s.series_index, day, breakdown_value: s.breakdown_value },
        title: `${trendLabel(s, query)} · ${i === null ? "" : (s.labels[i] ?? fmtBucket(day, interval(query)))}`,
      });
    },
    [onSelect, query],
  );
  if (series.length === 0 || series.every((s) => s.data.length === 0)) {
    return (
      <Empty icon="trends" title="No matching events in this range">
        Try a wider date range, or check that the event name matches what your app sends.
      </Empty>
    );
  }
  const display = query.display;
  const height = compact ? 220 : 340;

  if (display === "BoldNumber") {
    const main = series.find((s) => !s.compare) ?? series[0];
    const prev = series.find((s) => s.compare && s.series_index === main.series_index && s.breakdown_value === main.breakdown_value);
    const delta = prev && prev.aggregated_value ? ((main.aggregated_value - prev.aggregated_value) / Math.abs(prev.aggregated_value)) * 100 : null;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-1.5 p-6">
        <button
          type="button"
          className="num rounded-md text-[clamp(40px,7vw,76px)] leading-none font-semibold tracking-tight focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
          title={`${fmtNumber(main.aggregated_value)} · click to see persons`}
          aria-label={`${fmtNumber(main.aggregated_value)} ${trendLabel(main, query)}. Show persons.`}
          onClick={() => select(main, main.days.length - 1)}
        >
          {fmtCompact(main.aggregated_value)}
        </button>
        <div className="text-muted-foreground">{trendLabel(main, query)}</div>
        {delta !== null && <Delta value={delta} suffix="vs previous period" />}
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

  if (display === "ActionsTable") return <TrendsTable query={query} series={series} slot={slot} onSelect={select} />;

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
      tooltipTitle={(i) => (base.days[i] ? fmtBucket(base.days[i], interval(query)) : labels[i])}
      onPointClick={(si, i) => select(series[si], i)}
      legend={!compact || series.length <= 6}
      dataTable={!compact}
    />
  );
}

export function InsightResultView({ query, result, compact = false }: { query: InsightQuery; result: InsightResult; compact?: boolean }) {
  const [target, setTarget] = useState<ActorsTarget | null>(null);
  const [funnelTab, setFunnelTab] = useState<"steps" | "time">("steps");
  const closeActors = useCallback(() => setTarget(null), []);
  let body: ReactNode;

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
        <div className="flex flex-col gap-4">
          {!compact && result.time_to_convert.length > 0 && (
            <Seg
              label="Funnel view"
              value={funnelTab}
              onChange={setFunnelTab}
              className="self-start"
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
              dataTable
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
        dataTable={!compact}
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
          dataTable={!compact}
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
        <PathsSankey
          links={result.links}
          compact={compact}
          onOpenLink={(link) =>
            setTarget({
              selection: { type: "PathsLink", source: link.source, target: link.target },
              title: `${link.source.replace(/^\d+_/, "")} → ${link.target.replace(/^\d+_/, "")}`,
            })
          }
        />
      );
  } else if (result.kind === "Sql") {
    body = <SqlTable columns={result.columns} types={result.types} rows={result.rows} truncated={result.truncated} compact={compact} />;
  } else {
    body = <Empty icon="alert" title="Unexpected result shape" />;
  }

  return (
    <>
      {body}
      {target && <ActorsModal query={query} target={target} onClose={closeActors} />}
    </>
  );
}

// ── SQL ─────────────────────────────────────────────────────────────────

function cell(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function toCsv(columns: string[], rows: unknown[][]): string {
  const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
  return [columns.map(esc).join(","), ...rows.map((r) => r.map((v) => esc(cell(v))).join(","))].join("\n");
}

/** Rows rendered in the SQL table (virtualized). The server already caps results; CSV export is not limited by this. */
const SQL_RENDER_CAP = 5000;
const NUMERIC_TYPE = /int|float|double|decimal|number/i;

interface SqlRow {
  n: number;
  cells: unknown[];
}
const sqlCol = columnHelper<SqlRow>();

export function SqlTable({ columns, types, rows, truncated, compact }: { columns: string[]; types: string[]; rows: unknown[][]; truncated: boolean; compact?: boolean }) {
  const data = useMemo<SqlRow[]>(() => rows.slice(0, SQL_RENDER_CAP).map((cells, n) => ({ n, cells })), [rows]);
  const cols = useMemo(
    () =>
      columns.map((name, ci) => {
        const numeric = NUMERIC_TYPE.test(types[ci] ?? "");
        return sqlCol.accessor((r) => r.cells[ci] ?? null, {
          id: `c${ci}`,
          header: () => (
            <>
              {name}
              <span className="ml-1.5 font-normal tracking-normal text-muted-foreground normal-case">{types[ci]}</span>
            </>
          ),
          sortFn: numeric ? "basic" : "alphanumeric",
          cell: ({ getValue }) => {
            const v = getValue();
            return (
              <span className="block max-w-90 truncate font-mono text-xs" title={cell(v)}>
                {v === null ? <span className="text-muted-foreground">null</span> : typeof v === "number" ? fmtNumber(v) : cell(v)}
              </span>
            );
          },
          meta: { align: numeric ? "right" : undefined },
        });
      }),
    [columns, types],
  );
  if (columns.length === 0) return <Empty icon="table" title="The query returned no columns" />;
  return (
    <div className="flex flex-col gap-3">
      {!compact && (
        <div className="flex items-center gap-2">
          <span className="num flex-1 text-xs text-muted-foreground">
            {fmtNumber(rows.length)} rows
            {rows.length > SQL_RENDER_CAP && ` · showing the first ${fmtNumber(SQL_RENDER_CAP)}`}
            {truncated && " · truncated at the row cap"}
          </span>
          <Button
            variant="outline"
            size="sm"
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
          </Button>
        </div>
      )}
      <div className="overflow-hidden rounded-lg ring-1 ring-foreground/10">
        <DataTable
          label="SQL result"
          columns={cols}
          data={data}
          getRowId={(r) => String(r.n)}
          dense
          sortable
          virtualize={{ maxHeight: compact ? 260 : 560 }}
        />
      </div>
    </div>
  );
}

/** Skeleton sized like a chart so nothing shifts when results land. */
export function ChartSkeleton({ height = 340 }: { height?: number }) {
  return (
    <div className="flex flex-col justify-end gap-2" style={{ height }} aria-busy="true" aria-label="Loading result">
      <div className="flex items-end gap-1.5" style={{ height: height - 40 }}>
        {Array.from({ length: 18 }, (_, i) => (
          <Skeleton key={i} className="flex-1" style={{ height: `${30 + ((i * 37) % 60)}%` }} />
        ))}
      </div>
      <Skeleton className="h-2.5 w-full" />
    </div>
  );
}
