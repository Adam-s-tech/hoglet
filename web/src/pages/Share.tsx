// Public share view. Resolves the capability token; renders what it can.

import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { ErrorState, Notice, Skeleton } from "@/components/feedback";
import { Icon, Logo } from "@/components/icons";
import { CardBar, IconBadge, Panel } from "@/components/page";
import type { SavedInsight } from "@/lib/api";
import { publicShareQuery } from "@/lib/queries";
import { kindInfo, summarize } from "@/insight/defaults";

export function SharePage() {
  const { token } = useParams({ from: "/share/$token" });
  const { data, error, refetch } = useQuery(publicShareQuery(token));
  const items = data?.dashboard ? data.dashboard.tiles.map((t) => t.insight).filter((i): i is SavedInsight => !!i) : data?.insight ? [data.insight] : [];
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
          <Skeleton className="h-60 w-full" />
        ) : (
          <Panel>
            <CardBar>
              <h2>{data.dashboard?.name ?? data.insight?.name}</h2>
            </CardBar>
            <ul className="m-0 list-none divide-y p-0">
              {items.map((i) => (
                <li key={i.id} className="flex items-center gap-3 px-4 py-3">
                  <IconBadge>
                    <Icon name={i.query ? kindInfo(i.query.kind).icon : "alert"} size={14} />
                  </IconBadge>
                  <span className="flex min-w-0 flex-col">
                    <b className="truncate">{i.name}</b>
                    <span className="truncate text-xs text-muted-foreground">{summarize(i.query)}</span>
                  </span>
                </li>
              ))}
            </ul>
            <div className="p-4">
              <Notice>Live charts on public links need the server's public query endpoint, which this build doesn't provide yet.</Notice>
            </div>
          </Panel>
        )}
      </main>
    </div>
  );
}
