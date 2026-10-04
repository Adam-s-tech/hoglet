// Small pieces every chart shares: tooltip shell, swatch, delta.

import type { CSSProperties, ReactNode } from "react";
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
