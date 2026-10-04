import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { columnHelper, DataTable } from "@/components/data-table";
import { AppDialog, Confirm } from "@/components/dialogs";
import { CopyButton } from "@/components/copy";
import { DateRangePicker, type RangeValue } from "@/components/date-range";
import { Empty, ErrorState, LoadingBar, Skeleton, SkeletonRows } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { InlineEdit } from "@/components/inline-edit";
import { FormField, Page, PageHeader, Panel, SearchInput } from "@/components/page";
import { toast } from "@/components/toast";
import { ChartSkeleton, InsightResultView } from "@/insight/Result";
import { incomplete, kindInfo, sanitize, summarize, withDateRange } from "@/insight/defaults";
import { api, errorMessage, type Dashboard, type DashboardTile, type SavedInsight, type TileInput } from "@/lib/api";
import { Gated } from "@/components/role";
import { canEdit, projectPath, useApp, usePath, useProjectId } from "@/lib/context";
import { fmtDate, fmtRelative } from "@/lib/format";
import { navigate } from "@/lib/nav";
import { dashboardQuery, dashboardsQuery, insightsQuery, qk, sharesQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";

const dashboardRoute = getRouteApi("/project/$projectId/dashboards/$id");

// ── List ─────────────────────────────────────────────────────────────────

const col = columnHelper<Dashboard>();

export function DashboardsPage() {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const queryClient = useQueryClient();
  const { data, error, isPending, refetch } = useQuery(dashboardsQuery(projectId));
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");

  const create = useMutation({
    mutationFn: (n: string) => api.createDashboard(projectId, n.trim() || "New dashboard"),
    onSuccess: async (d) => {
      await queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId) });
      setCreating(false);
      setName("");
      navigate(path(`dashboards/${d.id}`));
    },
    onError: (e) => toast(errorMessage(e), true),
  });

  const columns = useMemo(
    () => [
      col.accessor("name", {
        header: "Name",
        cell: (c) => (
          <div className="flex items-center gap-3">
            <span className="inline-grid size-7 flex-none place-items-center rounded-md bg-muted text-muted-foreground">
              <Icon name="dashboard" size={14} />
            </span>
            <Link
              to={projectPath(projectId, `dashboards/${c.row.original.id}`)}
              onClick={(e) => e.stopPropagation()}
              className="truncate font-semibold hover:text-brand-foreground hover:underline"
            >
              {c.getValue()}
            </Link>
          </div>
        ),
      }),
      col.accessor((d) => d.tiles.length, {
        id: "tiles",
        header: "Tiles",
        cell: (c) => <span className="num">{c.getValue()}</span>,
        meta: { align: "right" },
      }),
      col.accessor("created_at", {
        header: "Created",
        cell: (c) => <span className="num text-muted-foreground">{fmtDate(c.getValue())}</span>,
        meta: { align: "right" },
      }),
    ],
    [projectId],
  );

  const newButton = (label: string) => (
    <Button onClick={() => setCreating(true)}>
      <Icon name="plus" size={14} /> {label}
    </Button>
  );

  return (
    <Page>
      <PageHeader title="Dashboards" sub="Insights side by side, live, on one screen." actions={<Gated allowed={editable}>{newButton("New dashboard")}</Gated>} />
      <Panel>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} />
        ) : isPending ? (
          <SkeletonRows rows={4} />
        ) : data && data.length === 0 ? (
          <Empty icon="dashboard" title="No dashboards yet" action={<Gated allowed={editable}>{newButton("Create a dashboard")}</Gated>}>
            Create one, then use “Add to dashboard” on any insight to pin it.
          </Empty>
        ) : (
          <DataTable
            label="Dashboards"
            columns={columns}
            data={data ?? []}
            getRowId={(d) => d.id}
            onRowClick={(d) => navigate(path(`dashboards/${d.id}`))}
            sortable
          />
        )}
      </Panel>
      {creating && (
        <AppDialog
          title="New dashboard"
          onClose={() => {
            if (!create.isPending) setCreating(false);
          }}
          footer={
            <>
              <Button variant="outline" onClick={() => setCreating(false)} disabled={create.isPending}>
                Cancel
              </Button>
              <Button type="submit" form="new-dashboard-form" disabled={create.isPending}>
                Create
              </Button>
            </>
          }
        >
          <form
            id="new-dashboard-form"
            onSubmit={(e) => {
              e.preventDefault();
              if (!create.isPending) create.mutate(name);
            }}
          >
            <FormField label="Name" htmlFor="new-dashboard-name">
              <Input id="new-dashboard-name" autoFocus value={name} maxLength={120} placeholder="Product health" onChange={(e) => setName(e.target.value)} />
            </FormField>
          </form>
        </AppDialog>
      )}
    </Page>
  );
}

