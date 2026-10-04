import { useState } from "react";
import { fmtNumber, fmtPercent } from "../lib/format";

export interface Slice {
  key: string;
  label: string;
  value: number;
  color: string;
}

/** Donut with a 2px surface gap between slices and a legend with values. */
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
  return (
    <div className="row gap-24 wrap" style={{ alignItems: "center", justifyContent: "center" }}>
      <div className="chart" style={{ width: size }}>
        <svg width={size} height={size} role="img" aria-label="Pie chart">
          {arcs.map(({ d, i, frac }) =>
            frac > 0 ? (
              <path
                key={i}
                d={d}
                fill={slices[i].color}
                stroke="var(--surface)"
                strokeWidth={2}
                fillRule="evenodd"
                opacity={hover === null || hover === i ? 1 : 0.45}
                style={{ cursor: onSliceClick ? "pointer" : "default", transition: "opacity .12s" }}
                onPointerEnter={() => setHover(i)}
                onPointerLeave={() => setHover(null)}
                onClick={() => onSliceClick?.(i)}
              />
            ) : null,
          )}
          <text x={c} y={c - 6} textAnchor="middle" style={{ fontSize: 20, fontWeight: 650, fill: "var(--ink)" }}>
            {fmtNumber(focus ? focus.value : total)}
          </text>
          <text x={c} y={c + 14} textAnchor="middle">
            {focus ? fmtPercent(total ? (focus.value / total) * 100 : 0) : "total"}
          </text>
        </svg>
      </div>
      <div className="col" style={{ gap: 4, minWidth: 200, maxWidth: 360 }}>
        {slices.map((s, i) => (
          <button
            key={s.key}
            className="menu-item"
            data-active={hover === i}
            onPointerEnter={() => setHover(i)}
            onPointerLeave={() => setHover(null)}
            onClick={() => onSliceClick?.(i)}
          >
            <span className="swatch" style={{ background: s.color }} />
            <span className="truncate">{s.label}</span>
            <span className="meta">
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
    <div className="col" style={{ gap: 10 }}>
      {rows.map((r, i) => (
        <button
          key={r.key}
          onClick={() => onClick?.(i)}
          style={{ border: 0, background: "none", padding: 0, font: "inherit", color: "inherit", textAlign: "left", cursor: onClick ? "pointer" : "default" }}
        >
          <div className="row" style={{ marginBottom: 4, fontSize: 13 }}>
            <span className="swatch" style={{ background: r.color }} />
            <span className="truncate grow">{r.label}</span>
            <b className="num">{format(r.value)}</b>
          </div>
          <div style={{ height: 14, borderRadius: 4, background: "var(--surface-2)" }}>
            <div style={{ width: `${(Math.abs(r.value) / max) * 100}%`, height: "100%", borderRadius: "0 4px 4px 0", background: r.color }} />
          </div>
        </button>
      ))}
    </div>
  );
}
