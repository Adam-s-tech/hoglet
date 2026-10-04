// Line / area / grouped bar / stacked bar over an ordinal x axis.
// One y axis, recessive grid, crosshair + tooltip, click-through to persons.

import { useMemo, useState, type KeyboardEvent as RKeyboardEvent, type MouseEvent as RMouseEvent, type PointerEvent as RPointerEvent } from "react";
import { fmtCompact, fmtNumber } from "@/lib/format";
import { useSize } from "@/lib/hooks";
import { cn } from "@/lib/utils";
import { DataTableToggle, Swatch, Tip, TipFoot, TipRow, TipTitle } from "./parts";
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
  /** Offer the plotted numbers as a table (the text alternative). Off in dashboard tiles. */
  dataTable?: boolean;
}

const M = { top: 10, right: 12, bottom: 26 };
const KIND_NAME: Record<SeriesKind, string> = { line: "Line chart", area: "Area chart", bar: "Bar chart", stacked: "Stacked bar chart" };

export function TimeSeriesChart({ labels, series, kind, height = 300, format = fmtNumber, tooltipTitle, onPointClick, legend = true, clickHint = "Click or press Enter to see persons", axisFormat = fmtCompact, dataTable = false }: Props) {
  const [ref, size] = useSize<HTMLDivElement>();
  const [hidden, setHidden] = useState<Set<string>>(() => new Set());
  const [hover, setHover] = useState<{ i: number; s: number; x: number; y: number } | null>(null);
  const [kbd, setKbd] = useState(false);

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

  // Keyboard: arrows move between points (left/right) and series (up/down),
  // Enter/Space opens the persons behind the point, Escape clears.
  const onKey = (e: RKeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    if (n === 0 || visible.length === 0) return;
    const move = (i: number, s: number) => {
      const ni = Math.max(0, Math.min(n - 1, i));
      const ser = series[s];
      const cx = xAt(ni);
      const cy = y(ser?.data[ni] ?? 0);
      setKbd(true);
      setHover({ i: ni, s, x: cx, y: cy });
    };
    const cur = hover ?? { i: n - 1, s: visible[0].idx, x: 0, y: 0 };
    const vpos = Math.max(0, visible.findIndex((v) => v.idx === cur.s));
    switch (e.key) {
      case "ArrowLeft":
        e.preventDefault();
        move(hover ? cur.i - 1 : n - 1, cur.s);
        break;
      case "ArrowRight":
        e.preventDefault();
        move(hover ? cur.i + 1 : n - 1, cur.s);
        break;
      case "Home":
        e.preventDefault();
        move(0, cur.s);
        break;
      case "End":
        e.preventDefault();
        move(n - 1, cur.s);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(cur.i, visible[(vpos - 1 + visible.length) % visible.length].idx);
        break;
      case "ArrowDown":
        e.preventDefault();
        move(cur.i, visible[(vpos + 1) % visible.length].idx);
        break;
      case "Enter":
      case " ":
        if (hover && onPointClick) {
          e.preventDefault();
          onPointClick(hover.s, hover.i);
        }
        break;
      case "Escape":
        if (hover) {
          setHover(null);
          setKbd(false);
        }
        break;
    }
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

  const summary = `${KIND_NAME[kind]} of ${n} ${n === 1 ? "period" : "periods"}${labels.length ? `, ${labels[0]} to ${labels[n - 1]}` : ""}. ${series
    .slice(0, 6)
    .map((s) => `${s.label}: ${format(s.data[0] ?? 0)} to ${format(s.data[s.data.length - 1] ?? 0)}`)
    .join("; ")}${series.length > 6 ? `; and ${series.length - 6} more` : ""}.`;

  return (
    <div
      className="chart rounded-md focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
      ref={ref}
      tabIndex={0}
      role="group"
      aria-roledescription="interactive chart"
      aria-label={`${summary} Use arrow keys to inspect points${onPointClick ? ", Enter to see persons" : ""}.`}
      onKeyDown={onKey}
      onBlur={() => {
        if (kbd) {
          setKbd(false);
          setHover(null);
        }
      }}
    >
      <svg width={width} height={height} role="img" aria-label={summary}>
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
            {hover && isBar && <rect x={left + band * hover.i} y={M.top} width={band} height={innerH} fill="var(--foreground)" opacity={0.05} />}

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
                  stroke="var(--card)"
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
                        n === 1 || hover?.i === i ? <circle key={i} cx={xAt(i)} cy={y(v)} r={4.5} fill={s.color} stroke="var(--card)" strokeWidth={2} /> : null,
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
              onPointerMove={(e) => {
                setKbd(false);
                setHover(locate(e));
              }}
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
        <Tip
          style={{
            left: Math.min(Math.max(0, hover.x + 14), Math.max(0, width - 230)),
            top: Math.max(0, Math.min(hover.y - 20, height - 40 - tipRows.length * 20)),
          }}
        >
          <TipTitle>{tooltipTitle ? tooltipTitle(hover.i) : labels[hover.i]}</TipTitle>
          {tipRows.slice(0, 12).map(({ s, idx, v }) => (
            <TipRow key={idx} strong={idx === hover.s} swatch={<Swatch color={s.color} line={kind === "line"} faded={s.dashed} />} label={s.label} value={format(v)} />
          ))}
          {tipRows.length > 12 && <div className="text-muted-foreground">+{tipRows.length - 12} more</div>}
          {onPointClick && <TipFoot>{clickHint}</TipFoot>}
        </Tip>
      )}

      {kbd && hover && (
        <div className="sr-only" aria-live="polite" role="status">
          {`${tooltipTitle ? tooltipTitle(hover.i) : labels[hover.i]}: ${series[hover.s]?.label} ${format(series[hover.s]?.data[hover.i] ?? 0)}`}
        </div>
      )}

      {legend && series.length > 1 && (
        <div className="mt-2.5 flex flex-wrap gap-x-3.5 gap-y-1 text-xs text-muted-foreground" role="group" aria-label="Series">
          {series.map((s) => (
            <button
              key={s.key}
              type="button"
              aria-pressed={!hidden.has(s.key)}
              onClick={() => {
                const next = new Set(hidden);
                if (next.has(s.key)) next.delete(s.key);
                else if (next.size < series.length - 1) next.add(s.key);
                setHidden(next);
              }}
              title={s.label}
              className={cn(
                "inline-flex max-w-70 items-center gap-1.5 rounded px-1 py-0.5 text-xs hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none",
                hidden.has(s.key) && "opacity-40",
              )}
            >
              <Swatch color={s.color} line={kind === "line"} faded={s.dashed} />
              <span className="truncate">{s.label}</span>
            </button>
          ))}
        </div>
      )}
      {dataTable && (
        <DataTableToggle
          caption={`${KIND_NAME[kind]} data`}
          head={["Period", ...series.map((s) => s.label)]}
          rows={labels.map((l, i) => [tooltipTitle ? tooltipTitle(i) : l, ...series.map((s) => format(s.data[i] ?? 0))])}
        />
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
