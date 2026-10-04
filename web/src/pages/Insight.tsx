// The insight builder: editor on the left, live result on the right.
// New insights keep their query in the URL (?kind=&q=, replace-navigation) so
// a reload never loses work; saved insights load from the API once and track
// "unsaved changes" locally.

import { useCallback, useEffect, useRef, useState } from "react";
import { getRouteApi, Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ChartDisplay } from "@/types/ChartDisplay";
import type { InsightQuery } from "@/types/InsightQuery";
import type { Interval } from "@/types/Interval";
import { api, errorMessage, type SavedInsight, type ShareLink } from "@/lib/api";
import { canEdit, useApp, usePath, useProjectId } from "@/lib/context";
import { autoInterval, fmtNumber, fmtRelative } from "@/lib/format";
import { navigate as go } from "@/lib/nav";
import { dashboardsQuery, insightQuery, qk, sharesQuery } from "@/lib/queries";
import { Icon } from "@/components/icons";
import { Badge } from "@/components/ui/badge";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Toggle } from "@/components/ui/toggle";
import { CopyButton } from "@/components/copy";
import { AppDialog, Confirm } from "@/components/dialogs";
import { DateRangePicker } from "@/components/date-range";
import { Empty, ErrorState, LoadingBar, Skeleton } from "@/components/feedback";
import { InlineEdit } from "@/components/inline-edit";
import { CardBar, CardPad, FormField, Page, PageHeader, Panel } from "@/components/page";
import { toast } from "@/components/toast";
import { KINDS, defaultName, defaultQuery, kindFromSlug, kindInfo, normalizeQuery } from "@/insight/defaults";
import { QueryEditor } from "@/insight/Editor";
import { OptionSelect } from "@/insight/pickers";
import { ChartSkeleton, InsightResultView, useInsightQuery } from "@/insight/Result";
import { SqlEditor } from "@/insight/SqlEditor";

const newRoute = getRouteApi("/project/$projectId/insights/new");
const savedRoute = getRouteApi("/project/$projectId/insights/$id");

const DISPLAYS: { value: ChartDisplay; label: string }[] = [
  { value: "ActionsLineGraph", label: "Line" },
  { value: "ActionsAreaGraph", label: "Area" },
  { value: "ActionsBar", label: "Bar" },
  { value: "ActionsBarValue", label: "Total value" },
  { value: "ActionsTable", label: "Table" },
  { value: "ActionsPie", label: "Pie" },
  { value: "BoldNumber", label: "Number" },
];

const INTERVALS: { value: Interval; label: string }[] = [
  { value: "hour", label: "Hourly" },
  { value: "day", label: "Daily" },
  { value: "week", label: "Weekly" },
  { value: "month", label: "Monthly" },
];

// ── Routes ───────────────────────────────────────────────────────────────

/** Bound on remembered URL writes, so the set can never grow. */
const WRITTEN_MAX = 8;

/** What the router will hand back for a query we wrote: the same JSON, normalized. */
function searchSignature(kindSlug: string | undefined, q: InsightQuery | undefined): string {
  return JSON.stringify([kindSlug ?? null, q ? normalizeQuery(JSON.parse(JSON.stringify(q))) : null]);
}

export function NewInsightPage() {
  const search = newRoute.useSearch();
  const navigate = newRoute.useNavigate();
  // Our own debounced writes come back as search changes; only a change we did
  // not write (a fresh "New insight" link) resets the builder.
  const written = useRef<string[]>([]);
  const [seen, setSeen] = useState({ sig: searchSignature(search.kind, search.q), epoch: 0 });
  const sig = searchSignature(search.kind, search.q);
  if (sig !== seen.sig) setSeen({ sig, epoch: written.current.includes(sig) ? seen.epoch : seen.epoch + 1 });

  const persist = useCallback(
    (q: InsightQuery) => {
      const kind = kindInfo(q.kind).slug;
      written.current = [searchSignature(kind, q), ...written.current].slice(0, WRITTEN_MAX);
      void navigate({ search: { kind, q }, replace: true });
    },
    [navigate],
  );
  const initial = search.q ?? defaultQuery(kindFromSlug(search.kind ?? null));
  return <InsightEditor key={`new:${seen.epoch}`} id={null} initial={initial} persist={persist} />;
}

