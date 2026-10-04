import { useMemo } from "react";
import type { InsightQuery } from "../types/InsightQuery";
import { api } from "../lib/api";
import { useApp, usePath, useProjectId } from "../lib/context";
import { fmtCompact, fmtNumber, fmtRelative } from "../lib/format";
import { useApi } from "../lib/hooks";
import { eventLabel } from "../lib/properties";
import { Link } from "../lib/router";
import { Icon } from "../ui/icons";
import { Empty, ErrorState, Skeleton, SkeletonRows } from "../ui/kit";
import { eventNode, kindInfo, summarize } from "../insight/defaults";
import { ChartSkeleton, InsightResultView, useInsightQuery } from "../insight/Result";
import { FirstEventWatcher } from "./Onboarding";

function ActivityChart() {
  const query: InsightQuery = useMemo(
    () => ({
      kind: "TrendsQuery",
      series: [{ ...eventNode(null, "total"), custom_name: "Events" }, { ...eventNode(null, "dau"), custom_name: "Unique users" }],
      date_range: { date_from: "-14d", date_to: null },
      interval: "day",
      properties: [],
      breakdown: null,
      formula: null,
      compare: false,
      display: "ActionsLineGraph",
    }),
    [],
  );
  const run = useInsightQuery(query, 0);
  if (run.error && !run.data) return <ErrorState error={run.error} retry={run.reload} />;
  if (!run.data || !run.sent) return <ChartSkeleton height={260} />;
  return <InsightResultView query={run.sent} result={run.data.result} compact />;
}

export function HomePage() {
  const { project, workspace } = useApp();
  const projectId = useProjectId();
  const path = usePath();
  const status = useApi(`status:${projectId}`, (s) => api.status(projectId, s), { pollMs: 15_000 });
  const insights = useApi(`insights:${projectId}`, (s) => api.insights(projectId, s));
  const dashboards = useApi(`dashboards:${projectId}`, (s) => api.dashboards(projectId, s));
  const events = useApi(`catalog-events:${projectId}:`, (s) => api.catalogEvents(projectId, "", s));
  const empty = status.data && !status.data.has_events;
  const name = workspace.user.name || workspace.user.email.split("@")[0];

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>{project.name}</h1>
          <div className="sub">
            Welcome back, {name}.
            {status.data?.has_events && (
              <>
                {" "}
                {fmtNumber(status.data.stored_events)} events stored · last one {fmtRelative(status.data.last_event_at)}.
              </>
            )}
          </div>
        </div>
        <div className="actions">
          <Link className="btn" to={path("activity")}>
            <Icon name="activity" size={14} /> Live events
          </Link>
          <Link className="btn primary" to={`${path("insights/new")}?kind=trends`}>
            <Icon name="plus" size={14} /> New insight
          </Link>
        </div>
      </div>

      {empty && (
        <div className="card card-pad col gap-12" style={{ marginBottom: 16 }}>
          <div className="row">
            <h2 className="grow">Get started</h2>
            <Link className="btn primary" to={path("onboarding")}>
              Connect your app <Icon name="arrowRight" size={13} />
            </Link>
          </div>
          <p className="secondary">No events yet. Point any PostHog SDK at this server and they appear within seconds.</p>
          <FirstEventWatcher />
        </div>
      )}

      <div className="grid-2" style={{ gridTemplateColumns: "minmax(0, 2fr) minmax(0, 1fr)" }}>
        <div className="card">
          <div className="card-head">
            <h3>Events and unique users · last 14 days</h3>
            <Link className="btn ghost small" to={`${path("insights/new")}?kind=trends`}>
              Explore <Icon name="arrowRight" size={12} />
            </Link>
          </div>
          <div className="card-body" style={{ minHeight: 300 }}>
            <ActivityChart />
          </div>
        </div>
        <div className="card list-card">
          <div className="card-head">
            <h3>Top events</h3>
          </div>
          {events.error && !events.data ? (
            <ErrorState error={events.error} compact />
          ) : !events.data ? (
            <SkeletonRows rows={6} />
          ) : events.data.length === 0 ? (
            <Empty icon="bolt" title="No events seen yet" />
          ) : (
            [...events.data]
              .sort((a, b) => b.count - a.count)
              .slice(0, 8)
              .map((e) => (
                <Link key={e.name} className="item" to={`${path("activity")}?event=${encodeURIComponent(e.name)}`}>
                  <Icon name="bolt" size={14} style={{ color: "var(--ink-3)" }} />
                  <span className="truncate grow">{eventLabel(e.name)}</span>
                  <span className="muted small num">{e.count ? fmtCompact(e.count) : ""}</span>
                </Link>
              ))
          )}
        </div>
      </div>

      <div className="grid-2 mt-16">
        <div className="card list-card">
          <div className="card-head">
            <h3>Recent insights</h3>
            <Link className="btn ghost small" to={path("insights")}>
              All insights
            </Link>
          </div>
          {insights.error && !insights.data ? (
            <ErrorState error={insights.error} compact />
          ) : !insights.data ? (
            <div style={{ padding: 16 }}>
              <Skeleton height={120} />
            </div>
          ) : insights.data.length === 0 ? (
            <Empty icon="trends" title="No saved insights" action={<Link className="btn small" to={`${path("insights/new")}?kind=funnels`}>Build a funnel</Link>}>
              Save an insight to find it here.
            </Empty>
          ) : (
            [...insights.data]
              .sort((a, b) => b.updated_at - a.updated_at)
              .slice(0, 6)
              .map((i) => (
                <Link key={i.id} className="item" to={path(`insights/${i.id}`)}>
                  <span className="kind-icon">
                    <Icon name={i.query ? kindInfo(i.query.kind).icon : "alert"} size={14} />
                  </span>
                  <span className="col grow" style={{ gap: 0, minWidth: 0 }}>
                    <b className="truncate">{i.name}</b>
                    <span className="muted small truncate">{summarize(i.query)}</span>
                  </span>
                  <span className="muted small">{fmtRelative(i.updated_at)}</span>
                </Link>
              ))
          )}
        </div>
        <div className="card list-card">
          <div className="card-head">
            <h3>Dashboards</h3>
            <Link className="btn ghost small" to={path("dashboards")}>
              All dashboards
            </Link>
          </div>
          {dashboards.error && !dashboards.data ? (
            <ErrorState error={dashboards.error} compact />
          ) : !dashboards.data ? (
            <div style={{ padding: 16 }}>
              <Skeleton height={120} />
            </div>
          ) : dashboards.data.length === 0 ? (
            <Empty icon="dashboard" title="No dashboards yet">
              Pin insights side by side for a one-glance view.
            </Empty>
          ) : (
            dashboards.data.slice(0, 6).map((d) => (
              <Link key={d.id} className="item" to={path(`dashboards/${d.id}`)}>
                <span className="kind-icon">
                  <Icon name="dashboard" size={14} />
                </span>
                <b className="truncate grow">{d.name}</b>
                <span className="muted small">{d.tiles.length} tiles</span>
              </Link>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
