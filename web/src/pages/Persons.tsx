import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Avatar } from "@/components/avatar";
import { TabsBar } from "@/components/controls";
import { CopyButton } from "@/components/copy";
import { columnHelper, DataTable } from "@/components/data-table";
import { AppDialog } from "@/components/dialogs";
import { Empty, ErrorState, Notice, Skeleton, SkeletonRows } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { CardBar, CardPad, KV, Page, PageHeader, Panel, SearchInput, StatLabel } from "@/components/page";
import { toast } from "@/components/toast";
import { Badge } from "@/components/ui/badge";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, errorMessage } from "@/lib/api";
import { canEdit, useApp, usePath, useProjectId } from "@/lib/context";
import { fmtDateTime, fmtNumber, fmtRelative } from "@/lib/format";
import { useDebounced } from "@/lib/hooks";
import { navigate } from "@/lib/nav";
import { personEventsQuery, personQuery, personsQuery, qk } from "@/lib/queries";
import type { PersonSummary } from "@/types/PersonSummary";
import { AUTO_PAGES, EventTable, ScrollEnd } from "@/pages/Activity";
import { FirstRunEmpty } from "@/components/first-run";

const personRoute = getRouteApi("/project/$projectId/persons/$id");

function IdentifiedBadge({ identified }: { identified: boolean }) {
  return identified ? <Badge className="bg-brand-wash text-brand-foreground">identified</Badge> : <Badge variant="secondary">anonymous</Badge>;
}

const col = columnHelper<PersonSummary>();

export function PersonsPage() {
  const pid = useProjectId();
  const path = usePath();
  const [search, setSearch] = useState("");
  const q = useDebounced(search.trim(), 250);
  const { data, error, isPending, isPlaceholderData, refetch, fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = useInfiniteQuery(personsQuery(pid, q));

  const pages = data?.pages ?? [];
  const persons = useMemo(() => {
    const seen = new Set<string>();
    return pages.flatMap((p) => p.persons).filter((p) => !seen.has(p.id) && seen.add(p.id));
  }, [pages]);

  const columns = useMemo(
    () => [
      col.accessor("display_name", {
        header: "Person",
        cell: ({ row }) => {
          const p = row.original;
          return (
            <div className="flex items-center gap-2.5">
              <Avatar name={p.display_name} id={p.id} />
              <Link
                to={path(`persons/${encodeURIComponent(p.id)}`)}
                onClick={(e) => e.stopPropagation()}
                className="max-w-72 truncate font-semibold hover:text-brand-foreground hover:underline"
              >
                {p.display_name}
              </Link>
              <IdentifiedBadge identified={p.is_identified} />
            </div>
          );
        },
      }),
      col.accessor((p) => p.distinct_ids[0] ?? "", {
        id: "distinct_id",
        header: "Distinct ID",
        cell: ({ row }) => {
          const ids = row.original.distinct_ids;
          return (
            <span className="inline-flex items-center gap-1.5 font-mono text-xs text-muted-foreground">
              <span className="max-w-64 truncate">{ids[0]}</span>
              {ids.length > 1 && <Badge variant="secondary">+{ids.length - 1}</Badge>}
            </span>
          );
        },
      }),
      col.accessor("created_at", {
        header: "Created",
        cell: (c) => <span className="whitespace-nowrap text-muted-foreground">{fmtRelative(c.getValue())}</span>,
      }),
      col.accessor((p) => p.last_seen ?? "", {
        id: "last_seen",
        header: "Last seen",
        cell: ({ row }) => <span className="whitespace-nowrap text-muted-foreground">{row.original.last_seen ? fmtRelative(row.original.last_seen) : "–"}</span>,
        meta: { align: "right" },
      }),
    ],
    [path],
  );

  const loadMore = () => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  };

  return (
    <Page>
      <PageHeader title="Persons" sub="Everyone who sent an event, merged across devices by identify and alias." />
      <Panel>
        <CardBar>
          <SearchInput
            wrapperClassName="w-full sm:w-96"
            placeholder="Search by email, name or distinct ID…"
            aria-label="Search persons"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            autoFocus
          />
          <span className="flex-1" />
          {isPlaceholderData && <span className="text-xs text-muted-foreground">Searching…</span>}
        </CardBar>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} />
        ) : isPending ? (
          <SkeletonRows rows={10} />
        ) : persons.length === 0 ? (
          q ? (
            <Empty icon="search" title={`No one matches “${q}”`} />
          ) : (
            <FirstRunEmpty icon="users" title="No persons yet">
              Persons appear when events arrive. Call <code>posthog.identify(userId, {"{ email }"})</code> after login to link anonymous visits to a known user.
            </FirstRunEmpty>
          )
        ) : (
          <ScrollEnd onEnd={() => pages.length < AUTO_PAGES && loadMore()}>
            <DataTable
              label="Persons"
              columns={columns}
              data={persons}
              getRowId={(p) => p.id}
              onRowClick={(p) => navigate(path(`persons/${encodeURIComponent(p.id)}`))}
              sortable
              virtualize={{ maxHeight: 640 }}
            />
          </ScrollEnd>
        )}
      </Panel>
      {hasNextPage && persons.length > 0 && (
        <div className="mt-4 flex flex-wrap items-center justify-center gap-3">
          <Button variant="outline" onClick={loadMore} disabled={isFetchingNextPage}>
            {isFetchingNextPage ? "Loading…" : "Load more"}
          </Button>
        </div>
      )}
      {isFetchNextPageError && <p className="mt-2 text-center text-sm text-destructive">{errorMessage(error)}</p>}
    </Page>
  );
}

