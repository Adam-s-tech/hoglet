import { useState } from "react";
import { fmtNumber, fmtPercent } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Swatch } from "./parts";

export interface Slice {
  key: string;
  label: string;
  value: number;
  color: string;
}

/**
 * Donut with a 2px card-coloured gap between slices and a legend with values.
 * The legend rows are real buttons: they are the keyboard path to each slice.
 */
export function PieChart({ slices, onSliceClick, size = 220 }: { slices: Slice[]; onSliceClick?: (index: number) => void; size?: number }) {
  const [hover, setHover] = useState<number | null>(null);
  const total = slices.reduce((a, s) => a + Math.max(0, s.value), 0);
  const r = size / 2 - 4;
  const inner = r * 0.62;
  const c = size / 2;
  let angle = -Math.PI / 2;
  const arcs = slices.map((s, i) => {
    const frac = total > 0 ? Math.max(0, s.value) / total : 0;
    const a0 = angle;
    const a1 = angle + frac * Math.PI * 2;
    angle = a1;
    const large = a1 - a0 > Math.PI ? 1 : 0;
    const p = (rad: number, a: number) => `${(c + rad * Math.cos(a)).toFixed(2)},${(c + rad * Math.sin(a)).toFixed(2)}`;
    const d =
      frac >= 0.9999
        ? `M${p(r, 0)}A${r},${r} 0 1 1 ${p(r, Math.PI)}A${r},${r} 0 1 1 ${p(r, 0)}M${p(inner, 0)}A${inner},${inner} 0 1 0 ${p(inner, Math.PI)}A${inner},${inner} 0 1 0 ${p(inner, 0)}Z`
        : `M${p(r, a0)}A${r},${r} 0 ${large} 1 ${p(r, a1)}L${p(inner, a1)}A${inner},${inner} 0 ${large} 0 ${p(inner, a0)}Z`;
    return { d, i, frac };
  });
  const focus = hover !== null ? slices[hover] : null;
  const summary = `Donut chart, ${fmtNumber(total)} total: ${slices
    .slice(0, 6)
    .map((s) => `${s.label} ${fmtPercent(total ? (s.value / total) * 100 : 0)}`)
    .join(", ")}${slices.length > 6 ? `, and ${slices.length - 6} more` : ""}.`;
  return (
    <div className="flex flex-wrap items-center justify-center gap-6">
      <div className="chart" style={{ width: size }}>
        <svg width={size} height={size} role="img" aria-label={summary}>
          {arcs.map(({ d, i, frac }) =>
            frac > 0 ? (
              <path
                key={i}
                d={d}
                fill={slices[i].color}
                stroke="var(--card)"
                strokeWidth={2}
                fillRule="evenodd"
                opacity={hover === null || hover === i ? 1 : 0.45}
                className="transition-opacity duration-100 motion-reduce:transition-none"
                style={{ cursor: onSliceClick ? "pointer" : "default" }}
                onPointerEnter={() => setHover(i)}
                onPointerLeave={() => setHover(null)}
                onClick={() => onSliceClick?.(i)}
              />
            ) : null,
          )}
          <text x={c} y={c - 6} textAnchor="middle" style={{ fontSize: 20, fontWeight: 650, fill: "var(--foreground)" }}>
            {fmtNumber(focus ? focus.value : total)}
          </text>
          <text x={c} y={c + 14} textAnchor="middle">
            {focus ? fmtPercent(total ? (focus.value / total) * 100 : 0) : "total"}
          </text>
        </svg>
      </div>
      <div className="flex max-w-90 min-w-50 flex-col gap-1">
        {slices.map((s, i) => (
          <button
            key={s.key}
            type="button"
            onPointerEnter={() => setHover(i)}
            onPointerLeave={() => setHover(null)}
            onFocus={() => setHover(i)}
            onBlur={() => setHover(null)}
            onClick={() => onSliceClick?.(i)}
            title={onSliceClick ? `${s.label}: click to see persons` : s.label}
            className={cn(
              "flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none",
              onSliceClick ? "hover:bg-muted" : "cursor-default",
              hover === i && "bg-muted",
            )}
          >
            <Swatch color={s.color} />
            <span className="min-w-0 flex-1 truncate">{s.label}</span>
            <span className="num text-xs text-muted-foreground">
              {fmtNumber(s.value)} · {fmtPercent(total ? (s.value / total) * 100 : 0)}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

/** Horizontal value bars (ActionsBarValue): one row per series. */
export function HBarList({ rows, format = fmtNumber, onClick }: { rows: Slice[]; format?: (n: number) => string; onClick?: (index: number) => void }) {
  const max = Math.max(1, ...rows.map((r) => Math.abs(r.value)));
  return (
    <ul className="flex flex-col gap-2.5" aria-label="Values by series">
      {rows.map((r, i) => (
        <li key={r.key}>
          <button
            type="button"
            onClick={() => onClick?.(i)}
            title={onClick ? `${r.label}: click to see persons` : r.label}
            className={cn(
              "block w-full rounded-md p-1 text-left focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none",
              onClick ? "hover:bg-muted/60" : "cursor-default",
            )}
          >
            <div className="mb-1 flex items-center gap-2 text-[13px]">
              <Swatch color={r.color} />
              <span className="min-w-0 flex-1 truncate">{r.label}</span>
              <b className="num">{format(r.value)}</b>
            </div>
            <div className="h-3.5 rounded bg-muted">
              <div className="h-full rounded-r" style={{ width: `${(Math.abs(r.value) / max) * 100}%`, background: r.color }} />
            </div>
          </button>
        </li>
      ))}
    </ul>
  );
}
