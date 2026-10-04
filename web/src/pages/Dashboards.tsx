import { useEffect, useRef, useState } from "react";
import { api, errorMessage, type Dashboard, type DashboardTile } from "../lib/api";
import { canEdit, useApp, usePath, useProjectId } from "../lib/context";
import { fmtDate, fmtRelative } from "../lib/format";
import { invalidate, useApi } from "../lib/hooks";
import { Link, navigate } from "../lib/router";
import { DateRangePicker, type RangeValue } from "../ui/DateRange";
import { Icon } from "../ui/icons";
import { Confirm, CopyButton, Empty, ErrorState, InlineEdit, LoadingBar, MenuButton, Modal, Skeleton, SkeletonRows, toast } from "../ui/kit";
import { kindInfo, summarize, withDateRange } from "../insight/defaults";
import { ChartSkeleton, InsightResultView, useInsightQuery } from "../insight/Result";

export function DashboardsPage() {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const { data, error, loading, reload } = useApi(`dashboards:${projectId}`, (s) => api.dashboards(projectId, s));
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");

  const create = async () => {
    try {
      const d = await api.createDashboard(projectId, name.trim() || "New dashboard");
      invalidate(`dashboards:${projectId}`);
      navigate(path(`dashboards/${d.id}`));
    } catch (e) {
      toast(errorMessage(e), true);
    }
  };

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>Dashboards</h1>
          <div className="sub">Insights side by side, live, on one screen.</div>
        </div>
        <div className="actions">
          {canEdit(organization) && (
            <button className="btn primary" onClick={() => setCreating(true)}>
              <Icon name="plus" size={14} /> New dashboard
            </button>
          )}
        </div>
      </div>
      <div className="card">
        {error ? (
          <ErrorState error={error} retry={reload} />
        ) : !data && loading ? (
          <SkeletonRows rows={4} />
        ) : data && data.length === 0 ? (
          <Empty icon="dashboard" title="No dashboards yet" action={canEdit(organization) && <button className="btn primary" onClick={() => setCreating(true)}>Create a dashboard</button>}>
            Create one, then use “Add to dashboard” on any insight to pin it.
          </Empty>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th className="r">Tiles</th>
                <th className="r">Created</th>
              </tr>
            </thead>
            <tbody>
              {(data ?? []).map((d) => (
                <tr key={d.id} className="clickable" onClick={() => navigate(path(`dashboards/${d.id}`))}>
                  <td>
                    <div className="row gap-12">
                      <span className="kind-icon">
                        <Icon name="dashboard" size={14} />
                      </span>
                      <Link to={path(`dashboards/${d.id}`)} onClick={(e) => e.stopPropagation()} style={{ fontWeight: 600 }}>
                        {d.name}
                      </Link>
                    </div>
                  </td>
                  <td className="r">{d.tiles.length}</td>
                  <td className="r muted">{fmtDate(d.created_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      {creating && (
        <Modal
          title="New dashboard"
          onClose={() => setCreating(false)}
          footer={
            <>
              <button className="btn" onClick={() => setCreating(false)}>
                Cancel
              </button>
              <button className="btn primary" onClick={create}>
                Create
              </button>
            </>
          }
        >
          <label className="field">
            <span>Name</span>
            <input className="input" value={name} placeholder="Product health" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && create()} />
          </label>
        </Modal>
      )}
    </div>
  );
}

function Tile({ tile, range, onRemove, onResize, editable, refreshKey }: { tile: DashboardTile; range: RangeValue | null; onRemove: () => void; onResize: (w: number, h: number) => void; editable: boolean; refreshKey: number }) {
  const path = usePath();
  const insight = tile.insight;
  const query = insight?.query ? withDateRange(insight.query, range) : null;
  const run = useInsightQuery(query, 0);
  const seen = useRef(refreshKey);
  useEffect(() => {
    if (seen.current === refreshKey) return;
    seen.current = refreshKey;
    run.refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);
  const w = Math.max(3, Math.min(12, tile.w));
  const h = Math.max(2, Math.min(8, tile.h));
  const k = insight?.query ? kindInfo(insight.query.kind) : null;
  return (
    <div className="card tile" style={{ gridColumn: `span ${w}`, gridRow: `span ${h}` }}>
      <div className="card-head">
        <span className="kind-icon" style={{ width: 24, height: 24 }}>
          <Icon name={k?.icon ?? "alert"} size={13} />
        </span>
        <div className="col grow" style={{ gap: 0, minWidth: 0 }}>
          <Link to={path(`insights/${tile.insight_id}`)} className="truncate" style={{ fontWeight: 600 }}>
            {insight?.name ?? "Missing insight"}
          </Link>
          {insight && <span className="muted small truncate">{summarize(insight.query)}</span>}
        </div>
        <MenuButton label={<Icon name="more" />} className="btn ghost icon small" align="end" title="Tile actions">
          {(close) => (
            <>
              <button className="menu-item" onClick={() => navigate(path(`insights/${tile.insight_id}`))}>
                <Icon name="external" size={14} /> Open insight
              </button>
              {editable && (
                <>
                  <div className="menu-label">Size</div>
                  {[
                    { label: "Small", w: 4, h: 3 },
                    { label: "Half width", w: 6, h: 3 },
                    { label: "Full width", w: 12, h: 4 },
                  ].map((s) => (
                    <button
                      key={s.label}
                      className="menu-item"
                      aria-selected={tile.w === s.w && tile.h === s.h}
                      onClick={() => {
                        close();
                        onResize(s.w, s.h);
                      }}
                    >
                      {s.label}
                    </button>
                  ))}
                  <div className="menu-sep" />
                  <button
                    className="menu-item danger"
                    onClick={() => {
                      close();
                      onRemove();
                    }}
                  >
                    <Icon name="x" size={14} /> Remove from dashboard
                  </button>
                </>
              )}
            </>
          )}
        </MenuButton>
      </div>
      <div className="tile-body">
        <LoadingBar show={run.pending && !!run.data} />
        {!insight?.query ? (
          <Empty icon="alert" title="This insight can't be rendered" />
        ) : run.hint ? (
          <Empty icon="info" title={run.hint} />
        ) : run.error && !run.pending ? (
          <ErrorState error={run.error} retry={run.reload} compact />
        ) : run.data && run.sent ? (
          <InsightResultView query={run.sent} result={run.data.result} compact />
        ) : (
          <ChartSkeleton height={Math.max(140, h * 110 - 90)} />
        )}
      </div>
    </div>
  );
}

function ShareModal({ dashboard, onClose }: { dashboard: Dashboard; onClose: () => void }) {
  const projectId = useProjectId();
  const shares = useApi(`shares:${projectId}`, (s) => api.shares(projectId, s));
  const mine = (shares.data ?? []).filter((s) => s.object_type === "dashboard" && s.object_id === dashboard.id);
  const [busy, setBusy] = useState(false);
  const url = (token: string) => `${window.location.origin}/share/${token}`;
  return (
    <Modal title={`Share “${dashboard.name}”`} onClose={onClose}>
      <div className="col gap-16">
        <p className="secondary">Anyone with a share link can view this dashboard without signing in. Revoke a link to cut access immediately.</p>
        {shares.error ? <ErrorState error={shares.error} compact /> : null}
        {mine.map((s) => (
          <div key={s.id} className="col" style={{ gap: 6 }}>
            <div className="token-box">
              <code>{url(s.token)}</code>
              <CopyButton text={url(s.token)} />
            </div>
            <div className="row small muted">
              Created {fmtRelative(s.created_at)}
              <span className="spacer" />
              <button
                className="btn ghost small danger"
                onClick={async () => {
                  try {
                    await api.deleteShare(projectId, s.id);
                    invalidate(`shares:${projectId}`);
                    shares.reload();
                    toast("Share link revoked");
                  } catch (e) {
                    toast(errorMessage(e), true);
                  }
                }}
              >
                Revoke
              </button>
            </div>
          </div>
        ))}
        {shares.data && mine.length === 0 && (
          <button
            className="btn primary"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await api.createShare(projectId, "dashboard", dashboard.id);
                invalidate(`shares:${projectId}`);
                shares.reload();
              } catch (e) {
                toast(errorMessage(e), true);
              } finally {
                setBusy(false);
              }
            }}
          >
            <Icon name="share" size={14} /> Create share link
          </button>
        )}
      </div>
    </Modal>
  );
}

export function DashboardPage({ id }: { id: string }) {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const { data, error, setData, reload } = useApi(`dashboard:${projectId}:${id}`, (s) => api.dashboard(projectId, id, s), { keepPrevious: false });
  const [range, setRange] = useState<RangeValue | null>(null);
  const [sharing, setSharing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);

  if (error) {
    return (
      <div className="page">
        <ErrorState error={error} retry={reload} />
      </div>
    );
  }
  if (!data) {
    return (
      <div className="page">
        <Skeleton height={28} width={280} />
        <div className="dash-grid mt-24">
          {[0, 1, 2].map((i) => (
            <div key={i} className="card" style={{ gridColumn: "span 4", gridRow: "span 3" }}>
              <div style={{ padding: 16 }}>
                <ChartSkeleton height={280} />
              </div>
            </div>
          ))}
        </div>
      </div>
    );
  }

  const tiles = [...data.tiles].sort((a, b) => a.y - b.y || a.x - b.x);
  const saveTiles = async (next: DashboardTile[]) => {
    const prev = data;
    setData({ ...data, tiles: next });
    try {
      const updated = await api.replaceTiles(
        projectId,
        id,
        next.map(({ insight_id, x, y, w, h }) => ({ insight_id, x, y, w, h })),
      );
      setData({ ...updated, tiles: updated.tiles.map((t) => ({ ...t, insight: t.insight ?? next.find((n) => n.insight_id === t.insight_id)?.insight })) });
      invalidate(`dashboards:${projectId}`);
    } catch (e) {
      setData(prev);
      toast(errorMessage(e), true);
    }
  };

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <div className="row small muted" style={{ marginBottom: 2 }}>
            <Link to={path("dashboards")} className="link">
              Dashboards
            </Link>
            <Icon name="chevronRight" size={12} />
          </div>
          <h1>
            {editable ? (
              <InlineEdit
                value={data.name}
                ariaLabel="Dashboard name"
                onSave={async (name) => {
                  try {
                    const d = await api.renameDashboard(projectId, id, name);
                    setData({ ...data, name: d.name });
                    invalidate(`dashboards:${projectId}`);
                  } catch (e) {
                    toast(errorMessage(e), true);
                  }
                }}
              />
            ) : (
              data.name
            )}
          </h1>
        </div>
        <div className="actions">
          <DateRangePicker value={range ?? { date_from: "-7d", date_to: null }} onChange={setRange} />
          {range && (
            <button className="btn ghost small" onClick={() => setRange(null)} title="Use each insight's own date range">
              Reset dates
            </button>
          )}
          <button className="btn icon" onClick={() => setRefreshKey((k) => k + 1)} title="Recompute every tile" aria-label="Refresh all">
            <Icon name="refresh" />
          </button>
          {editable && (
            <button className="btn" onClick={() => setSharing(true)}>
              <Icon name="share" size={14} /> Share
            </button>
          )}
          {editable && (
            <MenuButton label={<Icon name="more" />} className="btn icon" align="end" title="More actions">
              {(close) => (
                <button
                  className="menu-item danger"
                  onClick={() => {
                    close();
                    setDeleting(true);
                  }}
                >
                  <Icon name="trash" size={14} /> Delete dashboard
                </button>
              )}
            </MenuButton>
          )}
        </div>
      </div>

      {!range && tiles.length > 0 && <p className="muted small" style={{ marginTop: -8, marginBottom: 12 }}>Each tile uses its own date range. Pick a range above to override all tiles.</p>}

      {tiles.length === 0 ? (
        <div className="card">
          <Empty icon="dashboard" title="This dashboard is empty" action={<Link className="btn primary" to={path("insights")}>Go to insights</Link>}>
            Open any insight and choose “Add to dashboard” to pin it here.
          </Empty>
        </div>
      ) : (
        <div className="dash-grid">
          {tiles.map((t, i) => (
            <Tile
              key={`${t.insight_id}-${i}`}
              tile={t}
              range={range}
              editable={editable}
              refreshKey={refreshKey}
              onRemove={() => saveTiles(tiles.filter((_, j) => j !== i))}
              onResize={(w, h) => saveTiles(tiles.map((x, j) => (j === i ? { ...x, w, h } : x)))}
            />
          ))}
        </div>
      )}

      {sharing && <ShareModal dashboard={data} onClose={() => setSharing(false)} />}
      {deleting && (
        <Confirm
          title="Delete dashboard?"
          body={`“${data.name}” will be deleted. The insights on it are kept.`}
          confirmLabel="Delete"
          danger
          onClose={() => setDeleting(false)}
          onConfirm={async () => {
            await api.deleteDashboard(projectId, id);
            invalidate(`dashboards:${projectId}`);
            toast("Dashboard deleted");
            navigate(path("dashboards"));
          }}
        />
      )}
    </div>
  );
}
