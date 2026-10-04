// The insight builder: editor on the left, live result on the right.

import { useEffect, useRef, useState } from "react";
import type { ChartDisplay } from "../types/ChartDisplay";
import type { InsightQuery } from "../types/InsightQuery";
import type { Interval } from "../types/Interval";
import { api, errorMessage, type SavedInsight } from "../lib/api";
import { canEdit, useApp, usePath, useProjectId } from "../lib/context";
import { autoInterval, fmtNumber, fmtRelative } from "../lib/format";
import { invalidate, useApi } from "../lib/hooks";
import { Link, navigate, useLocation } from "../lib/router";
import { DateRangePicker } from "../ui/DateRange";
import { Icon } from "../ui/icons";
import { Confirm, Empty, ErrorState, InlineEdit, LoadingBar, MenuButton, Skeleton, toast } from "../ui/kit";
import { KINDS, decodeQuery, defaultName, defaultQuery, encodeQuery, kindFromSlug, kindInfo } from "../insight/defaults";
import { QueryEditor } from "../insight/Editor";
import { ChartSkeleton, InsightResultView, useInsightQuery } from "../insight/Result";
import { SqlEditor } from "../insight/SqlEditor";

const DISPLAYS: { value: ChartDisplay; label: string }[] = [
  { value: "ActionsLineGraph", label: "Line" },
  { value: "ActionsAreaGraph", label: "Area" },
  { value: "ActionsBar", label: "Bar" },
  { value: "ActionsBarValue", label: "Total value" },
  { value: "ActionsTable", label: "Table" },
  { value: "ActionsPie", label: "Pie" },
  { value: "BoldNumber", label: "Number" },
];

export function AddToDashboard({ ensureSaved }: { ensureSaved: () => Promise<SavedInsight | null> }) {
  const projectId = useProjectId();
  const path = usePath();
  const dashboards = useApi(`dashboards:${projectId}`, (s) => api.dashboards(projectId, s));
  const add = async (dashboardId: string | null, close: () => void) => {
    close();
    try {
      const insight = await ensureSaved();
      if (!insight) return;
      let dash = dashboardId ? await api.dashboard(projectId, dashboardId) : await api.createDashboard(projectId, window.prompt("Dashboard name", "My dashboard")?.trim() || "My dashboard");
      const bottom = dash.tiles.reduce((m, t) => Math.max(m, t.y + t.h), 0);
      const tiles = [...dash.tiles.map(({ insight_id, x, y, w, h }) => ({ insight_id, x, y, w, h })), { insight_id: insight.id, x: 0, y: bottom, w: 6, h: 3 }];
      dash = await api.replaceTiles(projectId, dash.id, tiles);
      invalidate(`dashboard`);
      toast(`Added to ${dash.name}`);
      navigate(path(`dashboards/${dash.id}`));
    } catch (e) {
      toast(errorMessage(e), true);
    }
  };
  return (
    <MenuButton
      label={
        <>
          <Icon name="dashboard" size={14} /> Add to dashboard
        </>
      }
      align="end"
    >
      {(close) => (
        <div style={{ minWidth: 240 }}>
          <div className="menu-label">Dashboards</div>
          {dashboards.error ? <ErrorState error={dashboards.error} compact /> : null}
          {(dashboards.data ?? []).map((d) => (
            <button key={d.id} className="menu-item" onClick={() => add(d.id, close)}>
              <Icon name="dashboard" size={14} />
              <span className="truncate">{d.name}</span>
              <span className="meta">{d.tiles.length} tiles</span>
            </button>
          ))}
          {dashboards.data?.length === 0 && <div className="muted small" style={{ padding: "4px 10px 8px" }}>No dashboards yet.</div>}
          <div className="menu-sep" />
          <button className="menu-item" onClick={() => add(null, close)}>
            <Icon name="plus" size={14} /> New dashboard…
          </button>
        </div>
      )}
    </MenuButton>
  );
}

function ResultToolbar({ query, onChange }: { query: InsightQuery; onChange: (q: InsightQuery) => void }) {
  return (
    <>
      {"date_range" in query && (
        <DateRangePicker
          small
          value={query.date_range}
          onChange={(date_range) => {
            const next = { ...query, date_range } as InsightQuery;
            if ("interval" in next) (next as { interval: Interval }).interval = autoInterval(date_range.date_from, date_range.date_to);
            onChange(next);
          }}
        />
      )}
      {"interval" in query && (
        <select className="select small" value={query.interval} onChange={(e) => onChange({ ...query, interval: e.target.value as Interval })} aria-label="Interval">
          <option value="hour">Hourly</option>
          <option value="day">Daily</option>
          <option value="week">Weekly</option>
          <option value="month">Monthly</option>
        </select>
      )}
      {query.kind === "TrendsQuery" && (
        <>
          <button className="btn small" aria-pressed={query.compare} onClick={() => onChange({ ...query, compare: !query.compare })} title="Compare to the previous period">
            Compare
          </button>
          <select className="select small" value={query.display} onChange={(e) => onChange({ ...query, display: e.target.value as ChartDisplay })} aria-label="Chart type">
            {DISPLAYS.map((d) => (
              <option key={d.value} value={d.value}>
                {d.label}
              </option>
            ))}
          </select>
        </>
      )}
    </>
  );
}