// ── Tile ─────────────────────────────────────────────────────────────────

const SIZES = [
  { label: "Small", w: 4, h: 3 },
  { label: "Half width", w: 6, h: 3 },
  { label: "Full width", w: 12, h: 4 },
];

interface TileProps {
  tile: DashboardTile;
  range: RangeValue | null;
  editable: boolean;
  refreshKey: number;
  first: boolean;
  last: boolean;
  onRemove: () => void;
  onResize: (w: number, h: number) => void;
  onMove: (delta: -1 | 1) => void;
}

function Tile({ tile, range, editable, refreshKey, first, last, onRemove, onResize, onMove }: TileProps) {
  const projectId = useProjectId();
  const path = usePath();
  const insight = tile.insight;
  const query = insight?.query ? withDateRange(insight.query, range) : null;
  const hint = query ? incomplete(query) : null;
  const clean = query && !hint ? sanitize(query) : null;
  const json = clean ? JSON.stringify(clean) : null;

  // Same key as insightResultQuery, so the insight page and its dashboard share one cache entry.
  const bypassCache = useRef(false);
  const run = useQuery({
    queryKey: [...qk.query(projectId), json] as const,
    queryFn: ({ signal }) => {
      const refresh = bypassCache.current;
      bypassCache.current = false;
      return api.query(projectId, { query: clean!, refresh }, signal);
    },
    enabled: clean !== null,
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  });
  const { refetch } = run;
  const seen = useRef(refreshKey);
  useEffect(() => {
    if (seen.current === refreshKey) return;
    seen.current = refreshKey;
    bypassCache.current = true;
    void refetch();
  }, [refreshKey, refetch]);

  const w = Math.max(3, Math.min(12, tile.w));
  const h = Math.max(2, Math.min(8, tile.h));
  const kind = insight?.query ? kindInfo(insight.query.kind) : null;
  const sizeValue = SIZES.find((s) => s.w === tile.w && s.h === tile.h)?.label ?? "";
  const name = insight?.name ?? "Missing insight";

  return (
    <Panel
      className="min-h-0 [grid-column:span_12] lg:[grid-column:span_var(--w)] [grid-row:span_var(--h)]"
      style={{ "--w": w, "--h": h } as CSSProperties}
    >
      <div className="flex min-h-11 items-center gap-2.5 py-2 pr-2 pl-4">
        <span className="inline-grid size-6 flex-none place-items-center rounded-md bg-muted text-muted-foreground">
          <Icon name={kind?.icon ?? "alert"} size={13} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <Link to={path(`insights/${tile.insight_id}`)} className="truncate font-semibold hover:text-brand-foreground hover:underline">
            {name}
          </Link>
          {insight ? <span className="truncate text-xs text-muted-foreground">{summarize(insight.query)}</span> : null}
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label={`Actions for ${name}`} />}>
            <Icon name="more" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-56">
            <DropdownMenuItem onClick={() => navigate(path(`insights/${tile.insight_id}`))}>
              <Icon name="external" size={14} /> Open insight
            </DropdownMenuItem>
            {editable && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuGroup>
                  <DropdownMenuLabel>Size</DropdownMenuLabel>
                  <DropdownMenuRadioGroup
                    value={sizeValue}
                    onValueChange={(label) => {
                      const s = SIZES.find((x) => x.label === label);
                      if (s) onResize(s.w, s.h);
                    }}
                  >
                    {SIZES.map((s) => (
                      <DropdownMenuRadioItem key={s.label} value={s.label} closeOnClick>
                        {s.label}
                      </DropdownMenuRadioItem>
                    ))}
                  </DropdownMenuRadioGroup>
                </DropdownMenuGroup>
                <DropdownMenuSeparator />
                <DropdownMenuItem disabled={first} onClick={() => onMove(-1)}>
                  <Icon name="arrowUp" size={14} /> Move earlier
                </DropdownMenuItem>
                <DropdownMenuItem disabled={last} onClick={() => onMove(1)}>
                  <Icon name="arrowDown" size={14} /> Move later
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem variant="destructive" onClick={onRemove}>
                  <Icon name="x" size={14} /> Remove from dashboard
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <div className="relative min-h-0 flex-1 overflow-hidden border-t px-3.5 pt-2.5 pb-3">
        <LoadingBar show={run.isFetching && !!run.data} />
        {!insight?.query ? (
          <Empty icon="alert" title="This insight can't be rendered" />
        ) : hint ? (
          <Empty icon="info" title={hint} />
        ) : run.error && !run.data ? (
          <ErrorState error={run.error} retry={() => void refetch()} compact />
        ) : run.data && clean ? (
          <InsightResultView query={clean} result={run.data.result} compact />
        ) : (
          <ChartSkeleton height={Math.max(140, h * 110 - 90)} />
        )}
      </div>
    </Panel>
  );
}