function str(v: unknown): string {
  if (v === null || v === undefined) return "";
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}

function PersonEvents({ personId }: { personId: string }) {
  const pid = useProjectId();
  const { data, error, isPending, refetch, fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = useInfiniteQuery(personEventsQuery(pid, personId));
  const pages = data?.pages ?? [];
  const events = useMemo(() => {
    const seen = new Set<string>();
    return pages.flatMap((p) => p.events).filter((e) => !seen.has(e.uuid) && seen.add(e.uuid));
  }, [pages]);
  const loadOlder = () => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  };
  if (error && !data) return <ErrorState error={error} retry={() => void refetch()} />;
  if (isPending) return <SkeletonRows rows={8} />;
  if (events.length === 0) return <Empty icon="activity" title="No events for this person in the stored range" />;
  return (
    <>
      <ScrollEnd onEnd={() => pages.length < AUTO_PAGES && loadOlder()}>
        <EventTable events={events} showPerson={false} />
      </ScrollEnd>
      {(hasNextPage || isFetchNextPageError) && (
        <div className="flex flex-wrap items-center justify-center gap-3 border-t p-3">
          {hasNextPage && (
            <Button variant="outline" onClick={loadOlder} disabled={isFetchingNextPage}>
              {isFetchingNextPage ? "Loading…" : "Load older"}
            </Button>
          )}
          {isFetchNextPageError && <span className="text-sm text-destructive">{errorMessage(error)}</span>}
        </div>
      )}
    </>
  );
}

function ErasePerson({ personId, name, onClose }: { personId: string; name: string; onClose: () => void }) {
  const pid = useProjectId();
  const path = usePath();
  const queryClient = useQueryClient();
  const [typed, setTyped] = useState("");
  const ok = typed.trim() === name.trim();
  const erase = useMutation({
    mutationFn: () => api.erasePerson(pid, personId),
    onSuccess: async (r) => {
      toast(`Erased ${fmtNumber(r.events)} events and ${fmtNumber(r.distinct_ids)} distinct IDs`);
      // Leave the page first: dropping the person's queries while this page still observes them would refetch a 404.
      onClose();
      navigate(path("persons"));
      window.setTimeout(() => queryClient.removeQueries({ queryKey: qk.person(pid, personId) }), 500);
      // The persons list, the activity feed and the status counters all changed.
      await Promise.all([qk.persons(pid), qk.events(pid), qk.status(pid)].map((queryKey) => queryClient.invalidateQueries({ queryKey })));
    },
  });
  const close = () => {
    if (!erase.isPending) onClose();
  };
  return (
    <AppDialog
      title="Delete person and all their data"
      onClose={close}
      footer={
        <>
          <Button variant="outline" onClick={close} disabled={erase.isPending}>
            Cancel
          </Button>
          <Button variant="destructive" form="erase-person-form" type="submit" disabled={!ok || erase.isPending}>
            {erase.isPending ? "Erasing…" : "Erase permanently"}
          </Button>
        </>
      }
    >
      <form
        id="erase-person-form"
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (ok && !erase.isPending) erase.mutate();
        }}
      >
        <Notice tone="bad">
          This erases <b>{name}</b>, every distinct ID merged into them, and every event they sent, from stored data. It can't be undone. Use it for GDPR and
          similar deletion requests.
        </Notice>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="erase-confirm" className="text-sm">
            Type <code>{name}</code> to confirm
          </label>
          <Input
            id="erase-confirm"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            aria-label="Type the person's name to confirm"
          />
        </div>
        {erase.isError && <Notice tone="bad">{errorMessage(erase.error)}</Notice>}
      </form>
    </AppDialog>
  );
}

type Tab = "events" | "properties" | "ids";

