// Web analytics: one screen, Plausible-grade. Every row filters the page.

import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { columnHelper, DataTable } from "@/components/data-table";
import { DateRangePicker, type RangeValue } from "@/components/date-range";
import { AppDialog } from "@/components/dialogs";
import { Empty, ErrorState, LoadingBar, Skeleton } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { CardBar, Page, PageHeader, Panel, SearchInput, StatLabel, Toolbar } from "@/components/page";
import { TimeSeriesChart } from "@/charts/TimeSeries";
import { PropertyFilters } from "@/insight/pickers";
import { autoInterval, fmtBucket, fmtCompact, fmtDuration, fmtNumber, fmtPercent } from "@/lib/format";
import { useLocalStorage } from "@/lib/hooks";
import { useProjectId, usePath } from "@/lib/context";
import { describeFilter } from "@/lib/properties";
import { webBreakdownQuery, webOverviewQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { PropertyFilter } from "@/types/PropertyFilter";
import type { WebBreakdownRow } from "@/types/WebBreakdownRow";
import type { WebDimension } from "@/types/WebDimension";
import type { WebMetric } from "@/types/WebMetric";
import type { WebQuery } from "@/types/WebQuery";
import { LoadDemoButton } from "./Onboarding";

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

interface PanelTab {
  dim: WebDimension;
  label: string;
  col: string;
}
interface PanelDef {
  title: string;
  tabs: PanelTab[];
}

const PANELS: PanelDef[] = [
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

const PREVIEW_ROWS = 10;
const ALL_ROWS = 200;

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
  if (value === "" || value === "$$_none" || (dim === "referring_domain" && value === "$direct")) return dim === "referring_domain" ? "Direct / none" : "(none)";
  if (dim === "country") return countryName(value);
  return value;
}

const isPageLike = (dim: WebDimension) => dim === "page" || dim === "entry_page" || dim === "exit_page";

// ── KPI delta ────────────────────────────────────────────────────────────

function Delta({ metric, invert }: { metric: WebMetric; invert?: boolean }) {
  const flat = "text-xs font-semibold text-muted-foreground";
  if (metric.previous === null || metric.previous === 0) return <span className={flat}>no prior data</span>;
  const pct = ((metric.value - metric.previous) / Math.abs(metric.previous)) * 100;
  if (Math.abs(pct) < 0.05) return <span className={flat}>no change</span>;
  const good = invert ? pct < 0 : pct > 0;
  return (
    <span className={cn("num inline-flex items-center gap-0.5 text-xs font-semibold", good ? "text-good" : "text-destructive")} title={`Previous period: ${fmtNumber(metric.previous)}`}>
      <Icon name={pct > 0 ? "arrowUp" : "arrowDown"} size={11} strokeWidth={2.2} />
      {fmtPercent(Math.abs(pct))}
      <span className="sr-only">{pct > 0 ? " up" : " down"} from {fmtNumber(metric.previous)} in the previous period</span>
    </span>
  );
}

// ── Breakdowns ───────────────────────────────────────────────────────────

type OnFilter = (dim: WebDimension, value: string) => void;

function BreakdownList({ tab, query, onFilter }: { tab: PanelTab; query: WebQuery; onFilter: OnFilter }) {
  const projectId = useProjectId();
  const [all, setAll] = useState(false);
  const { data, error, isFetching, refetch } = useQuery(webBreakdownQuery(projectId, query, tab.dim, PREVIEW_ROWS));
  const rows = data?.rows ?? [];
  const max = Math.max(1, ...rows.map((r) => r.visitors));

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex border-t px-4 pt-2 pb-1.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
        <span className="flex-1">{tab.col}</span>
        <span className="w-16 text-right">Visitors</span>
        <span className="w-16 text-right">{isPageLike(tab.dim) ? "Views" : "Events"}</span>
      </div>
      <div className="relative flex-1">
        <LoadingBar show={isFetching && !!data} />
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} compact />
        ) : !data ? (
          <div className="flex flex-col gap-3 px-4 py-1" aria-busy="true" aria-label="Loading">
            {Array.from({ length: 8 }, (_, i) => (
              <Skeleton key={i} className="h-[18px]" style={{ width: `${90 - i * 9}%` }} />
            ))}
          </div>
        ) : rows.length === 0 ? (
          <div className="px-4 py-6 text-center text-muted-foreground">No data for this range.</div>
        ) : (
          <ul>
            {rows.map((r) => {
              const label = displayValue(tab.dim, r.value);
              return (
                <li key={r.value}>
                  <button
                    type="button"
                    onClick={() => onFilter(tab.dim, r.value)}
                    title={`Filter by ${label}`}
                    className="relative mx-2 mb-0.5 flex h-8 w-[calc(100%-1rem)] items-center gap-3 rounded-md px-2 text-left outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <span aria-hidden="true" className="absolute inset-y-[3px] left-0 rounded bg-chart-1/15" style={{ width: `${(r.visitors / max) * 100}%` }} />
                    <span className="relative min-w-0 flex-1 truncate">{label}</span>
                    <span className="num relative w-14 text-right font-semibold">{fmtCompact(r.visitors)}</span>
                    <span className="num relative w-14 text-right text-muted-foreground">{fmtCompact(r.views)}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {rows.length >= PREVIEW_ROWS && (
        <div className="px-2 pt-1.5 pb-2.5 text-center">
          <Button variant="ghost" size="sm" onClick={() => setAll(true)}>
            View all <Icon name="arrowRight" size={12} />
          </Button>
        </div>
      )}
      {all && <BreakdownAll dim={tab.dim} title={tab.label} column={tab.col} query={query} onClose={() => setAll(false)} onFilter={onFilter} />}
    </div>
  );
}

const rowCol = columnHelper<WebBreakdownRow>();

function BreakdownAll({ dim, title, column, query, onClose, onFilter }: { dim: WebDimension; title: string; column: string; query: WebQuery; onClose: () => void; onFilter: OnFilter }) {
  const projectId = useProjectId();
  const [search, setSearch] = useState("");
  const { data, error, isPending, refetch } = useQuery(webBreakdownQuery(projectId, query, dim, ALL_ROWS));
  const rows = useMemo(() => {
    const q = search.trim().toLowerCase();
    return (data?.rows ?? []).filter((r) => !q || displayValue(dim, r.value).toLowerCase().includes(q));
  }, [data, dim, search]);
  const hasBounce = (data?.rows ?? []).some((r) => r.bounce_rate !== null);

  const columns = useMemo(
    () => [
      rowCol.accessor((r) => displayValue(dim, r.value), {
        id: "value",
        header: column,
        cell: (c) => (
          <span className="block max-w-[420px] truncate" title={c.getValue()}>
            {c.getValue()}
          </span>
        ),
      }),
      rowCol.accessor("visitors", { header: "Visitors", cell: (c) => <span className="num">{fmtNumber(c.getValue())}</span>, meta: { align: "right" } }),
      rowCol.accessor("views", { header: isPageLike(dim) ? "Views" : "Events", cell: (c) => <span className="num">{fmtNumber(c.getValue())}</span>, meta: { align: "right" } }),
      ...(hasBounce
        ? [rowCol.accessor("bounce_rate", { header: "Bounce rate", cell: (c) => <span className="num">{fmtPercent(c.getValue())}</span>, meta: { align: "right" as const } })]
        : []),
    ],
    [dim, column, hasBounce],
  );

  return (
    <AppDialog title={title} description="Select a row to filter the whole page by it." onClose={onClose} wide>
      <div className="flex flex-col gap-3">
        <div className="flex items-center gap-3">
          <SearchInput wrapperClassName="flex-1" placeholder="Search…" aria-label={`Search ${title.toLowerCase()}`} value={search} onChange={(e) => setSearch(e.target.value)} />
          {data ? (
            <span className="num text-xs text-muted-foreground">
              {fmtNumber(rows.length)}
              {data.rows.length >= ALL_ROWS ? "+" : ""} rows
            </span>
          ) : null}
        </div>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} compact />
        ) : isPending ? (
          <div className="flex flex-col gap-2.5" aria-busy="true" aria-label="Loading">
            {Array.from({ length: 8 }, (_, i) => (
              <Skeleton key={i} className="h-6" />
            ))}
          </div>
        ) : rows.length === 0 ? (
          <div className="px-4 py-8 text-center text-muted-foreground">{search ? "Nothing matches your search." : "No data for this range."}</div>
        ) : (
          <div className="overflow-hidden rounded-lg border">
            <DataTable
              label={title}
              columns={columns}
              data={rows}
              getRowId={(r) => r.value}
              onRowClick={(r) => {
                onFilter(dim, r.value);
                onClose();
              }}
              sortable
              dense
              initialSorting={[{ id: "visitors", desc: true }]}
              virtualize={{ maxHeight: 420 }}
            />
          </div>
        )}
      </div>
    </AppDialog>
  );
}

