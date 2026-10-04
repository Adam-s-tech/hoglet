// The one table: shadcn's data-table pattern on TanStack Table (v9, only the
// features Hoglet uses are registered, so the rest tree-shakes away) with
// TanStack Virtual for long lists.
//
//   const columns = [col.accessor("name", { header: "Name", cell: (c) => ... })] // col = columnHelper<Row>()
//   <DataTable columns={columns} data={rows} getRowId={(r) => r.id} onRowClick={...} />
//
// - `sortable`: header click cycles asc / desc / off (client-side, loaded rows).
// - `columnMenu`: a "Columns" dropdown to hide/show columns.
// - `virtualize`: renders only the visible rows inside a scroll box; use it
//   for any list that can reach hundreds of rows (activity, SQL results).
// - `renderExpanded` + `expandedId`: one detail row under the active row.
// Rows are keyboard reachable when `onRowClick` is set (Enter / Space).

import {
  columnVisibilityFeature,
  createColumnHelper,
  createSortedRowModel,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  sortFn_text,
  tableFeatures,
  useTable,
  type ColumnDef,
  type SortingState,
  type ColumnVisibilityState as VisibilityState,
  type RowData,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Fragment, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { cn } from "@/lib/utils";
import { Icon } from "./icons";

const features = tableFeatures({
  rowSortingFeature,
  columnVisibilityFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, text: sortFn_text, basic: sortFn_basic },
});

/**
 * Column definitions are invariant in their cell value type, so a list of
 * columns with mixed value types only type-checks with `any` there. This is
 * TanStack Table's own constraint (`columnHelper.columns` uses it too) and the
 * only `any` in the app, confined to this boundary.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type Col<T extends RowData> = ColumnDef<typeof features, T, any>;
export const columnHelper = <T extends RowData>() => createColumnHelper<typeof features, T>();
export type { SortingState, VisibilityState };

/** Column `meta` Hoglet understands: alignment and a cell/header class. */
export interface ColMeta {
  align?: "right";
  className?: string;
}

export interface DataTableProps<T extends RowData> {
  columns: Col<T>[];
  data: T[];
  getRowId?: (row: T, index: number) => string;
  onRowClick?: (row: T) => void;
  sortable?: boolean;
  initialSorting?: SortingState;
  initialVisibility?: VisibilityState;
  columnMenu?: boolean;
  /** Virtualize rows inside a scroll box this tall (px). Rows may vary in height. */
  virtualize?: { maxHeight: number; estimateRowHeight?: number };
  expandedId?: string | null;
  renderExpanded?: (row: T) => ReactNode;
  rowClassName?: (row: T) => string | undefined;
  /** Dense rows (32px) for long technical lists. */
  dense?: boolean;
  className?: string;
  /** Accessible name of the table. */
  label: string;
}

const EMPTY_ROWS: never[] = [];

