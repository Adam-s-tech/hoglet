import { useState } from "react";
import { api } from "../lib/api";
import { usePath, useProjectId } from "../lib/context";
import { fmtRelative } from "../lib/format";
import { useApi } from "../lib/hooks";
import { Link, navigate } from "../lib/router";
import { Icon } from "../ui/icons";
import { Empty, ErrorState, MenuButton, SkeletonRows } from "../ui/kit";
import { KINDS, kindInfo, summarize } from "../insight/defaults";

export function NewInsightMenu({ primary = true }: { primary?: boolean }) {
  const path = usePath();
  return (
    <MenuButton
      className={`btn${primary ? " primary" : ""}`}
      align="end"
      label={
        <>
          <Icon name="plus" size={14} /> New insight
        </>
      }
    >
      {(close) =>
        KINDS.map((k) => (
          <button
            key={k.kind}
            className="menu-item"
            onClick={() => {
              close();
              navigate(`${path("insights/new")}?kind=${k.slug}`);
            }}
          >
            <span className="kind-icon">
              <Icon name={k.icon} size={14} />
            </span>
            <span className="col" style={{ gap: 0 }}>
              <b style={{ fontWeight: 600 }}>{k.label}</b>
              <span className="muted small">{k.blurb}</span>
            </span>
          </button>
        ))
      }
    </MenuButton>
  );
}

export function InsightsPage() {
  const projectId = useProjectId();
  const path = usePath();
  const { data, error, loading, reload } = useApi(`insights:${projectId}`, (s) => api.insights(projectId, s));
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState<string>("all");
  const list = (data ?? [])
    .filter((i) => kind === "all" || i.query?.kind === kind)
    .filter((i) => !search || `${i.name} ${summarize(i.query)}`.toLowerCase().includes(search.toLowerCase()))
    .sort((a, b) => b.updated_at - a.updated_at);

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>Insights</h1>
          <div className="sub">Saved questions about your product, answered live from your events.</div>
        </div>
        <div className="actions">
          <NewInsightMenu />
        </div>
      </div>

      <div className="grid-3" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(190px, 1fr))", marginBottom: 20 }}>
        {KINDS.map((k) => (
          <Link key={k.kind} to={`${path("insights/new")}?kind=${k.slug}`} className="card card-pad row" style={{ gap: 12, padding: "12px 14px" }}>
            <span className="kind-icon" style={{ background: "var(--accent-wash)", color: "var(--accent-ink)" }}>
              <Icon name={k.icon} size={15} />
            </span>
            <span className="col" style={{ gap: 0, minWidth: 0 }}>
              <b>{k.label}</b>
              <span className="muted small truncate">{k.blurb}</span>
            </span>
          </Link>
        ))}
      </div>

      <div className="card">
        <div className="card-head">
          <div className="search" style={{ width: 280 }}>
            <Icon name="search" size={14} />
            <input className="input" placeholder="Search insights…" value={search} onChange={(e) => setSearch(e.target.value)} aria-label="Search insights" />
          </div>
          <select className="select" value={kind} onChange={(e) => setKind(e.target.value)} aria-label="Insight type">
            <option value="all">All types</option>
            {KINDS.map((k) => (
              <option key={k.kind} value={k.kind}>
                {k.label}
              </option>
            ))}
          </select>
          <span className="spacer" />
          <span className="muted small">{data ? `${list.length} of ${data.length}` : ""}</span>
        </div>
        {error ? (
          <ErrorState error={error} retry={reload} />
        ) : !data && loading ? (
          <SkeletonRows rows={6} />
        ) : data && data.length === 0 ? (
          <Empty icon="trends" title="No saved insights yet" action={<NewInsightMenu />}>
            Build a trend, funnel or retention chart, then save it here to share and pin it on a dashboard.
          </Empty>
        ) : list.length === 0 ? (
          <Empty icon="search" title="Nothing matches" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Type</th>
                <th className="r">Last modified</th>
              </tr>
            </thead>
            <tbody>
              {list.map((i) => {
                const k = i.query ? kindInfo(i.query.kind) : null;
                return (
                  <tr key={i.id} className="clickable" onClick={() => navigate(path(`insights/${i.id}`))}>
                    <td>
                      <div className="row gap-12">
                        <span className="kind-icon">
                          <Icon name={k?.icon ?? "alert"} size={14} />
                        </span>
                        <div className="col" style={{ gap: 0, minWidth: 0 }}>
                          <Link to={path(`insights/${i.id}`)} onClick={(e) => e.stopPropagation()} style={{ fontWeight: 600 }}>
                            {i.name}
                          </Link>
                          <span className="muted small truncate" style={{ maxWidth: 560 }}>
                            {summarize(i.query)}
                          </span>
                        </div>
                      </div>
                    </td>
                    <td className="secondary">{k?.label ?? "Legacy"}</td>
                    <td className="r muted">{fmtRelative(i.updated_at)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
