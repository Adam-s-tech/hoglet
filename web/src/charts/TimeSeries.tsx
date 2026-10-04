// Line / area / grouped bar / stacked bar over an ordinal x axis.
// One y axis, recessive grid, crosshair + tooltip, click-through to persons.

import { useMemo, useState, type MouseEvent as RMouseEvent, type PointerEvent as RPointerEvent } from "react";
import { fmtCompact, fmtNumber } from "../lib/format";
import { useSize } from "../lib/hooks";
import { barPath, labelStride, linear, niceDomain, textWidth } from "./scale";

export interface ChartSeries {
  key: string;
  label: string;
  color: string;
  data: number[];
  /** Comparison period: dashed, recessive. */
  dashed?: boolean;
}

export type SeriesKind = "line" | "area" | "bar" | "stacked";

interface Props {
  labels: string[];
  series: ChartSeries[];
  kind: SeriesKind;
  height?: number;
  format?: (n: number) => string;
  tooltipTitle?: (i: number) => string;
  onPointClick?: (seriesIndex: number, index: number) => void;
  legend?: boolean;
  /** Hint shown at the foot of the tooltip. */
  clickHint?: string;
  axisFormat?: (n: number) => string;
}

const M = { top: 10, right: 12, bottom: 26 };

export function TimeSeriesChart({ labels, series, kind, height = 300, format = fmtNumber, tooltipTitle, onPointClick, legend = true, clickHint = "Click to see persons", axisFormat = fmtCompact }: Props) {
  const [ref, size] = useSize<HTMLDivElement>();
  const [hidden, setHidden] = useState<Set<string>>(() => new Set());
  const [hover, setHover] = useState<{ i: number; s: number; x: number; y: number } | null>(null);

  const visible = series.map((s, idx) => ({ s, idx })).filter(({ s }) => !hidden.has(s.key));
  const n = labels.length;
  const width = Math.max(0, size.width);

  const { lo, hi, ticks } = useMemo(() => {
    let min = 0;
    let max = 0;
    if (kind === "stacked") {
      for (let i = 0; i < n; i++) {
        let pos = 0;
        let neg = 0;
        for (const { s } of visible) {
          const v = s.data[i] ?? 0;
          if (v >= 0) pos += v;
          else neg += v;
        }
        max = Math.max(max, pos);
        min = Math.min(min, neg);
      }
    } else {
      for (const { s } of visible) for (const v of s.data) {
        if (v > max) max = v;
        if (v < min) min = v;
      }
    }
    return niceDomain(min, max, height < 200 ? 3 : 5);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, hidden, kind, n, height]);

  const left = Math.max(28, ...ticks.map((t) => textWidth(axisFormat(t)) + 12));
  const innerW = Math.max(10, width - left - M.right);
  const innerH = Math.max(10, height - M.top - M.bottom);
  const y = linear(lo, hi, M.top + innerH, M.top);
  const isBar = kind === "bar" || kind === "stacked";
  const band = n > 0 ? innerW / n : innerW;
  const xAt = (i: number) => (isBar ? left + band * i + band / 2 : n <= 1 ? left + innerW / 2 : left + (innerW * i) / (n - 1));
  const stride = labelStride(n, innerW, Math.max(56, ...labels.slice(0, 50).map((l) => textWidth(l) + 16)));

  const locate = (e: RPointerEvent<SVGRectElement> | RMouseEvent<SVGRectElement>) => {
    const rect = (e.currentTarget.ownerSVGElement as SVGSVGElement).getBoundingClientRect();
    const px = e.clientX - rect.left;
    const py = e.clientY - rect.top;
    let i: number;
    if (isBar) i = Math.floor((px - left) / band);
    else i = n <= 1 ? 0 : Math.round(((px - left) / innerW) * (n - 1));
    i = Math.max(0, Math.min(n - 1, i));
    let s = visible[0]?.idx ?? 0;
    if (kind === "bar" && visible.length > 1) {
      const groupW = band * 0.78;
      const slot = Math.floor((px - (left + band * i + (band - groupW) / 2)) / (groupW / visible.length));
      s = visible[Math.max(0, Math.min(visible.length - 1, slot))].idx;
    } else if (kind === "stacked") {
      let pos = 0;
      let neg = 0;
      for (const v of visible) {
        const val = v.s.data[i] ?? 0;
        const a = val >= 0 ? pos : neg;
        const b = a + val;
        if (val >= 0) pos = b;
        else neg = b;
        const top = Math.min(y(a), y(b));
        const bot = Math.max(y(a), y(b));
        if (py >= top && py <= bot) s = v.idx;
      }
    } else {
      let best = Infinity;
      for (const v of visible) {
        const d = Math.abs(y(v.s.data[i] ?? 0) - py);
        if (d < best) {
          best = d;
          s = v.idx;
        }
      }
    }
    return { i, s, x: px, y: py };
  };

  const zeroY = y(0);
  const groupW = band * (visible.length > 1 ? 0.78 : 0.62);
  const barW = kind === "bar" ? Math.max(1, groupW / Math.max(1, visible.length) - 2) : Math.max(1, Math.min(band * 0.62, 56));

  const stacks = useMemo(() => {
    if (kind !== "stacked") return null;
    const out: { idx: number; i: number; y0: number; y1: number }[] = [];
    for (let i = 0; i < n; i++) {
      let pos = 0;
      let neg = 0;
      for (const v of visible) {
        const val = v.s.data[i] ?? 0;
        if (val === 0) continue;
        const a = val >= 0 ? pos : neg;
        const b = a + val;
        if (val >= 0) pos = b;
        else neg = b;
        out.push({ idx: v.idx, i, y0: a, y1: b });
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, hidden, kind, n]);

  const tipRows = hover
    ? visible
        .map(({ s, idx }) => ({ s, idx, v: s.data[hover.i] ?? 0 }))
        .sort((a, b) => (kind === "stacked" ? 0 : b.v - a.v))
    : [];

  return (
    <div className="chart" ref={ref}>
      <svg width={width} height={height} role="img" aria-label={`Chart of ${series.map((s) => s.label).join(", ")}`}>
        {width > 0 && (
          <>
            {ticks.map((t) => (
              <g key={t}>
                <line className={t === 0 ? "baseline" : "gridline"} x1={left} x2={left + innerW} y1={Math.round(y(t)) + 0.5} y2={Math.round(y(t)) + 0.5} />
                <text x={left - 8} y={y(t)} dy="0.32em" textAnchor="end">
                  {axisFormat(t)}
                </text>
              </g>
            ))}
            {labels.map((l, i) =>
              i % stride === 0 ? (
                <text key={i} x={xAt(i)} y={height - 8} textAnchor={!isBar && i === 0 && n > 1 ? "start" : !isBar && i === n - 1 && n > 1 ? "end" : "middle"}>
                  {l}
                </text>
              ) : null,
            )}

            {hover && !isBar && <line className="crosshair" x1={xAt(hover.i)} x2={xAt(hover.i)} y1={M.top} y2={M.top + innerH} />}
            {hover && isBar && <rect x={left + band * hover.i} y={M.top} width={band} height={innerH} fill="var(--ink)" opacity={0.04} />}

            {kind === "bar" &&
              visible.map(({ s, idx }, k) =>
                s.data.map((v, i) => {
                  const x = left + band * i + (band - groupW) / 2 + k * (groupW / visible.length) + 1;
                  return <path key={`${idx}-${i}`} d={barPath(x, zeroY, barW, y(v))} fill={s.color} opacity={s.dashed ? 0.45 : hover && hover.i !== i ? 0.85 : 1} />;
                }),
              )}

            {stacks?.map((b) => {
              const s = series[b.idx];
              const x = xAt(b.i) - barW / 2;
              const top = Math.min(y(b.y0), y(b.y1));
              const h = Math.abs(y(b.y1) - y(b.y0));
              return (
                <rect
                  key={`${b.idx}-${b.i}`}
                  x={x}
                  y={top}
                  width={barW}
                  height={Math.max(0, h)}
                  rx={2}
                  fill={s.color}
                  stroke="var(--surface)"
                  strokeWidth={1}
                  opacity={hover && hover.i !== b.i ? 0.85 : 1}
                />
              );
            })}

            {(kind === "line" || kind === "area") &&
              visible.map(({ s, idx }) => {
                const pts = s.data.map((v, i) => `${xAt(i).toFixed(1)},${y(v).toFixed(1)}`);
                const line = `M${pts.join("L")}`;
                return (
                  <g key={idx}>
                    {kind === "area" && n > 1 && (
                      <path d={`${line}L${xAt(n - 1).toFixed(1)},${zeroY}L${xAt(0).toFixed(1)},${zeroY}Z`} fill={s.color} opacity={s.dashed ? 0.05 : visible.length > 1 ? 0.1 : 0.14} />
                    )}
                    <path
                      d={line}
                      fill="none"
                      stroke={s.color}
                      strokeWidth={2}
                      strokeLinejoin="round"
                      strokeLinecap="round"
                      strokeDasharray={s.dashed ? "4 4" : undefined}
                      opacity={s.dashed ? 0.6 : 1}
                    />
                    {(n === 1 || (hover && hover.s === idx)) &&
                      s.data.map((v, i) =>
                        n === 1 || hover?.i === i ? <circle key={i} cx={xAt(i)} cy={y(v)} r={4.5} fill={s.color} stroke="var(--surface)" strokeWidth={2} /> : null,
                      )}
                  </g>
                );
              })}

            <rect
              className="hit"
              x={left}
              y={M.top}
              width={innerW}
              height={innerH}
              onPointerMove={(e) => setHover(locate(e))}
              onPointerLeave={() => setHover(null)}
              onClick={(e) => {
                const h = locate(e);
                setHover(h);
                onPointClick?.(h.s, h.i);
              }}
              style={{ cursor: onPointClick ? "pointer" : "default" }}
            />
          </>
        )}
      </svg>

      {hover && (
        <div
          className="tip"
          style={{
            left: Math.min(Math.max(0, hover.x + 14), Math.max(0, width - 230)),
            top: Math.max(0, Math.min(hover.y - 20, height - 40 - tipRows.length * 20)),
          }}
        >
          <div className="tip-title">{tooltipTitle ? tooltipTitle(hover.i) : labels[hover.i]}</div>
          {tipRows.slice(0, 12).map(({ s, idx, v }) => (
            <div className="tip-row" key={idx} style={{ fontWeight: idx === hover.s ? 600 : undefined }}>
              <span className={`swatch${kind === "line" ? " line" : ""}`} style={{ background: s.color, opacity: s.dashed ? 0.5 : 1 }} />
              <span className="lab">{s.label}</span>
              <span className="val">{format(v)}</span>
            </div>
          ))}
          {tipRows.length > 12 && <div className="muted small">+{tipRows.length - 12} more</div>}
          {onPointClick && <div className="tip-foot">{clickHint}</div>}
        </div>
      )}

      {legend && series.length > 1 && (
        <div className="legend" role="group" aria-label="Series">
          {series.map((s) => (
            <button
              key={s.key}
              aria-pressed={!hidden.has(s.key)}
              onClick={() => {
                const next = new Set(hidden);
                if (next.has(s.key)) next.delete(s.key);
                else if (next.size < series.length - 1) next.add(s.key);
                setHidden(next);
              }}
              title={s.label}
            >
              <span className={`swatch${kind === "line" ? " line" : ""}`} style={{ background: s.color, opacity: s.dashed ? 0.5 : 1 }} />
              <span className="truncate">{s.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** Tiny inline trend line (KPI tiles, tables). */
export function Sparkline({ data, color = "var(--s1)", width = 96, height = 28 }: { data: number[]; color?: string; width?: number; height?: number }) {
  if (data.length < 2) return <svg width={width} height={height} aria-hidden="true" />;
  const max = Math.max(...data, 1);
  const min = Math.min(...data, 0);
  const x = linear(0, data.length - 1, 1, width - 1);
  const yy = linear(min, max, height - 2, 2);
  const d = `M${data.map((v, i) => `${x(i).toFixed(1)},${yy(v).toFixed(1)}`).join("L")}`;
  return (
    <svg width={width} height={height} aria-hidden="true">
      <path d={`${d}L${width - 1},${height}L1,${height}Z`} fill={color} opacity={0.1} />
      <path d={d} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
    </svg>
  );
}