export function PersonPage() {
  const { id } = personRoute.useParams();
  const pid = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const { data, error, refetch } = useQuery(personQuery(pid, id));
  const [tab, setTab] = useState<Tab>("events");
  const [erasing, setErasing] = useState(false);
  const [propSearch, setPropSearch] = useState("");

  const p = data?.person;
  const props = useMemo(() => (p?.properties ?? {}) as Record<string, unknown>, [p]);
  const keys = useMemo(() => {
    const needle = propSearch.toLowerCase();
    return Object.keys(props)
      .filter((k) => !needle || k.toLowerCase().includes(needle) || str(props[k]).toLowerCase().includes(needle))
      .sort((a, b) => Number(a.startsWith("$")) - Number(b.startsWith("$")) || a.localeCompare(b));
  }, [props, propSearch]);

  if (error && !data) {
    return (
      <Page>
        <ErrorState error={error} retry={() => void refetch()} />
      </Page>
    );
  }

  const stats = [
    { label: "Events", value: data ? fmtNumber(data.event_count) : null },
    { label: "Sessions", value: data ? fmtNumber(data.session_count) : null },
    { label: "First seen", value: data ? fmtDateTime(data.first_seen ?? p?.created_at) : null },
    { label: "Last seen", value: data ? fmtRelative(data.last_seen) : null },
  ];

  return (
    <Page>
      <Breadcrumb className="mb-2.5">
        <BreadcrumbList>
          <BreadcrumbItem>
            <BreadcrumbLink render={<Link to={path("persons")} />}>Persons</BreadcrumbLink>
          </BreadcrumbItem>
          <BreadcrumbSeparator />
          <BreadcrumbItem>
            <BreadcrumbPage className="max-w-64 truncate">{p?.display_name ?? "…"}</BreadcrumbPage>
          </BreadcrumbItem>
        </BreadcrumbList>
      </Breadcrumb>
      <PageHeader
        leading={p ? <Avatar name={p.display_name} id={p.id} large /> : <Skeleton className="size-[52px] rounded-full" />}
        title={p ? p.display_name : <Skeleton className="h-7 w-64" />}
        sub={
          <div className="flex flex-wrap items-center gap-2">
            {p && <IdentifiedBadge identified={p.is_identified} />}
            <span className="font-mono text-xs">{id}</span>
            <CopyButton text={id} label="" title="Copy person ID" />
          </div>
        }
        actions={
          <Button variant="outline" nativeButton={false} render={<Link to={path("activity")} search={{ person_id: id }} />}>
            <Icon name="activity" size={14} /> Live activity
          </Button>
        }
      />

      <Panel className="mb-4">
        <div className="grid grid-cols-2 gap-4 p-4 md:grid-cols-4">
          {stats.map((s) => (
            <div key={s.label}>
              <StatLabel className="mb-1">{s.label}</StatLabel>
              <div className="num text-lg font-semibold">{s.value ?? <Skeleton className="h-5 w-20" />}</div>
            </div>
          ))}
        </div>
      </Panel>

      <TabsBar
        value={tab}
        onChange={setTab}
        options={[
          { value: "events", label: "Events" },
          { value: "properties", label: `Properties${p ? ` (${Object.keys(props).length})` : ""}` },
          { value: "ids", label: `Distinct IDs${data ? ` (${data.distinct_ids.length})` : ""}` },
        ]}
      />
      <Panel>
        {tab === "events" && <PersonEvents personId={id} />}
        {tab === "properties" && (
          <CardPad className="flex flex-col gap-3">
            <SearchInput
              wrapperClassName="w-full sm:w-80"
              placeholder="Search properties…"
              aria-label="Search properties"
              value={propSearch}
              onChange={(e) => setPropSearch(e.target.value)}
            />
            {!data ? (
              <SkeletonRows rows={6} className="px-0" />
            ) : keys.length === 0 ? (
              <Empty icon="info" title={propSearch ? "No matching properties" : "No person properties set"}>
                {!propSearch && (
                  <>
                    Set them with <code>posthog.identify(id, {"{ email, plan }"})</code> or <code>$set</code>.
                  </>
                )}
              </Empty>
            ) : (
              <KV items={keys.map((k) => [k, str(props[k])])} />
            )}
          </CardPad>
        )}
        {tab === "ids" && (
          <div>
            {!data ? (
              <SkeletonRows rows={3} />
            ) : (
              <ul className="m-0 list-none divide-y p-0">
                {data.distinct_ids.map((d) => (
                  <li key={d} className="flex items-center gap-2 px-4 py-1.5">
                    <span className="min-w-0 flex-1 truncate font-mono text-xs">{d}</span>
                    <CopyButton text={d} label="" title={`Copy ${d}`} />
                  </li>
                ))}
              </ul>
            )}
            <p className="border-t px-4 py-2.5 text-xs text-muted-foreground">
              Every distinct ID merged into this person by <code>$identify</code>, <code>$create_alias</code> or <code>$merge_dangerously</code>.
            </p>
          </div>
        )}
      </Panel>

      {canEdit(organization) && p && (
        <Panel className="mt-6 ring-destructive/30">
          <CardPad className="flex flex-wrap items-center gap-4">
            <div className="min-w-0 flex-1">
              <h2 className="text-sm">Delete person and all their data</h2>
              <p className="text-sm text-muted-foreground">Erases this person, their distinct IDs and every event they sent. For GDPR deletion requests.</p>
            </div>
            <Button variant="destructive" onClick={() => setErasing(true)}>
              <Icon name="trash" size={14} /> Delete person
            </Button>
          </CardPad>
        </Panel>
      )}
      {erasing && p && <ErasePerson personId={p.id} name={p.display_name} onClose={() => setErasing(false)} />}
    </Page>
  );
}
