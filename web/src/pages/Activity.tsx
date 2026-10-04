// Live event feed: newest first, polled, expandable to the full payload.

import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Combobox, ComboboxContent, ComboboxEmpty, ComboboxInput, ComboboxItem, ComboboxList } from "@/components/ui/combobox";
import { Switch } from "@/components/ui/switch";
import { Seg } from "@/components/controls";
import { CopyButton, JsonView } from "@/components/copy";
import { columnHelper, DataTable } from "@/components/data-table";
import { Empty, ErrorState, SkeletonRows } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { Page, PageHeader, Panel, SearchInput, Toolbar } from "@/components/page";
import { errorMessage } from "@/lib/api";
import { usePath, useProjectId } from "@/lib/context";
import { fmtDateTime, fmtRelative } from "@/lib/format";
import { useDebounced, useNow } from "@/lib/hooks";
import { eventLabel } from "@/lib/properties";
import { catalogEventsQuery, eventsQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { EventRow } from "@/types/EventRow";
import { LoadDemoButton } from "@/pages/Onboarding";
import { KV } from "@/components/page";

const route = getRouteApi("/project/$projectId/activity");

/** Rows that arrive by polling stay highlighted this long. */
const FRESH_MS = 2200;
/** Scrolling to the end loads older pages on its own up to this many pages; past it, only the button does. */
export const AUTO_PAGES = 5;
const POLL_MS = 5000;

function str(v: unknown): string {
  if (v === null || v === undefined) return "";
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}

function sortedKeys(props: Record<string, unknown>): string[] {
  return Object.keys(props).sort((a, b) => Number(a.startsWith("$")) - Number(b.startsWith("$")) || a.localeCompare(b));
}

export function EventDetail({ event }: { event: EventRow }) {
  const [mode, setMode] = useState<"table" | "json">("table");
  const props = event.properties ?? {};
  const items: [ReactNode, ReactNode][] = [
    ["event", event.event],
    ["distinct_id", event.distinct_id],
    ["timestamp", event.timestamp],
    ...sortedKeys(props).map((k): [ReactNode, ReactNode] => [k, str(props[k]) || <span className="text-muted-foreground">""</span>]),
  ];
  return (
    <div className="flex flex-col gap-3 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <Seg
          label="View"
          value={mode}
          onChange={setMode}
          options={[
            { value: "table", label: "Properties" },
            { value: "json", label: "JSON" },
          ]}
        />
        <span className="flex-1" />
        <span className="font-mono text-xs text-muted-foreground">{event.uuid}</span>
        <CopyButton text={JSON.stringify(event, null, 2)} label="Copy event" />
      </div>
      <div className="max-h-96 overflow-auto">{mode === "json" ? <JsonView value={event} /> : <KV items={items} />}</div>
    </div>
  );
}

/**
 * Calls `onEnd` when any scroll box inside (the virtualized table) is scrolled near
 * its bottom. Scroll events don't bubble, so this listens in the capture phase.
 */
export function ScrollEnd({ onEnd, children, className }: { onEnd: () => void; children: ReactNode; className?: string }) {
  return (
    <div
      className={className}
      onScrollCapture={(e) => {
        const el = e.target;
        if (!(el instanceof HTMLElement)) return;
        if (el.scrollHeight - el.scrollTop - el.clientHeight < 240) onEnd();
      }}
    >
      {children}
    </div>
  );
}

const col = columnHelper<EventRow>();

export function EventTable({
  events,
  showPerson = true,
  fresh,
  maxHeight = 640,
}: {
  events: EventRow[];
  showPerson?: boolean;
  fresh?: ReadonlySet<string>;
  maxHeight?: number;
}) {
  const path = usePath();
  const [open, setOpen] = useState<string | null>(null);
  const now = useNow(5000);

  const columns = useMemo(
    () => [
      col.display({
        id: "expand",
        header: () => <span className="sr-only">Details</span>,
        cell: ({ row }) => {
          const expanded = open === row.original.uuid;
          return (
            <>
              <Icon name={expanded ? "chevronDown" : "chevronRight"} size={13} className="text-muted-foreground" />
              <span className="sr-only">{expanded ? "Collapse event" : "Expand event"}</span>
            </>
          );
        },
        meta: { className: "w-7 pr-0" },
      }),
      col.accessor("event", {
        header: "Event",
        cell: (c) => {
          const name = c.getValue();
          const label = eventLabel(name);
          return (
            <span className="inline-flex items-baseline gap-2 font-medium">
              {label}
              {label !== name && <span className="font-mono text-xs font-normal text-muted-foreground">{name}</span>}
            </span>
          );
        },
      }),
      ...(showPerson
        ? [
            col.accessor("distinct_id", {
              header: "Person",
              cell: ({ row }) => (
                <Link
                  to={path(`persons/${encodeURIComponent(row.original.person_id)}`)}
                  className="block max-w-56 truncate font-mono text-xs text-brand-foreground hover:underline"
                  onClick={(ev) => ev.stopPropagation()}
                >
                  {row.original.distinct_id}
                </Link>
              ),
            }),
          ]
        : []),
      col.display({
        id: "url",
        header: "URL / screen",
        cell: ({ row }) => {
          const p = row.original.properties ?? {};
          const url = str(p.$pathname) || str(p.$current_url) || str(p.$screen_name);
          return (
            <span className="block max-w-80 truncate text-muted-foreground" title={url}>
              {url || "–"}
            </span>
          );
        },
      }),
      col.display({
        id: "lib",
        header: "Library",
        cell: ({ row }) => <span className="text-xs whitespace-nowrap text-muted-foreground">{str((row.original.properties ?? {}).$lib) || "–"}</span>,
      }),
      col.accessor("timestamp", {
        header: "Time",
        cell: (c) => (
          <time dateTime={c.getValue()} className="whitespace-nowrap text-muted-foreground" title={fmtDateTime(c.getValue())}>
            {fmtRelative(c.getValue(), now)}
          </time>
        ),
        meta: { align: "right" },
      }),
    ],
    [showPerson, open, path, now],
  );

  return (
    <DataTable
      label="Events"
      columns={columns}
      data={events}
      getRowId={(e) => e.uuid}
      onRowClick={(e) => setOpen((cur) => (cur === e.uuid ? null : e.uuid))}
      expandedId={open}
      renderExpanded={(e) => <EventDetail event={e} />}
      rowClassName={(e) => cn("transition-colors duration-700", fresh?.has(e.uuid) && "bg-brand-wash hover:bg-brand-wash")}
      virtualize={{ maxHeight, estimateRowHeight: 41 }}
      dense
    />
  );
}

/** Event-name filter: a Base UI combobox over the catalog (plus the active value, even if the catalog doesn't list it). */
function EventFilter({ value, onChange }: { value: string | null; onChange: (v: string | null) => void }) {
  const pid = useProjectId();
  const { data } = useQuery(catalogEventsQuery(pid, ""));
  const items = useMemo(() => {
    const names = [...(data ?? [])].sort((a, b) => b.count - a.count).map((e) => e.name);
    return value && !names.includes(value) ? [value, ...names] : names;
  }, [data, value]);
  return (
    <Combobox
      items={items}
      value={value}
      onValueChange={(v) => onChange(typeof v === "string" ? v : null)}
      itemToStringLabel={(v: string) => eventLabel(v)}
      filter={(item: string, query: string) => {
        const q = query.trim().toLowerCase();
        return !q || item.toLowerCase().includes(q) || eventLabel(item).toLowerCase().includes(q);
      }}
    >
      <ComboboxInput placeholder="All events" aria-label="Filter by event" showTrigger={false} showClear={!!value} className="w-64" />
      <ComboboxContent className="w-96">
        <ComboboxEmpty>No matching events</ComboboxEmpty>
        <ComboboxList>
          {(item: string) => (
            <ComboboxItem key={item} value={item}>
              <span className="truncate">{eventLabel(item)}</span>
              {eventLabel(item) !== item && <span className="truncate font-mono text-xs text-muted-foreground">{item}</span>}
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  );
}

export function ActivityPage() {
  const pid = useProjectId();
  const path = usePath();
  const search = route.useSearch();
  const navigate = route.useNavigate();
  const event = search.event ?? null;
  const personId = search.person_id ?? null;
  const [live, setLive] = useState(true);
  const [personInput, setPersonInput] = useState(personId ?? "");
  const debouncedPerson = useDebounced(personInput.trim(), 400);

  // URL to input (links from a person page) and input to URL (debounced).
  useEffect(() => setPersonInput(personId ?? ""), [personId]);
  useEffect(() => {
    if (debouncedPerson !== (personId ?? "")) void navigate({ search: { event: event ?? undefined, person_id: debouncedPerson || undefined }, replace: true });
    // Only the debounced input drives the URL; `personId`/`event` changes are handled above and by setEvent.
  }, [debouncedPerson]);

  const setEvent = (e: string | null) => void navigate({ search: { event: e ?? undefined, person_id: personId ?? undefined }, replace: true });

  const q = useInfiniteQuery(eventsQuery(pid, { event, personId }, live ? POLL_MS : false));
  const { data, error, isPending, isFetching, isPlaceholderData, fetchNextPage, hasNextPage, isFetchingNextPage, refetch } = q;

  const pages = data?.pages ?? [];
  const events = useMemo(() => {
    const seen = new Set<string>();
    const out: EventRow[] = [];
    for (const p of pages) {
      for (const e of p.events) {
        if (!seen.has(e.uuid)) {
          seen.add(e.uuid);
          out.push(e);
        }
      }
    }
    return out;
  }, [pages]);

  // Highlight rows that arrived since the last poll (first page only: older pages are history).
  const filterKey = `${event ?? ""}\u0000${personId ?? ""}`;
  const seenIds = useRef<{ key: string; ids: Set<string> } | null>(null);
  const [fresh, setFresh] = useState<ReadonlySet<string>>(new Set());
  const firstPage = pages[0];
  useEffect(() => {
    if (!firstPage || isPlaceholderData) return;
    const ids = firstPage.events.map((e) => e.uuid);
    const prev = seenIds.current;
    if (!prev || prev.key !== filterKey) {
      seenIds.current = { key: filterKey, ids: new Set(ids) };
      return;
    }
    const added = ids.filter((id) => !prev.ids.has(id));
    if (!added.length) return;
    for (const id of ids) prev.ids.add(id);
    setFresh(new Set(added));
    const t = window.setTimeout(() => setFresh(new Set()), FRESH_MS);
    return () => window.clearTimeout(t);
  }, [firstPage, isPlaceholderData, filterKey]);

  const loadOlder = () => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  };
  const filtered = !!(event || personId);

  return (
    <Page>
      <PageHeader
        title="Activity"
        sub="Every event as it lands, newest first."
        actions={
          <>
            <label className="flex items-center gap-2 text-muted-foreground">
              <Switch checked={live} onCheckedChange={setLive} aria-label="Live updates" />
              {live ? (
                <span className="flex items-center gap-1.5">
                  <span className="size-1.5 rounded-full bg-good motion-safe:animate-pulse" aria-hidden="true" /> Live
                </span>
              ) : (
                "Paused"
              )}
            </label>
            <Button variant="outline" size="icon" aria-label="Refresh" title="Refresh" onClick={() => void refetch()} disabled={isFetching && !isPending}>
              <Icon name="refresh" className={isFetching ? "motion-safe:animate-spin" : undefined} />
            </Button>
          </>
        }
      />

      <Toolbar>
        <EventFilter value={event} onChange={setEvent} />
        <SearchInput
          wrapperClassName="w-72"
          placeholder="Person ID"
          aria-label="Filter by person ID"
          value={personInput}
          onChange={(e) => setPersonInput(e.target.value)}
        />
        {filtered && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setPersonInput("");
              void navigate({ search: {}, replace: true });
            }}
          >
            Clear
          </Button>
        )}
      </Toolbar>

      <Panel>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} />
        ) : isPending ? (
          <SkeletonRows rows={10} />
        ) : events.length === 0 ? (
          filtered ? (
            <Empty icon="search" title="No events match these filters" />
          ) : (
            <Empty
              icon="activity"
              title="No events yet"
              action={
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <LoadDemoButton />
                  <Button nativeButton={false} render={<Link to={path("onboarding")} />}>Connect your app</Button>
                </div>
              }
            >
              Point a PostHog SDK at this server and events show up here within seconds. This page refreshes on its own.
            </Empty>
          )
        ) : (
          <ScrollEnd onEnd={() => pages.length < AUTO_PAGES && loadOlder()}>
            <EventTable events={events} fresh={fresh} />
          </ScrollEnd>
        )}
      </Panel>
      {hasNextPage && events.length > 0 && (
        <div className="mt-4 flex flex-wrap items-center justify-center gap-3">
          <Button variant="outline" onClick={loadOlder} disabled={isFetchingNextPage}>
            {isFetchingNextPage ? "Loading…" : "Load older events"}
          </Button>
        </div>
      )}
      {q.isFetchNextPageError && <p className="mt-2 text-center text-sm text-destructive">{errorMessage(q.error)}</p>}
    </Page>
  );
}
