// Public share view. Resolves the capability token; renders what it can.

import { api, type SavedInsight } from "../lib/api";
import { useApi } from "../lib/hooks";
import { Logo, Icon } from "../ui/icons";
import { ErrorState, Skeleton } from "../ui/kit";
import { kindInfo, summarize } from "../insight/defaults";

export function SharePage({ token }: { token: string }) {
  const { data, error, reload } = useApi(`share:${token}`, (s) => api.publicShare(token, s));
  const items = data?.dashboard ? data.dashboard.tiles.map((t) => t.insight).filter((i): i is SavedInsight => !!i) : data?.insight ? [data.insight] : [];
  return (
    <div className="auth" style={{ placeItems: "start center" }}>
      <div style={{ width: "min(960px, 100%)" }} className="col gap-16">
        <div className="row">
          <Logo size={26} />
          <b>Hoglet</b>
          <span className="muted small">· shared view</span>
        </div>
        {error ? (
          <div className="card">
            <ErrorState error={error} retry={reload} />
          </div>
        ) : !data ? (
          <Skeleton height={240} />
        ) : (
          <div className="card">
            <div className="card-head">
              <h2>{data.dashboard?.name ?? data.insight?.name}</h2>
            </div>
            <div className="list-card">
              {items.map((i) => (
                <div key={i.id} className="item">
                  <span className="kind-icon">
                    <Icon name={i.query ? kindInfo(i.query.kind).icon : "alert"} size={14} />
                  </span>
                  <span className="col" style={{ gap: 0 }}>
                    <b>{i.name}</b>
                    <span className="muted small">{summarize(i.query)}</span>
                  </span>
                </div>
              ))}
            </div>
            <div className="notice" style={{ margin: 16 }}>
              <Icon name="info" />
              Live charts on public links need the server's public query endpoint, which this build doesn't provide yet.
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