export function SavedInsightPage() {
  const { id } = savedRoute.useParams();
  return <InsightEditor key={id} id={id} />;
}

// ── Add to dashboard ─────────────────────────────────────────────────────

export function AddToDashboard({ ensureSaved }: { ensureSaved: () => Promise<SavedInsight | null> }) {
  const projectId = useProjectId();
  const path = usePath();
  const queryClient = useQueryClient();
  const dashboards = useQuery(dashboardsQuery(projectId));
  const [naming, setNaming] = useState(false);
  const add = useMutation({
    mutationFn: async ({ dashboardId, name }: { dashboardId: string | null; name?: string }) => {
      const insight = await ensureSaved();
      if (!insight) return null;
      let dash = dashboardId ? await api.dashboard(projectId, dashboardId) : await api.createDashboard(projectId, name?.trim() || "My dashboard");
      const bottom = dash.tiles.reduce((m, t) => Math.max(m, t.y + t.h), 0);
      const tiles = [...dash.tiles.map(({ insight_id, x, y, w, h }) => ({ insight_id, x, y, w, h })), { insight_id: insight.id, x: 0, y: bottom, w: 6, h: 3 }];
      dash = await api.replaceTiles(projectId, dash.id, tiles);
      return dash;
    },
    onSuccess: async (dash) => {
      if (!dash) return;
      await queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId) });
      toast(`Added to ${dash.name}`);
      go(path(`dashboards/${dash.id}`));
    },
    onError: (e) => toast(errorMessage(e), true),
  });
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger render={<Button variant="outline" disabled={add.isPending} />}>
          <Icon name="dashboard" size={14} /> Add to dashboard
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuGroup>
            <DropdownMenuLabel>Dashboards</DropdownMenuLabel>
          </DropdownMenuGroup>
          {dashboards.error ? (
            <div className="p-1">
              <ErrorState error={dashboards.error} retry={() => void dashboards.refetch()} compact />
            </div>
          ) : null}
          {(dashboards.data ?? []).map((d) => (
            <DropdownMenuItem key={d.id} onClick={() => add.mutate({ dashboardId: d.id })}>
              <Icon name="dashboard" size={14} />
              <span className="min-w-0 flex-1 truncate">{d.name}</span>
              <span className="text-xs text-muted-foreground">{d.tiles.length} {d.tiles.length === 1 ? "tile" : "tiles"}</span>
            </DropdownMenuItem>
          ))}
          {dashboards.data?.length === 0 && <div className="px-2 pt-1 pb-2 text-muted-foreground">No dashboards yet.</div>}
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setNaming(true)}>
            <Icon name="plus" size={14} /> New dashboard…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {naming && (
        <NewDashboardDialog
          onClose={() => setNaming(false)}
          onCreate={(name) => {
            setNaming(false);
            add.mutate({ dashboardId: null, name });
          }}
        />
      )}
    </>
  );
}

function NewDashboardDialog({ onClose, onCreate }: { onClose: () => void; onCreate: (name: string) => void }) {
  const [name, setName] = useState("My dashboard");
  return (
    <AppDialog
      title="New dashboard"
      description="The insight is saved and added as its first tile."
      onClose={onClose}
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" form="new-dashboard-form">
            Create and add
          </Button>
        </>
      }
    >
      <form
        id="new-dashboard-form"
        onSubmit={(e) => {
          e.preventDefault();
          onCreate(name);
        }}
      >
        <FormField label="Dashboard name" htmlFor="new-dashboard-name">
          <Input id="new-dashboard-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} maxLength={120} />
        </FormField>
      </form>
    </AppDialog>
  );
}

// ── Share ────────────────────────────────────────────────────────────────

