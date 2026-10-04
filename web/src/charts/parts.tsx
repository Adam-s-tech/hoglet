// Small pieces every chart shares: tooltip shell, swatch, delta.

import { useId, useState, type CSSProperties, type ReactNode } from "react";
import { fmtPercent } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Colour key for a series. `line` draws a thin stroke instead of a square. */
export function Swatch({ color, line, faded, className }: { color: string; line?: boolean; faded?: boolean; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("inline-block flex-none rounded-[3px]", line ? "h-[3px] w-2.5 rounded-[2px]" : "size-2.5", className)}
      style={{ background: color, opacity: faded ? 0.5 : 1 }}
    />
  );
}

/** Floating tooltip, positioned absolutely inside a `relative` chart wrapper. */
export function Tip({ style, children, className }: { style: CSSProperties; children: ReactNode; className?: string }) {
  return (
    <div
      role="presentation"
      className={cn(
        "pointer-events-none absolute z-20 max-w-80 min-w-40 rounded-lg bg-popover px-2.5 py-2 text-xs text-popover-foreground shadow-lg ring-1 ring-foreground/10",
        className,
      )}
      style={style}
    >
      {children}
    </div>
  );
}

export function TipTitle({ children }: { children: ReactNode }) {
  return <div className="mb-1.5 font-semibold text-muted-foreground">{children}</div>;
}

export function TipRow({ swatch, label, value, strong }: { swatch?: ReactNode; label: ReactNode; value: ReactNode; strong?: boolean }) {
  return (
    <div className={cn("flex items-center gap-2 py-px", strong && "font-semibold")}>
      {swatch}
      <span className="min-w-0 flex-1 truncate text-muted-foreground">{label}</span>
      <span className="num font-semibold">{value}</span>
    </div>
  );
}

export function TipFoot({ children }: { children: ReactNode }) {
  return <div className="mt-1.5 border-t pt-1.5 text-[11px] text-muted-foreground">{children}</div>;
}

/** Rows past this are cut from a chart's data table (the table is a text alternative, not an export). */
export const DATA_TABLE_ROWS = 400;

/**
 * Text alternative for a chart: a toggle that reveals the plotted numbers as a
 * real, scrollable table. `rows` are pre-formatted cells; the first column is
 * the row header.
 */
export function DataTableToggle({ caption, head, rows, className }: { caption: string; head: string[]; rows: string[][]; className?: string }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const shown = rows.slice(0, DATA_TABLE_ROWS);
  return (
    <div className={cn("mt-2", className)}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((v) => !v)}
        className="rounded px-1 py-0.5 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
      >
        {open ? "Hide data table" : "Show data table"}
      </button>
      <div id={id}>
        {open && (
          <div className="mt-1.5 max-h-72 overflow-auto rounded-lg ring-1 ring-foreground/10" tabIndex={0} role="region" aria-label={`${caption} (scrollable)`}>
            <table className="w-full border-collapse text-xs">
              <caption className="sr-only">{caption}</caption>
              <thead className="sticky top-0 bg-card">
                <tr>
                  {head.map((h, i) => (
                    <th key={i} scope="col" className={cn("border-b px-2.5 py-1.5 font-semibold whitespace-nowrap text-muted-foreground", i === 0 ? "text-left" : "text-right")}>
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {shown.map((r, ri) => (
                  <tr key={ri} className="border-b last:border-0">
                    {r.map((c, ci) =>
                      ci === 0 ? (
                        <th key={ci} scope="row" className="px-2.5 py-1 text-left font-normal whitespace-nowrap">
                          {c}
                        </th>
                      ) : (
                        <td key={ci} className="num px-2.5 py-1 text-right">
                          {c}
                        </td>
                      ),
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
            {rows.length > shown.length && (
              <p className="px-2.5 py-1.5 text-muted-foreground">
                Showing the first {DATA_TABLE_ROWS} of {rows.length} rows.
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

/** Percent change with an arrow; up is good. */
export function Delta({ value, suffix, className }: { value: number; suffix?: ReactNode; className?: string }) {
  const tone = value > 0 ? "text-good" : value < 0 ? "text-destructive" : "text-muted-foreground";
  return (
    <span className={cn("num inline-flex items-center gap-0.5 text-xs font-semibold", tone, className)}>
      <span aria-hidden="true">{value > 0 ? "▲" : value < 0 ? "▼" : ""}</span>
      <span className="sr-only">{value > 0 ? "up " : value < 0 ? "down " : "flat "}</span>
      {fmtPercent(Math.abs(value))}
      {suffix ? <span className="ml-1 font-normal text-muted-foreground">{suffix}</span> : null}
    </span>
  );
}
