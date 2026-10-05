// Public share view. Resolves the capability token and shows what the link
// covers: the dashboard or insight and what each tile measures.
//
// TODO(decisions.md, pending boss decision): live charts on a public link need a
// public, read-only query endpoint scoped to the share token. Hoglet has none, and
// adding one is a decision for the boss (it exposes query results without a
// session). Until it is decided and built, this page deliberately renders
// metadata only; do not add an unauthenticated query route from here.

import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { ErrorState, Notice, Skeleton } from "@/components/feedback";
import { Icon, Logo } from "@/components/icons";
import { Badge } from "@/components/ui/badge";
import { IconBadge, Panel } from "@/components/page";
import type { SavedInsight } from "@/lib/api";
import { fmtDate, rangeLabel } from "@/lib/format";
import { publicShareQuery } from "@/lib/queries";
import { kindInfo, summarize } from "@/insight/defaults";

/** What an insight measures, without exposing SQL text on a public page. */
function measures(i: SavedInsight): string {
  if (!i.query) return "Saved before typed queries";
  return i.query.kind === "SqlQuery" ? "Custom SQL query" : summarize(i.query);
}

function rangeOf(i: SavedInsight): string | null {
  const q = i.query;
  return q && "date_range" in q ? rangeLabel(q.date_range.date_from, q.date_range.date_to) : null;
}

function TileCard({ insight }: { insight: SavedInsight }) {
  const kind = insight.query ? kindInfo(insight.query.kind) : null;
  const range = rangeOf(insight);
  return (
    <li className="flex gap-3 rounded-xl bg-card p-4 ring-1 ring-foreground/10">
      <IconBadge className="size-9">
        <Icon name={kind ? kind.icon : "alert"} size={16} />
      </IconBadge>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <h2 className="truncate text-sm">{insight.name}</h2>
        <p className="text-xs text-muted-foreground">{measures(insight)}</p>
        <div className="mt-1 flex flex-wrap items-center gap-1.5">
          {kind ? <Badge variant="secondary">{kind.label}</Badge> : null}
          {range ? <Badge variant="secondary">{range}</Badge> : null}
        </div>
        {insight.description ? <p className="mt-1 text-xs">{insight.description}</p> : null}
      </div>
    </li>
  );
}

export function SharePage() {
  const { token } = useParams({ from: "/share/$token" });
  const { data, error, refetch } = useQuery(publicShareQuery(token));
  const items = data?.dashboard ? data.dashboard.tiles.map((t) => t.insight).filter((i): i is SavedInsight => !!i) : data?.insight ? [data.insight] : [];
  const title = data?.dashboard?.name ?? data?.insight?.name;
  const isDashboard = !!data?.dashboard;
  return (
    <div className="min-h-screen bg-background px-3.5 py-10 text-foreground md:px-7">
      <main className="mx-auto flex w-full max-w-[960px] flex-col gap-4">
        <div className="flex items-center gap-2">
          <Logo size={26} />
          <b>Hoglet</b>
          <span className="text-xs text-muted-foreground">· shared view</span>
        </div>
        {error && !data ? (
          <Panel>
            <ErrorState error={error} retry={() => void refetch()} />
          </Panel>
        ) : !data ? (
          <div aria-busy="true" aria-label="Loading shared view" className="flex flex-col gap-3">
            <Skeleton className="h-8 w-64" />
            <Skeleton className="h-16 w-full" />
            <Skeleton className="h-24 w-full" />
          </div>
        ) : (
          <>
            <header className="flex flex-col gap-1">
              <h1 className="text-[22px]">{title}</h1>
              <p className="text-xs text-muted-foreground">
                Shared {isDashboard ? "dashboard" : "insight"}
                {data.share.created_at ? ` · link created ${fmtDate(data.share.created_at * 1000)}` : ""}
                {data.share.expires_at ? ` · expires ${fmtDate(data.share.expires_at * 1000)}` : " · no expiry"}
              </p>
            </header>
            <Notice>
              This link lists what {isDashboard ? "the dashboard" : "the insight"} covers: each insight's name, type, what it measures and its date range. Live charts and numbers are not
              published on share links yet; ask the owner to add you to their Hoglet to see them.
            </Notice>
            {items.length === 0 ? (
              <Panel>
                <p className="p-6 text-center text-muted-foreground">{isDashboard ? "This dashboard has no insights yet." : "Nothing to show for this link."}</p>
              </Panel>
            ) : (
              <ul className="m-0 grid list-none gap-3 p-0 sm:grid-cols-2">
                {items.map((i) => (
                  <TileCard key={i.id} insight={i} />
                ))}
              </ul>
            )}
          </>
        )}
      </main>
    </div>
  );
}