function ShareDialog({ insight, onClose }: { insight: SavedInsight; onClose: () => void }) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  const shares = useQuery(sharesQuery(projectId));
  const mine: ShareLink[] = (shares.data ?? []).filter((s) => s.object_type === "insight" && s.object_id === insight.id);
  const url = (token: string) => `${window.location.origin}/share/${token}`;
  const refresh = () => queryClient.invalidateQueries({ queryKey: qk.shares(projectId) });
  const create = useMutation({
    mutationFn: () => api.createShare(projectId, "insight", insight.id),
    onSuccess: refresh,
    onError: (e) => toast(errorMessage(e), true),
  });
  const revoke = useMutation({
    mutationFn: (id: string) => api.deleteShare(projectId, id),
    onSuccess: async () => {
      await refresh();
      toast("Share link revoked");
    },
    onError: (e) => toast(errorMessage(e), true),
  });
  return (
    <AppDialog title={`Share “${insight.name}”`} onClose={onClose}>
      <div className="flex flex-col gap-4">
        <p className="text-muted-foreground">Anyone with a share link can view this insight without signing in. Revoke a link to cut access immediately.</p>
        {shares.error ? <ErrorState error={shares.error} retry={() => void shares.refetch()} compact /> : null}
        {mine.map((s) => (
          <div key={s.id} className="flex flex-col gap-1.5">
            <div className="flex items-center gap-2 rounded-lg bg-muted px-2.5 py-1.5">
              <code className="min-w-0 flex-1 truncate">{url(s.token)}</code>
              <CopyButton text={url(s.token)} />
            </div>
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              Created {fmtRelative(s.created_at)}
              <span className="flex-1" />
              <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" disabled={revoke.isPending} onClick={() => revoke.mutate(s.id)}>
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

// ── Result toolbar ───────────────────────────────────────────────────────

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
      {"interval" in query && <OptionSelect<Interval> size="sm" label="Interval" value={query.interval} options={INTERVALS} onChange={(interval) => onChange({ ...query, interval })} />}
      {query.kind === "TrendsQuery" && (
        <>
          <Toggle variant="outline" size="sm" pressed={query.compare} onPressedChange={(compare) => onChange({ ...query, compare })} title="Compare to the previous period">
            Compare
          </Toggle>
          <OptionSelect<ChartDisplay> size="sm" label="Chart type" value={query.display} options={DISPLAYS} onChange={(display) => onChange({ ...query, display })} />
        </>
      )}
    </>
  );
}

function ResultMeta({ elapsed, cached }: { elapsed: number; cached: boolean }) {
  return (
    <span className="num flex items-center gap-1.5 text-xs text-muted-foreground" title={cached ? "Served from cache; the data version hasn't changed" : "Computed fresh"}>
      <Icon name={cached ? "bolt" : "clock"} size={12} />
      {cached ? "cached" : `${fmtNumber(elapsed)} ms`}
    </span>
  );
}

// ── The editor ───────────────────────────────────────────────────────────

function InsightEditor({ id, initial, persist }: { id: string | null; initial?: InsightQuery; persist?: (q: InsightQuery) => void }) {
  const projectId = useProjectId();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const path = usePath();
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const saved = useQuery({ ...insightQuery(projectId, id ?? ""), enabled: id !== null });
  const [query, setQuery] = useState<InsightQuery | null>(initial ?? null);
  const untouched = useRef(query);
  const [name, setName] = useState("");
  const [dirty, setDirty] = useState(id === null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [sqlRun, setSqlRun] = useState<InsightQuery | null>(null);

  // Load a saved insight into the editor once; later refetches never clobber edits.
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

  // New insights mirror their query into the URL (debounced) so a reload never loses work.
  useEffect(() => {
    if (id || !query || !persist || query === untouched.current) return;
    const t = window.setTimeout(() => persist(query), 400);
    return () => window.clearTimeout(t);
  }, [id, query, persist]);

  const isSql = query?.kind === "SqlQuery";
  const run = useInsightQuery(isSql ? sqlRun : query, isSql ? 0 : 350);

  const update = (q: InsightQuery) => {
    setQuery(q);
    setDirty(true);
  };

  const saveMutation = useMutation({
    mutationFn: (draft: { name: string; description: string; query: InsightQuery }) => (id ? api.updateInsight(projectId, id, draft) : api.createInsight(projectId, draft)),
  });

  const save = async (): Promise<SavedInsight | null> => {
    if (!query) return null;
    if (!editable) {
      toast("Only project owners and admins can save insights.", true);
      return null;
    }
    try {
      const result = await saveMutation.mutateAsync({ name: name.trim() || defaultName(query), description: "", query });
      queryClient.setQueryData(qk.insight(projectId, result.id), result);
      // The insight list, this insight and any dashboard tile showing it share these prefixes.
      void queryClient.invalidateQueries({ queryKey: qk.insights(projectId) });
      void queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId) });
      setName(result.name);
      setDirty(false);
      toast(id ? "Insight saved" : "Insight created");
      if (!id) {
        loadedFor.current = result.id;
        go(path(`insights/${result.id}`), { replace: true });
      }
      return result;
    } catch (e) {
      toast(errorMessage(e), true);
      return null;
    }
  };

  // Ctrl/Cmd+S saves, wherever focus is. Always calls the latest `save`.
  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const deleteMutation = useMutation({
    mutationFn: () => api.deleteInsight(projectId, id as string),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: qk.insights(projectId) });
      void queryClient.invalidateQueries({ queryKey: qk.dashboards(projectId) });
      toast("Insight deleted");
      go(path("insights"));
    },
  });

  if (id && saved.error) {
    return (
      <Page>
        <ErrorState error={saved.error} retry={() => void saved.refetch()} />
      </Page>
    );
  }
  if (id && !query) {
    if (saved.data && saved.data.query === null) {
      return (
        <Page narrow>
          <Empty
            icon="alert"
            title={`“${saved.data.name}” uses a legacy query format`}
            action={
              <Button render={<Link to={path("insights/new")} />}>Build a new insight</Button>
            }
          >
            This insight was saved before the current query model and can't be opened in the builder.
          </Empty>
        </Page>
      );
    }
    return (
      <Page>
        <Skeleton className="h-7 w-80" />
        <div className="mt-6 grid gap-4 lg:grid-cols-[380px_minmax(0,1fr)]">
          <Skeleton className="h-[420px]" />
          <Skeleton className="h-[420px]" />
        </div>
      </Page>
    );
  }
  if (!query) return null;

  const switchKind = (slug: string) => {
    const kind = kindFromSlug(slug);
    if (kind === query.kind) return;
    update(defaultQuery(kind));
    setSqlRun(null);
  };

  const meta = isSql && !sqlRun ? undefined : run.data?.meta;
  const resultBody = (
    <div className={`relative p-4 ${isSql ? "min-h-[200px]" : "min-h-[380px]"}`}>
      <LoadingBar show={run.pending && !!run.data} />
      {run.hint ? (
        <Empty icon="info" title={run.hint} />
      ) : isSql && !sqlRun ? (
        <Empty icon="play" title="Run the query to see results">
          Press{" "}
          <KbdGroup>
            <Kbd>Ctrl</Kbd>
            <Kbd>Enter</Kbd>
          </KbdGroup>{" "}
          in the editor.
        </Empty>
      ) : run.error && !run.pending ? (
        <ErrorState error={run.error} retry={run.refresh} />
      ) : run.data && run.sent ? (
        <div className={run.pending ? "opacity-55 transition-opacity" : "transition-opacity"}>
          <InsightResultView query={run.sent} result={run.data.result} />
        </div>
      ) : (
        <ChartSkeleton />
      )}
    </div>
  );

  return (
    <Page>
      <Breadcrumb className="mb-1 flex min-h-6 items-center">
        <BreadcrumbList className="text-xs">
          <BreadcrumbItem>
            <BreadcrumbLink render={<Link to={path("insights")} />}>Insights</BreadcrumbLink>
          </BreadcrumbItem>
          <BreadcrumbSeparator />
          <BreadcrumbItem>
            <BreadcrumbPage>{id ? "Saved insight" : "New insight"}</BreadcrumbPage>
          </BreadcrumbItem>
          {dirty && id && (
            <Badge variant="secondary" className="ml-1 bg-warn-wash text-warn">
              Unsaved changes
            </Badge>
          )}
        </BreadcrumbList>
      </Breadcrumb>
      <PageHeader
        title={
          <InlineEdit
            value={name}
            placeholder={defaultName(query)}
            ariaLabel="Insight name"
            onSave={(v) => {
              setName(v);
              setDirty(true);
            }}
          />
        }
        sub={saved.data ? <span className="text-xs">Last saved {fmtRelative(saved.data.updated_at)}</span> : undefined}
        actions={
          <>
            <AddToDashboard ensureSaved={async () => (id && !dirty && saved.data ? saved.data : save())} />
            {id && saved.data && editable && (
              <Button variant="outline" onClick={() => setSharing(true)}>
                <Icon name="share" size={14} /> Share
              </Button>
            )}
            {id && (
              <DropdownMenu>
                <DropdownMenuTrigger render={<Button variant="outline" size="icon" aria-label="More actions" title="More actions" />}>
                  <Icon name="more" />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-48">
                  <DropdownMenuItem
                    onClick={() =>
                      void navigate({
                        to: "/project/$projectId/insights/new",
                        params: (prev) => ({ ...prev, projectId }),
                        search: { kind: kindInfo(query.kind).slug, q: query },
                      })
                    }
                  >
                    <Icon name="copy" size={14} /> Duplicate as new
                  </DropdownMenuItem>
                  {editable && (
                    <DropdownMenuItem variant="destructive" onClick={() => setConfirmDelete(true)}>
                      <Icon name="trash" size={14} /> Delete insight
                    </DropdownMenuItem>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
            <Button onClick={() => void save()} disabled={saveMutation.isPending || (!dirty && !!id)} title="Save (Ctrl+S)" aria-keyshortcuts="Control+S Meta+S">
              <Icon name="save" size={14} /> {saveMutation.isPending ? "Saving…" : id ? "Save" : "Save insight"}
            </Button>
          </>
        }
      />

      <Tabs value={kindInfo(query.kind).slug} onValueChange={(v) => switchKind(String(v))} className="mb-4">
        <TabsList variant="line" aria-label="Insight type" className="h-9 max-w-full justify-start gap-1 overflow-x-auto border-b">
          {KINDS.map((k) => (
            <TabsTrigger key={k.kind} value={k.slug} title={k.blurb} className="flex-none px-3">
              <Icon name={k.icon} size={14} /> {k.label}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>

      {isSql && query.kind === "SqlQuery" ? (
        <div className="flex flex-col gap-4">
          <Panel>
            <CardBar>
              <h2 className="text-sm">SQL</h2>
              <span className="text-xs text-muted-foreground">
                Read-only. One table: <code>events</code> (uuid, event, distinct_id, person_id, timestamp, properties). Capped in rows, time and memory.
              </span>
            </CardBar>
            <CardPad className="flex flex-col gap-3">
              <SqlEditor value={query.query} onChange={(v) => update({ ...query, query: v })} onRun={() => setSqlRun({ ...query })} />
              <div className="flex flex-wrap items-center gap-2.5">
                <Button onClick={() => (sqlRun && sqlRun.kind === "SqlQuery" && sqlRun.query === query.query ? run.refresh() : setSqlRun({ ...query }))}>
                  <Icon name="play" size={13} /> Run
                </Button>
                <KbdGroup className="text-xs text-muted-foreground">
                  <Kbd>Ctrl</Kbd>
                  <Kbd>Enter</Kbd>
                </KbdGroup>
                <span className="flex-1" />
                {meta && <ResultMeta elapsed={meta.elapsed_ms} cached={meta.cached} />}
              </div>
            </CardPad>
          </Panel>
          <Panel>{resultBody}</Panel>
        </div>
      ) : (
        <div className="grid items-start gap-4 lg:grid-cols-[380px_minmax(0,1fr)]">
          <Panel>
            <QueryEditor query={query} onChange={update} />
          </Panel>
          <Panel className="relative">
            <CardBar>
              <ResultToolbar query={query} onChange={update} />
              <span className="flex-1" />
              {meta && <ResultMeta elapsed={meta.elapsed_ms} cached={meta.cached} />}
              <Button variant="ghost" size="icon-sm" title="Recompute, bypassing the cache" aria-label="Refresh" onClick={run.refresh} disabled={!!run.hint}>
                <Icon name="refresh" size={14} />
              </Button>
            </CardBar>
            {resultBody}
          </Panel>
        </div>
      )}

      {sharing && saved.data && <ShareDialog insight={saved.data} onClose={() => setSharing(false)} />}
      {confirmDelete && id && (
        <Confirm
          title="Delete insight?"
          body={`“${name}” will be removed from every dashboard. This can't be undone.`}
          confirmLabel="Delete"
          danger
          onClose={() => setConfirmDelete(false)}
          onConfirm={() => deleteMutation.mutateAsync()}
        />
      )}
    </Page>
  );
}