function BreakdownPanel({ panel, query, onFilter }: { panel: PanelDef; query: WebQuery; onFilter: OnFilter }) {
  const [dim, setDim] = useState<WebDimension>(panel.tabs[0].dim);
  if (panel.tabs.length === 1) {
    return (
      <Panel className="min-h-[420px]">
        <CardBar className="min-h-14 border-b-0">
          <h3 className="flex-1">{panel.title}</h3>
        </CardBar>
        <BreakdownList tab={panel.tabs[0]} query={query} onFilter={onFilter} />
      </Panel>
    );
  }
  return (
    <Panel className="min-h-[420px]">
      <Tabs value={dim} onValueChange={(v) => setDim(v as WebDimension)} className="min-h-0 flex-1 gap-0">
        <CardBar className="border-b-0">
          <h3 className="flex-1">{panel.title}</h3>
          <TabsList aria-label={`${panel.title} breakdown`} className="max-w-full overflow-x-auto">
            {panel.tabs.map((t) => (
              <TabsTrigger key={t.dim} value={t.dim} className="flex-none px-2.5 text-xs">
                {t.label}
              </TabsTrigger>
            ))}
          </TabsList>
        </CardBar>
        {panel.tabs.map((t) => (
          <TabsContent key={t.dim} value={t.dim} className="flex min-h-0 flex-col">
            <BreakdownList tab={t} query={query} onFilter={onFilter} />
          </TabsContent>
        ))}
      </Tabs>
    </Panel>
  );
}