export function DataTable<T extends RowData>({
  columns,
  data,
  getRowId,
  onRowClick,
  sortable,
  initialSorting,
  initialVisibility,
  columnMenu,
  virtualize,
  expandedId,
  renderExpanded,
  rowClassName,
  dense,
  className,
  label,
}: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(initialSorting ?? []);
  const [visibility, setVisibility] = useState<VisibilityState>(initialVisibility ?? {});
  const table = useTable(
    {
      features,
      columns,
      data: data ?? EMPTY_ROWS,
      getRowId,
      state: { sorting, columnVisibility: visibility },
      onSortingChange: setSorting,
      onColumnVisibilityChange: setVisibility,
      enableSorting: !!sortable,
      enableSortingRemoval: true,
    },
    (state) => ({ sorting: state.sorting, columnVisibility: state.columnVisibility }),
  );

  const rows = table.getRowModel().rows;
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: virtualize ? rows.length : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => virtualize?.estimateRowHeight ?? (dense ? 33 : 41),
    getItemKey: (i) => rows[i]?.id ?? i,
    overscan: 8,
  });
  const items = virtualize ? virtualizer.getVirtualItems() : [];
  const padTop = items.length ? items[0].start : 0;
  const padBottom = items.length ? virtualizer.getTotalSize() - items[items.length - 1].end : 0;
  const colCount = table.getVisibleLeafColumns().length;

  const cellPad = dense ? "px-3 py-1.5" : "px-4 py-2.5";

  const renderRow = (index: number) => {
    const row = rows[index];
    const original = row.original;
    const expanded = renderExpanded && expandedId === row.id;
    const clickable = !!onRowClick;
    return (
      <Fragment key={row.id}>
        <TableRow
          tabIndex={clickable ? 0 : undefined}
          onClick={clickable ? () => onRowClick(original) : undefined}
          onKeyDown={
            clickable
              ? (e) => {
                  if (e.target !== e.currentTarget) return;
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onRowClick(original);
                  }
                }
              : undefined
          }
          className={cn(clickable && "cursor-pointer focus-visible:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring", rowClassName?.(original))}
        >
          {row.getVisibleCells().map((cell) => {
            const meta = cell.column.columnDef.meta as ColMeta | undefined;
            return (
              <TableCell key={cell.id} className={cn(cellPad, meta?.align === "right" && "text-right", meta?.className)}>
                <table.FlexRender cell={cell} />
              </TableCell>
            );
          })}
        </TableRow>
        {expanded && (
          <TableRow className="hover:bg-transparent">
            <TableCell colSpan={colCount} className="bg-muted/40 p-0">
              {renderExpanded(original)}
            </TableCell>
          </TableRow>
        )}
      </Fragment>
    );
  };

  return (
    <div className={className}>
      {columnMenu && (
        <div className="flex justify-end border-b px-3 py-1.5">
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="ghost" size="sm" />}>
              <Icon name="table" size={14} /> Columns
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-44">
              <DropdownMenuGroup>
                <DropdownMenuLabel>Show columns</DropdownMenuLabel>
                {table
                  .getAllLeafColumns()
                  .filter((c) => c.getCanHide())
                  .map((c) => (
                    <label key={c.id} className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent">
                      <Checkbox checked={c.getIsVisible()} onCheckedChange={(v) => c.toggleVisibility(v === true)} />
                      {typeof c.columnDef.header === "string" ? c.columnDef.header : c.id}
                    </label>
                  ))}
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      )}
      <div ref={scrollRef} className={cn(virtualize && "overflow-y-auto")} style={virtualize ? { maxHeight: virtualize.maxHeight } : undefined}>
        <Table aria-label={label}>
          <TableHeader className={cn(virtualize && "sticky top-0 z-10 bg-card")}>
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id} className="hover:bg-transparent">
                {group.headers.map((header) => {
                  const meta = header.column.columnDef.meta as ColMeta | undefined;
                  const sorted = header.column.getIsSorted();
                  const canSort = sortable && header.column.getCanSort();
                  return (
                    <TableHead
                      key={header.id}
                      aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
                      className={cn(
                        "h-9 px-4 text-xs font-semibold tracking-wide text-muted-foreground uppercase",
                        dense && "px-3",
                        meta?.align === "right" && "text-right",
                        meta?.className,
                      )}
                    >
                      {header.isPlaceholder ? null : canSort ? (
                        <button
                          type="button"
                          onClick={header.column.getToggleSortingHandler()}
                          className={cn(
                            "-mx-1 inline-flex items-center gap-1 rounded px-1 uppercase hover:text-foreground focus-visible:outline-2",
                            sorted && "text-foreground",
                          )}
                        >
                          <table.FlexRender header={header} />
                          <Icon name={sorted === "asc" ? "arrowUp" : sorted === "desc" ? "arrowDown" : "chevronUpDown"} size={12} className={sorted ? "" : "opacity-40"} />
                        </button>
                      ) : (
                        <table.FlexRender header={header} />
                      )}
                    </TableHead>
                  );
                })}
              </TableRow>
            ))}
          </TableHeader>
          {virtualize ? (
            <>
              {padTop > 0 && (
                <tbody aria-hidden="true">
                  <tr>
                    <td colSpan={colCount} style={{ height: padTop, padding: 0 }} />
                  </tr>
                </tbody>
              )}
              {/* One <tbody> per row so a row and its expanded detail measure together. */}
              {items.map((item) => (
                <TableBody key={item.key} data-index={item.index} ref={virtualizer.measureElement}>
                  {renderRow(item.index)}
                </TableBody>
              ))}
              {padBottom > 0 && (
                <tbody aria-hidden="true">
                  <tr>
                    <td colSpan={colCount} style={{ height: padBottom, padding: 0 }} />
                  </tr>
                </tbody>
              )}
            </>
          ) : (
            <TableBody>{rows.map((_, i) => renderRow(i))}</TableBody>
          )}
        </Table>
      </div>
    </div>
  );
}
