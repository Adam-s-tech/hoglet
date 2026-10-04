import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { columnHelper, DataTable } from "@/components/data-table";
import { Empty, ErrorState, SkeletonRows } from "@/components/feedback";
import { Icon } from "@/components/icons";
import { CardBar, IconBadge, Page, PageHeader, Panel, SearchInput } from "@/components/page";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { usePath, useProjectId } from "@/lib/context";
import { fmtRelative } from "@/lib/format";
import { navigate } from "@/lib/nav";
import { insightsQuery } from "@/lib/queries";
import type { SavedInsight } from "@/lib/api";
import { KINDS, kindInfo, summarize } from "@/insight/defaults";

export function NewInsightMenu({ primary = true }: { primary?: boolean }) {
  const path = usePath();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button variant={primary ? "default" : "outline"} />}>
        <Icon name="plus" size={14} /> New insight
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        {KINDS.map((k) => (
          <DropdownMenuItem key={k.kind} className="gap-2.5 py-1.5" onClick={() => navigate(`${path("insights/new")}?kind=${k.slug}`)}>
            <IconBadge>
              <Icon name={k.icon} size={14} />
            </IconBadge>
            <span className="flex min-w-0 flex-col">
              <span className="font-semibold">{k.label}</span>
              <span className="truncate text-xs text-muted-foreground">{k.blurb}</span>
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const col = columnHelper<SavedInsight>();

const KIND_ITEMS = [{ value: "all", label: "All types" }, ...KINDS.map((k) => ({ value: k.kind as string, label: k.label }))];

export function InsightsPage() {
  const pid = useProjectId();
  const path = usePath();
  const { data, error, isPending, refetch } = useQuery(insightsQuery(pid));
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState("all");

  const list = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return (data ?? [])
      .filter((i) => kind === "all" || i.query?.kind === kind)
      .filter((i) => !needle || `${i.name} ${summarize(i.query)}`.toLowerCase().includes(needle));
  }, [data, search, kind]);

  const columns = useMemo(
    () => [
      col.accessor("name", {
        header: "Name",
        cell: ({ row }) => {
          const i = row.original;
          const k = i.query ? kindInfo(i.query.kind) : null;
          return (
            <div className="flex items-center gap-3">
              <IconBadge>
                <Icon name={k?.icon ?? "alert"} size={14} />
              </IconBadge>
              <div className="flex min-w-0 flex-col">
                <Link
                  to={path(`insights/${i.id}`)}
                  onClick={(e) => e.stopPropagation()}
                  className="max-w-md truncate font-semibold hover:text-brand-foreground hover:underline"
                >
                  {i.name}
                </Link>
                <span className="max-w-[560px] truncate text-xs text-muted-foreground">{summarize(i.query)}</span>
              </div>
            </div>
          );
        },
      }),
      col.accessor((i) => (i.query ? kindInfo(i.query.kind).label : "Legacy"), {
        id: "type",
        header: "Type",
        cell: (c) => <span className="text-muted-foreground">{c.getValue()}</span>,
      }),
      col.accessor("updated_at", {
        header: "Last modified",
        cell: (c) => <span className="whitespace-nowrap text-muted-foreground">{fmtRelative(c.getValue())}</span>,
        meta: { align: "right" },
      }),
    ],
    [path],
  );

  return (
    <Page>
      <PageHeader title="Insights" sub="Saved questions about your product, answered live from your events." actions={<NewInsightMenu />} />

      <div className="mb-5 grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
        {KINDS.map((k) => (
          <Link
            key={k.kind}
            to={path("insights/new")}
            search={{ kind: k.slug }}
            className="flex items-center gap-3 rounded-xl bg-card px-3.5 py-3 ring-1 ring-foreground/10 transition-colors outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
          >
            <IconBadge tone="brand" className="size-8">
              <Icon name={k.icon} size={15} />
            </IconBadge>
            <span className="flex min-w-0 flex-col">
              <span className="font-semibold">{k.label}</span>
              <span className="truncate text-xs text-muted-foreground">{k.blurb}</span>
            </span>
          </Link>
        ))}
      </div>

      <Panel>
        <CardBar>
          <SearchInput wrapperClassName="w-full sm:w-72" placeholder="Search insights…" aria-label="Search insights" value={search} onChange={(e) => setSearch(e.target.value)} />
          <Select value={kind} onValueChange={(v) => setKind(v ?? "all")} items={KIND_ITEMS}>
            <SelectTrigger aria-label="Insight type" className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KIND_ITEMS.map((k) => (
                <SelectItem key={k.value} value={k.value}>
                  {k.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="flex-1" />
          <span className="num text-xs text-muted-foreground">{data ? `${list.length} of ${data.length}` : ""}</span>
        </CardBar>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} />
        ) : isPending ? (
          <SkeletonRows rows={6} />
        ) : data && data.length === 0 ? (
          <Empty icon="trends" title="No saved insights yet" action={<NewInsightMenu />}>
            Build a trend, funnel or retention chart, then save it here to share and pin it on a dashboard.
          </Empty>
        ) : list.length === 0 ? (
          <Empty icon="search" title="Nothing matches" />
        ) : (
          <DataTable
            label="Saved insights"
            columns={columns}
            data={list}
            getRowId={(i) => i.id}
            onRowClick={(i) => navigate(path(`insights/${i.id}`))}
            sortable
            initialSorting={[{ id: "updated_at", desc: true }]}
            virtualize={{ maxHeight: 640, estimateRowHeight: 61 }}
          />
        )}
      </Panel>
    </Page>
  );
}