// ── Add insight ──────────────────────────────────────────────────────────

function AddInsightSheet({ existing, onAdd, onClose }: { existing: Set<string>; onAdd: (i: SavedInsight) => void; onClose: () => void }) {
  const projectId = useProjectId();
  const { data, error, isPending, refetch } = useQuery(insightsQuery(projectId));
  const [search, setSearch] = useState("");
  const rows = useMemo(() => {
    const q = search.trim().toLowerCase();
    return (data ?? []).filter((i) => !q || `${i.name} ${summarize(i.query)}`.toLowerCase().includes(q));
  }, [data, search]);

  return (
    <Sheet
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <SheetContent className="w-full sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Add an insight</SheetTitle>
          <SheetDescription>Pin a saved insight to this dashboard.</SheetDescription>
        </SheetHeader>
        <div className="flex min-h-0 flex-1 flex-col gap-3 px-4 pb-4">
          <SearchInput placeholder="Search insights…" aria-label="Search insights" value={search} onChange={(e) => setSearch(e.target.value)} autoFocus />
          <div className="min-h-0 flex-1 overflow-y-auto rounded-lg border">
            {error && !data ? (
              <ErrorState error={error} retry={() => void refetch()} compact />
            ) : isPending ? (
              <SkeletonRows rows={5} />
            ) : rows.length === 0 ? (
              <Empty icon="trends" title={data && data.length > 0 ? "No insights match" : "No saved insights yet"} className="py-10">
                {data && data.length > 0 ? "Try a different search." : "Save an insight first, then pin it here."}
              </Empty>
            ) : (
              <ul className="divide-y">
                {rows.map((i) => {
                  const added = existing.has(i.id);
                  const k = i.query ? kindInfo(i.query.kind) : null;
                  return (
                    <li key={i.id} className="flex items-center gap-2.5 px-3 py-2">
                      <span className="inline-grid size-7 flex-none place-items-center rounded-md bg-muted text-muted-foreground">
                        <Icon name={k?.icon ?? "alert"} size={14} />
                      </span>
                      <div className="flex min-w-0 flex-1 flex-col">
                        <span className="truncate font-medium">{i.name}</span>
                        <span className="truncate text-xs text-muted-foreground">{summarize(i.query)}</span>
                      </div>
                      <Button variant="outline" size="sm" disabled={added || !i.query} onClick={() => onAdd(i)} aria-label={`Add ${i.name} to dashboard`}>
                        {added ? (
                          <>
                            <Icon name="check" size={14} /> Added
                          </>
                        ) : (
                          <>
                            <Icon name="plus" size={14} /> Add
                          </>
                        )}
                      </Button>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </div>
      </SheetContent>
    </Sheet>
  );
}

// ── Share ────────────────────────────────────────────────────────────────

function ShareDialog({ dashboard, onClose }: { dashboard: Dashboard; onClose: () => void }) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  const shares = useQuery(sharesQuery(projectId));
  const mine = (shares.data ?? []).filter((s) => s.object_type === "dashboard" && s.object_id === dashboard.id);
  const url = (token: string) => `${window.location.origin}/share/${token}`;

  const create = useMutation({
    mutationFn: () => api.createShare(projectId, "dashboard", dashboard.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: qk.shares(projectId) }),
    onError: (e) => toast(errorMessage(e), true),
  });
  const revoke = useMutation({
    mutationFn: (id: string) => api.deleteShare(projectId, id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: qk.shares(projectId) });
      toast("Share link revoked");
    },
    onError: (e) => toast(errorMessage(e), true),
  });

  return (
    <AppDialog
      title={`Share “${dashboard.name}”`}
      description="Anyone with a share link can view this dashboard without signing in. Revoke a link to cut access immediately."
      onClose={onClose}
    >
      <div className="flex flex-col gap-4">
        {shares.error && !shares.data ? <ErrorState error={shares.error} retry={() => void shares.refetch()} compact /> : null}
        {shares.isPending ? <Skeleton className="h-16" /> : null}
        {mine.map((s) => (
          <div key={s.id} className="flex flex-col gap-1.5">
            <div className="flex items-center gap-2 rounded-lg bg-muted py-1.5 pr-1.5 pl-3">
              <code className="min-w-0 flex-1 truncate font-mono text-[12.5px]">{url(s.token)}</code>
              <CopyButton text={url(s.token)} />
            </div>
            <div className="flex items-center text-xs text-muted-foreground">
              Created {fmtRelative(s.created_at)}
              <span className="flex-1" />
              <Button
                variant="ghost"
                size="sm"
                className="text-destructive hover:text-destructive"
                disabled={revoke.isPending}
                onClick={() => revoke.mutate(s.id)}
                aria-label="Revoke share link"
              >
                Revoke
              </Button>
            </div>
          </div>
        ))}
        {shares.data && mine.length === 0 && (
          <Button className="self-start" disabled={create.isPending} onClick={() => create.mutate()}>
            <Icon name="share" size={14} /> Create share link
          </Button>
        )}
      </div>
    </AppDialog>
  );
}

// ── Detail ───────────────────────────────────────────────────────────────

function tileInputs(tiles: DashboardTile[]): TileInput[] {
  return tiles.map(({ insight_id, x, y, w, h }) => ({ insight_id, x, y, w, h }));
}

export function DashboardPage() {
  const { id } = dashboardRoute.useParams();
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const queryClient = useQueryClient();
  const [gone, setGone] = useState(false);
  const { data, error, refetch } = useQuery({ ...dashboardQuery(projectId, id), enabled: !gone });
  const [range, setRange] = useState<RangeValue | null>(null);
  const [sharing, setSharing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [adding, setAdding] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const detailKey = qk.dashboard(projectId, id);

  /** Tile edits: optimistic in the cache, rolled back if the server says no. */
  const saveTiles = useMutation({
    mutationFn: (next: DashboardTile[]) => api.replaceTiles(projectId, id, tileInputs(next)),
    onMutate: async (next) => {
      await queryClient.cancelQueries({ queryKey: detailKey });
      const prev = queryClient.getQueryData<Dashboard>(detailKey);
      if (prev) queryClient.setQueryData<Dashboard>(detailKey, { ...prev, tiles: next });
      return { prev };
    },
    onError: (e, _next, ctx) => {
      if (ctx?.prev) queryClient.setQueryData(detailKey, ctx.prev);
      toast(errorMessage(e), true);
    },
    onSuccess: (updated, next) => {
      queryClient.setQueryData<Dashboard>(detailKey, {
        ...updated,
        tiles: updated.tiles.map((t) => ({ ...t, insight: t.insight ?? next.find((n) => n.insight_id === t.insight_id)?.insight })),
      });
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId) }),
  });

  const rename = useMutation({
    mutationFn: (name: string) => api.renameDashboard(projectId, id, name),
    onMutate: async (name) => {
      await queryClient.cancelQueries({ queryKey: detailKey });
      const prev = queryClient.getQueryData<Dashboard>(detailKey);
      if (prev) queryClient.setQueryData<Dashboard>(detailKey, { ...prev, name });
      return { prev };
    },
    onError: (e, _name, ctx) => {
      if (ctx?.prev) queryClient.setQueryData(detailKey, ctx.prev);
      toast(errorMessage(e), true);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId) }),
  });

  const tiles = useMemo(() => [...(data?.tiles ?? [])].sort((a, b) => a.y - b.y || a.x - b.x), [data]);

  if (error && !data) {
    return (
      <Page>
        <Panel>
          <ErrorState error={error} retry={() => void refetch()} />
        </Panel>
      </Page>
    );
  }
  if (!data) {
    return (
      <Page>
        <Skeleton className="mb-6 h-7 w-72" />
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-12">
          {[0, 1, 2].map((i) => (
            <Panel key={i} className="lg:col-span-4">
              <div className="p-4">
                <ChartSkeleton height={280} />
              </div>
            </Panel>
          ))}
        </div>
      </Page>
    );
  }

  const move = (index: number, delta: -1 | 1) => {
    const next = [...tiles];
    const [item] = next.splice(index, 1);
    next.splice(index + delta, 0, item);
    // Order is (y, x): renumber so the new order is explicit.
    saveTiles.mutate(next.map((t, i) => ({ ...t, x: 0, y: i })));
  };
  const addInsight = (insight: SavedInsight) => {
    const y = tiles.reduce((m, t) => Math.max(m, t.y), -1) + 1;
    saveTiles.mutate([...tiles, { insight_id: insight.id, x: 0, y, w: 6, h: 3, insight }]);
  };

  return (
    <Page>
      <Breadcrumb className="mb-1">
        <BreadcrumbList>
          <BreadcrumbItem>
            <BreadcrumbLink render={<Link to={path("dashboards")} />}>Dashboards</BreadcrumbLink>
          </BreadcrumbItem>
          <BreadcrumbSeparator />
          <BreadcrumbItem>
            <BreadcrumbPage className="max-w-64 truncate">{data.name}</BreadcrumbPage>
          </BreadcrumbItem>
        </BreadcrumbList>
      </Breadcrumb>
      <PageHeader
        title={editable ? <InlineEdit value={data.name} ariaLabel="Dashboard name" onSave={(name) => rename.mutate(name)} /> : data.name}
        actions={
          <>
            <DateRangePicker value={range ?? { date_from: "-7d", date_to: null }} onChange={setRange} />
            {range && (
              <Button variant="ghost" size="sm" onClick={() => setRange(null)} title="Use each insight's own date range">
                Reset dates
              </Button>
            )}
            <Button variant="outline" size="icon" onClick={() => setRefreshKey((k) => k + 1)} title="Recompute every tile" aria-label="Refresh all tiles">
              <Icon name="refresh" />
            </Button>
            {editable && (
              <Button variant="outline" onClick={() => setAdding(true)}>
                <Icon name="plus" size={14} /> Add insight
              </Button>
            )}
            {editable && (
              <Button variant="outline" onClick={() => setSharing(true)}>
                <Icon name="share" size={14} /> Share
              </Button>
            )}
            {editable && (
              <DropdownMenu>
                <DropdownMenuTrigger render={<Button variant="outline" size="icon" aria-label="More dashboard actions" />}>
                  <Icon name="more" />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem variant="destructive" onClick={() => setDeleting(true)}>
                    <Icon name="trash" size={14} /> Delete dashboard
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </>
        }
      />

      {!range && tiles.length > 0 && <p className="-mt-2 mb-3 text-xs text-muted-foreground">Each tile uses its own date range. Pick a range above to override all tiles.</p>}

      {tiles.length === 0 ? (
        <Panel>
          <Empty
            icon="dashboard"
            title="This dashboard is empty"
            action={
              <div className="flex flex-wrap justify-center gap-2">
                {editable && (
                  <Button onClick={() => setAdding(true)}>
                    <Icon name="plus" size={14} /> Add insight
                  </Button>
                )}
                <Button variant="outline" nativeButton={false} render={<Link to={path("insights")} />}>
                  Go to insights
                </Button>
              </div>
            }
          >
            Add a saved insight here, or open any insight and choose “Add to dashboard”.
          </Empty>
        </Panel>
      ) : (
        <div className={cn("grid grid-cols-12 gap-4 [grid-auto-flow:dense] auto-rows-[110px]", saveTiles.isPending && "opacity-95")}>
          {tiles.map((t, i) => (
            <Tile
              key={`${t.insight_id}-${i}`}
              tile={t}
              range={range}
              editable={editable}
              refreshKey={refreshKey}
              first={i === 0}
              last={i === tiles.length - 1}
              onRemove={() => saveTiles.mutate(tiles.filter((_, j) => j !== i))}
              onResize={(w, h) => saveTiles.mutate(tiles.map((x, j) => (j === i ? { ...x, w, h } : x)))}
              onMove={(delta) => move(i, delta)}
            />
          ))}
        </div>
      )}

      {adding && <AddInsightSheet existing={new Set(tiles.map((t) => t.insight_id))} onAdd={addInsight} onClose={() => setAdding(false)} />}
      {sharing && <ShareDialog dashboard={data} onClose={() => setSharing(false)} />}
      {deleting && (
        <Confirm
          title="Delete dashboard?"
          body={`“${data.name}” will be deleted. The insights on it are kept.`}
          confirmLabel="Delete"
          danger
          onClose={() => setDeleting(false)}
          onConfirm={async () => {
            await api.deleteDashboard(projectId, id);
            setGone(true);
            await queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId), exact: true });
            toast("Dashboard deleted");
            navigate(path("dashboards"));
            queryClient.removeQueries({ queryKey: detailKey, exact: true });
          }}
        />
      )}
    </Page>
  );
}