export function InsightPage({ id }: { id: string | null }) {
  const projectId = useProjectId();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const path = usePath();
  const loc = useLocation();

  const saved = useApi(id ? `insight:${projectId}:${id}` : null, (s) => api.insight(projectId, id as string, s), { keepPrevious: false });
  const [query, setQuery] = useState<InsightQuery | null>(() => (id ? null : decodeQuery(loc.hash.replace(/^q=/, "")) ?? defaultQuery(kindFromSlug(loc.search.get("kind")))));
  const [name, setName] = useState("");
  const [dirty, setDirty] = useState(!id);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [sqlRun, setSqlRun] = useState<InsightQuery | null>(null);

  // Load a saved insight into the editor once.
  const loadedFor = useRef<string | null>(null);
  useEffect(() => {
    if (id && saved.data && loadedFor.current !== saved.data.id) {
      loadedFor.current = saved.data.id;
      setQuery(saved.data.query);
      setName(saved.data.name);
      setDirty(false);
      if (saved.data.query?.kind === "SqlQuery") setSqlRun(saved.data.query);
    }
  }, [id, saved.data]);

  // New insights keep their query in the URL so a reload never loses work.
  useEffect(() => {
    if (id || !query) return;
    const t = window.setTimeout(() => {
      window.history.replaceState(null, "", `${window.location.pathname}?kind=${kindInfo(query.kind).slug}#q=${encodeQuery(query)}`);
    }, 400);
    return () => window.clearTimeout(t);
  }, [id, query]);

  const isSql = query?.kind === "SqlQuery";
  const run = useInsightQuery(isSql ? sqlRun : query, isSql ? 0 : 350);

  const update = (q: InsightQuery) => {
    setQuery(q);
    setDirty(true);
  };

  const save = async (): Promise<SavedInsight | null> => {
    if (!query) return null;
    if (!editable) {
      toast("Only project owners and admins can save insights.", true);
      return null;
    }
    setSaving(true);
    try {
      const draft = { name: name.trim() || defaultName(query), description: "", query };
      const result = id ? await api.updateInsight(projectId, id, draft) : await api.createInsight(projectId, draft);
      invalidate(`insights:${projectId}`);
      invalidate(`insight:${projectId}:${result.id}`);
      setName(result.name);
      setDirty(false);
      toast(id ? "Insight saved" : "Insight created");
      if (!id) {
        loadedFor.current = result.id;
        navigate(path(`insights/${result.id}`), { replace: true });
      }
      return result;
    } catch (e) {
      toast(errorMessage(e), true);
      return null;
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (id && saved.error) {
    return (
      <div className="page">
        <ErrorState error={saved.error} retry={saved.reload} />
      </div>
    );
  }
  if (id && !query) {
    if (saved.data && saved.data.query === null) {
      return (
        <div className="page narrow">
          <Empty icon="alert" title={`“${saved.data.name}” uses a legacy query format`} action={<Link className="btn primary" to={path("insights/new")}>Build a new insight</Link>}>
            This insight was saved before the current query model and can't be opened in the builder.
          </Empty>
        </div>
      );
    }
    return (
      <div className="page">
        <Skeleton height={28} width={320} />
        <div className="builder mt-24">
          <Skeleton height={420} />
          <Skeleton height={420} />
        </div>
      </div>
    );
  }
  if (!query) return null;

  const switchKind = (slug: string) => {
    const kind = kindFromSlug(slug);
    if (kind === query.kind) return;
    update(defaultQuery(kind));
    setSqlRun(null);
  };

  const meta = run.data?.meta;
  const resultBody = (
    <div className="result-body" style={isSql ? { minHeight: 200 } : undefined}>
      <LoadingBar show={run.pending && !!run.data} />
      {run.hint ? (
        <Empty icon="info" title={run.hint} />
      ) : isSql && !sqlRun ? (
        <Empty icon="play" title="Run the query to see results">
          Press <span className="kbd">Ctrl</span> <span className="kbd">Enter</span> in the editor.
        </Empty>
      ) : run.error && !run.pending ? (
        <ErrorState error={run.error} retry={run.reload} />
      ) : run.data && run.sent ? (
        <div style={{ opacity: run.pending ? 0.55 : 1, transition: "opacity .15s" }}>
          <InsightResultView query={run.sent} result={run.data.result} />
        </div>
      ) : (
        <ChartSkeleton />
      )}
    </div>
  );

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <div className="row small muted" style={{ marginBottom: 2 }}>
            <Link to={path("insights")} className="link">
              Insights
            </Link>
            <Icon name="chevronRight" size={12} />
            <span>{id ? "Saved insight" : "New insight"}</span>
            {dirty && id && <span className="badge warn">Unsaved changes</span>}
          </div>
          <h1>
            <InlineEdit value={name} placeholder={defaultName(query)} ariaLabel="Insight name" onSave={(v) => { setName(v); setDirty(true); }} />
          </h1>
          {saved.data && <div className="sub small">Last saved {fmtRelative(saved.data.updated_at)}</div>}
        </div>
        <div className="actions">
          <AddToDashboard ensureSaved={async () => (id && !dirty && saved.data ? saved.data : save())} />
          {id && (
            <MenuButton label={<Icon name="more" />} className="btn icon" align="end" title="More actions">
              {(close) => (
                <>
                  <button
                    className="menu-item"
                    onClick={() => {
                      close();
                      navigate(`${path("insights/new")}?kind=${kindInfo(query.kind).slug}#q=${encodeQuery(query)}`);
                    }}
                  >
                    <Icon name="copy" size={14} /> Duplicate as new
                  </button>
                  {editable && (
                    <button
                      className="menu-item danger"
                      onClick={() => {
                        close();
                        setConfirmDelete(true);
                      }}
                    >
                      <Icon name="trash" size={14} /> Delete insight
                    </button>
                  )}
                </>
              )}
            </MenuButton>
          )}
          <button className="btn primary" onClick={() => void save()} disabled={saving || (!dirty && !!id)} title="Save (Ctrl+S)">
            <Icon name="save" size={14} /> {saving ? "Saving…" : id ? "Save" : "Save insight"}
          </button>
        </div>
      </div>

      <div className="seg" role="tablist" aria-label="Insight type" style={{ marginBottom: 16, flexWrap: "wrap" }}>
        {KINDS.map((k) => (
          <button key={k.kind} role="tab" aria-selected={k.kind === query.kind} onClick={() => switchKind(k.slug)} title={k.blurb}>
            <span className="row gap-4">
              <Icon name={k.icon} size={14} /> {k.label}
            </span>
          </button>
        ))}
      </div>

      {isSql && query.kind === "SqlQuery" ? (
        <div className="col gap-16">
          <div className="card">
            <div className="card-head">
              <h3>SQL</h3>
              <span className="muted small">
                Read-only. One table: <code>events</code> (uuid, event, distinct_id, person_id, timestamp, properties). Capped in rows, time and memory.
              </span>
            </div>
            <div className="card-body col gap-12">
              <SqlEditor value={query.query} onChange={(v) => update({ ...query, query: v })} onRun={() => setSqlRun({ ...query })} />
              <div className="row">
                <button className="btn accent" onClick={() => (sqlRun && sqlRun.kind === "SqlQuery" && sqlRun.query === query.query ? run.refresh() : setSqlRun({ ...query }))}>
                  <Icon name="play" size={13} /> Run
                </button>
                <span className="muted small">
                  <span className="kbd">Ctrl</span> <span className="kbd">Enter</span>
                </span>
                <span className="spacer" />
                {meta && <ResultMeta elapsed={meta.elapsed_ms} cached={meta.cached} />}
              </div>
            </div>
          </div>
          <div className="card">{resultBody}</div>
        </div>
      ) : (
        <div className="builder">
          <div className="card">
            <QueryEditor query={query} onChange={update} />
          </div>
          <div className="card" style={{ position: "relative" }}>
            <div className="result-head">
              <ResultToolbar query={query} onChange={update} />
              <span className="spacer" />
              {meta && <ResultMeta elapsed={meta.elapsed_ms} cached={meta.cached} />}
              <button className="btn ghost icon small" title="Recompute, bypassing the cache" aria-label="Refresh" onClick={run.refresh} disabled={!!run.hint}>
                <Icon name="refresh" size={14} />
              </button>
            </div>
            {resultBody}
          </div>
        </div>
      )}

      {confirmDelete && id && (
        <Confirm
          title="Delete insight?"
          body={`“${name}” will be removed from every dashboard. This can't be undone.`}
          confirmLabel="Delete"
          danger
          onClose={() => setConfirmDelete(false)}
          onConfirm={async () => {
            await api.deleteInsight(projectId, id);
            invalidate(`insights:${projectId}`);
            toast("Insight deleted");
            navigate(path("insights"));
          }}
        />
      )}
    </div>
  );
}

function ResultMeta({ elapsed, cached }: { elapsed: number; cached: boolean }) {
  return (
    <span className="result-meta" title={cached ? "Served from cache; the data version hasn't changed" : "Computed fresh"}>
      <Icon name={cached ? "bolt" : "clock"} size={12} />
      {cached ? "cached" : `${fmtNumber(elapsed)} ms`}
    </span>
  );
}
