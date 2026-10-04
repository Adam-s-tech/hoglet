import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useMemo, type ReactNode } from "react";
import { Empty, ErrorState, Skeleton, SkeletonRows } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { CardBar, CardPad, IconBadge, Page, PageHeader, Panel } from "@/components/page";
import { Button } from "@/components/ui/button";
import { useApp, usePath, useProjectId } from "@/lib/context";
import { fmtCompact, fmtNumber, fmtRelative } from "@/lib/format";
import { eventLabel } from "@/lib/properties";
import { catalogEventsQuery, dashboardsQuery, insightResultQuery, insightsQuery, statusQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { InsightQuery } from "@/types/InsightQuery";
import { eventNode, kindInfo, summarize } from "@/insight/defaults";
import { InsightResultView } from "@/insight/Result";
import { FirstEventWatcher, LoadDemoButton } from "@/pages/Onboarding";

const STATUS_POLL_MS = 15_000;

/** Events and unique users over the last two weeks: the one chart worth a home page. */
const ACTIVITY_QUERY: InsightQuery = {
  kind: "TrendsQuery",
  series: [
    { ...eventNode(null, "total"), custom_name: "Events" },
    { ...eventNode(null, "dau"), custom_name: "Unique users" },
  ],
  date_range: { date_from: "-14d", date_to: null },
  interval: "day",
  properties: [],
  breakdown: null,
  formula: null,
  compare: false,
  display: "ActionsLineGraph",
};

function ChartSkeleton({ height }: { height: number }) {
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

function ActivityChart() {
  const pid = useProjectId();
  const request = useMemo(() => ({ query: ACTIVITY_QUERY, refresh: false }), []);
  const { data, error, refetch } = useQuery(insightResultQuery(pid, request));
  if (error && !data) return <ErrorState error={error} retry={() => void refetch()} />;
  if (!data) return <ChartSkeleton height={260} />;
  return <InsightResultView query={ACTIVITY_QUERY} result={data.result} compact />;
}

function ListRow({ to, className, children }: { to: string; className?: string; children: ReactNode }) {
  return (
    <Link
      to={to}
      className={cn(
        "flex items-center gap-3 border-b px-4 py-2.5 transition-colors outline-none last:border-b-0 hover:bg-accent focus-visible:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        className,
      )}
    >
      {children}
    </Link>
  );
}

export function HomePage() {
  const { project, workspace } = useApp();
  const pid = useProjectId();
  const path = usePath();
  const status = useQuery(statusQuery(pid, STATUS_POLL_MS));
  const insights = useQuery(insightsQuery(pid));
  const dashboards = useQuery(dashboardsQuery(pid));
  const events = useQuery(catalogEventsQuery(pid, ""));
  const empty = status.data && !status.data.has_events;
  const name = workspace.user.name || workspace.user.email.split("@")[0];

  const topEvents = useMemo(() => [...(events.data ?? [])].sort((a, b) => b.count - a.count).slice(0, 8), [events.data]);
  const recentInsights = useMemo(() => [...(insights.data ?? [])].sort((a, b) => b.updated_at - a.updated_at).slice(0, 6), [insights.data]);

  return (
    <Page>
      <PageHeader
        title={project.name}
        sub={
          <>
            Welcome back, {name}.
            {status.data?.has_events && (
              <>
                {" "}
                {fmtNumber(status.data.stored_events)} events stored · last one {fmtRelative(status.data.last_event_at)}.
              </>
            )}
          </>
        }
        actions={
          <>
            <Button variant="outline" nativeButton={false} render={<Link to={path("activity")} />}>
              <Icon name="activity" size={14} /> Live events
            </Button>
            <Button nativeButton={false} render={<Link to={path("insights/new")} search={{ kind: "trends" }} />}>
              <Icon name="plus" size={14} /> New insight
            </Button>
          </>
        }
      />

      {empty && (
        <Panel className="mb-4">
          <CardPad className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="flex-1">Get started</h2>
              <LoadDemoButton />
              <Button nativeButton={false} render={<Link to={path("onboarding")} />}>
                Connect your app <Icon name="arrowRight" size={13} />
              </Button>
            </div>
            <p className="text-muted-foreground">No events yet. Point any PostHog SDK at this server and they appear within seconds.</p>
            <FirstEventWatcher />
          </CardPad>
        </Panel>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <Panel>
          <CardBar>
            <h2 className="flex-1 text-sm">Events and unique users · last 14 days</h2>
            <Button variant="ghost" size="sm" nativeButton={false} render={<Link to={path("insights/new")} search={{ kind: "trends" }} />}>
              Explore <Icon name="arrowRight" size={12} />
            </Button>
          </CardBar>
          <CardPad className="min-h-[300px]">
            <ActivityChart />
          </CardPad>
        </Panel>
        <Panel>
          <CardBar>
            <h2 className="text-sm">Top events</h2>
          </CardBar>
          {events.error && !events.data ? (
            <CardPad>
              <ErrorState error={events.error} compact />
            </CardPad>
          ) : !events.data ? (
            <SkeletonRows rows={6} />
          ) : topEvents.length === 0 ? (
            <Empty icon="bolt" title="No events seen yet" />
          ) : (
            topEvents.map((e) => (
              <ListRow key={e.name} to={`${path("activity")}?event=${encodeURIComponent(e.name)}`}>
                <Icon name="bolt" size={14} className="flex-none text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate">{eventLabel(e.name)}</span>
                <span className="num text-xs text-muted-foreground">{e.count ? fmtCompact(e.count) : ""}</span>
              </ListRow>
            ))
          )}
        </Panel>
      </div>

      <div className="mt-4 grid gap-4 md:grid-cols-2">
        <Panel>
          <CardBar>
            <h2 className="flex-1 text-sm">Recent insights</h2>
            <Button variant="ghost" size="sm" nativeButton={false} render={<Link to={path("insights")} />}>
              All insights
            </Button>
          </CardBar>
          {insights.error && !insights.data ? (
            <CardPad>
              <ErrorState error={insights.error} compact />
            </CardPad>
          ) : !insights.data ? (
            <CardPad>
              <Skeleton className="h-30 w-full" />
            </CardPad>
          ) : recentInsights.length === 0 ? (
            <Empty
              icon="trends"
              title="No saved insights"
              action={
                <Button variant="outline" size="sm" nativeButton={false} render={<Link to={path("insights/new")} search={{ kind: "funnels" }} />}>
                  Build a funnel
                </Button>
              }
            >
              Save an insight to find it here.
            </Empty>
          ) : (
            recentInsights.map((i) => (
              <ListRow key={i.id} to={path(`insights/${i.id}`)}>
                <IconBadge>
                  <Icon name={i.query ? kindInfo(i.query.kind).icon : "alert"} size={14} />
                </IconBadge>
                <span className="flex min-w-0 flex-1 flex-col">
                  <b className="truncate">{i.name}</b>
                  <span className="truncate text-xs text-muted-foreground">{summarize(i.query)}</span>
                </span>
                <span className="text-xs whitespace-nowrap text-muted-foreground">{fmtRelative(i.updated_at)}</span>
              </ListRow>
            ))
          )}
        </Panel>
        <Panel>
          <CardBar>
            <h2 className="flex-1 text-sm">Dashboards</h2>
            <Button variant="ghost" size="sm" nativeButton={false} render={<Link to={path("dashboards")} />}>
              All dashboards
            </Button>
          </CardBar>
          {dashboards.error && !dashboards.data ? (
            <CardPad>
              <ErrorState error={dashboards.error} compact />
            </CardPad>
          ) : !dashboards.data ? (
            <CardPad>
              <Skeleton className="h-30 w-full" />
            </CardPad>
          ) : dashboards.data.length === 0 ? (
            <Empty icon="dashboard" title="No dashboards yet">
              Pin insights side by side for a one-glance view.
            </Empty>
          ) : (
            dashboards.data.slice(0, 6).map((d) => (
              <ListRow key={d.id} to={path(`dashboards/${d.id}`)}>
                <IconBadge>
                  <Icon name="dashboard" size={14} />
                </IconBadge>
                <b className="min-w-0 flex-1 truncate">{d.name}</b>
                <span className="text-xs text-muted-foreground">{d.tiles.length} {d.tiles.length === 1 ? "tile" : "tiles"}</span>
              </ListRow>
            ))
          )}
        </Panel>
      </div>
    </Page>
  );
}