// ── Page ─────────────────────────────────────────────────────────────────

type ChartMetric = "visitors" | "pageviews";

interface Kpi {
  key: string;
  label: string;
  value: string;
  metric: WebMetric | undefined;
  invert?: boolean;
  chart?: ChartMetric;
}

export function WebPage() {
  const projectId = useProjectId();
  const path = usePath();
  const [range, setRange] = useLocalStorage<RangeValue>("hoglet.web.range", { date_from: "-7d", date_to: null });
  const [filters, setFilters] = useState<PropertyFilter[]>([]);
  const [metric, setMetric] = useState<ChartMetric>("visitors");
  const complete = useMemo(() => filters.filter((f) => f.key), [filters]);
  const query: WebQuery = useMemo(
    () => ({ date_from: range.date_from, date_to: range.date_to, interval: autoInterval(range.date_from, range.date_to), properties: complete }),
    [range.date_from, range.date_to, complete],
  );
  const { data, error, isFetching, refetch } = useQuery(webOverviewQuery(projectId, query));

  const addFilter: OnFilter = (dim, value) => {
    const key = DIM_PROPERTY[dim];
    const next = filters.filter((f) => f.key !== key);
    next.push(value === "" || value === "$$_none" ? { key, type: "event", operator: "is_not_set", value: null } : { key, type: "event", operator: "exact", value: [value] });
    setFilters(next);
  };

  const kpis: Kpi[] = [
    { key: "visitors", label: "Visitors", value: fmtCompact(data?.visitors.value), metric: data?.visitors, chart: "visitors" },
    { key: "pageviews", label: "Pageviews", value: fmtCompact(data?.pageviews.value), metric: data?.pageviews, chart: "pageviews" },
    { key: "sessions", label: "Sessions", value: fmtCompact(data?.sessions.value), metric: data?.sessions },
    { key: "bounce", label: "Bounce rate", value: fmtPercent(data?.bounce_rate.value), metric: data?.bounce_rate, invert: true },
    { key: "duration", label: "Session duration", value: fmtDuration(data?.session_duration_s.value), metric: data?.session_duration_s },
  ];
  const noData = data && data.pageviews.value === 0 && data.pageviews.previous === null && complete.length === 0;

  return (
    <Page>
      <PageHeader
        title="Web analytics"
        sub="Visitors, sources and pages from your pageviews."
        actions={
          <>
            {data && (
              <Badge variant="outline" className="h-7 gap-1.5 px-2.5 text-[12.5px] text-good" title="Persons with a pageview in the last 5 minutes" aria-live="polite">
                <span aria-hidden="true" className="size-2 rounded-full bg-good motion-safe:animate-pulse" />
                {fmtNumber(data.live_visitors)} live {data.live_visitors === 1 ? "visitor" : "visitors"}
              </Badge>
            )}
            <DateRangePicker value={range} onChange={setRange} />
          </>
        }
      />

      <Toolbar>
        <PropertyFilters value={filters} onChange={setFilters} sources={["event", "person"]} />
        {complete.length > 0 && (
          <Button variant="ghost" size="sm" onClick={() => setFilters([])}>
            Clear filters
          </Button>
        )}
      </Toolbar>

      {error && !data ? (
        <Panel>
          <ErrorState error={error} retry={() => void refetch()} />
        </Panel>
      ) : noData ? (
        <Panel>
          <Empty
            icon="globe"
            title="No pageviews yet"
            action={
              <div className="flex flex-wrap justify-center gap-2">
                <LoadDemoButton />
                <Button nativeButton={false} render={<Link to={path("onboarding")} />}>Connect your site</Button>
              </div>
            }
          >
            Add posthog-js to your site with <code>api_host</code> pointing at this server. Pageviews are captured automatically and appear here within seconds.
          </Empty>
        </Panel>
      ) : (
        <>
          <Panel className="relative mb-4">
            <LoadingBar show={isFetching && !!data} />
            <div className="grid grid-cols-2 border-b sm:grid-cols-3 lg:grid-cols-5" role="group" aria-label="Key metrics">
              {kpis.map((k) => {
                const inner = (
                  <>
                    <StatLabel>{k.label}</StatLabel>
                    {data ? <span className="num text-[26px] leading-tight font-semibold tracking-tight">{k.value}</span> : <Skeleton className="my-px h-[30px] w-[90px]" />}
                    {k.metric ? <Delta metric={k.metric} invert={k.invert} /> : <Skeleton className="h-3 w-[60px]" />}
                  </>
                );
                const cell = "relative flex flex-col items-start gap-1 px-[18px] py-3.5 text-left lg:border-r lg:last:border-r-0";
                const chart = k.chart;
                return chart ? (
                  <button
                    key={k.key}
                    type="button"
                    aria-pressed={metric === chart}
                    onClick={() => setMetric(chart)}
                    title={`Chart ${k.label.toLowerCase()}`}
                    className={cn(
                      cell,
                      "cursor-pointer outline-none hover:bg-muted/60 focus-visible:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
                      "after:absolute after:inset-x-[18px] after:-bottom-px after:h-0.5 after:rounded-sm after:bg-chart-1 after:opacity-0 aria-pressed:after:opacity-100",
                    )}
                  >
                    {inner}
                  </button>
                ) : (
                  <div key={k.key} className={cell}>
                    {inner}
                  </div>
                );
              })}
            </div>
            <div className="px-[18px] pt-[18px] pb-3">
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
                <Skeleton className="h-[260px]" />
              )}
            </div>
          </Panel>

          <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
            {PANELS.map((p) => (
              <BreakdownPanel key={p.title} panel={p} query={query} onFilter={addFilter} />
            ))}
          </div>
          {complete.length > 0 && (
            <p className="mt-4 text-xs text-muted-foreground">
              Filtered by{" "}
              {complete
                .map((f) => {
                  const d = describeFilter(f);
                  return `${d.key} ${d.op} ${d.value}`;
                })
                .join(", ")}
              .
            </p>
          )}
        </>
      )}
    </Page>
  );
}
