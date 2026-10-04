// Paths as a Sankey: step columns, nodes sized by persons, bands by flow.

import { useMemo, useState } from "react";
import type { PathLink } from "@/types/PathLink";
import { fmtDuration, fmtNumber } from "@/lib/format";
import { useSize } from "@/lib/hooks";
import { Tip, TipRow, TipTitle } from "./parts";

/** Flows listed for screen readers (the SVG itself is a picture); bounded. */
const SR_FLOWS = 50;

interface Node {
  id: string;
  name: string;
  step: number;
  inV: number;
  outV: number;
  value: number;
  x: number;
  y: number;
  h: number;
}
interface Band {
  link: PathLink;
  s: Node;
  t: Node;
  sy: number;
  ty: number;
  w: number;
}

function parse(id: string): { step: number; name: string } {
  const m = /^(\d+)_(.*)$/.exec(id);
  return m ? { step: Number(m[1]), name: m[2] } : { step: 1, name: id };
}

function short(s: string, n = 28): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

export function PathsSankey({ links, onNodeClick }: { links: PathLink[]; onNodeClick?: (name: string) => void }) {
  const [ref, size] = useSize<HTMLDivElement>();
  const [hot, setHot] = useState<{ kind: "node" | "link"; id: string } | null>(null);
  const [tip, setTip] = useState<{ x: number; y: number; band: Band } | null>(null);

  const layout = useMemo(() => {
    const nodes = new Map<string, Node>();
    const get = (id: string) => {
      let n = nodes.get(id);
      if (!n) {
        const p = parse(id);
        n = { id, name: p.name, step: p.step, inV: 0, outV: 0, value: 0, x: 0, y: 0, h: 0 };
        nodes.set(id, n);
      }
      return n;
    };
    for (const l of links) {
      get(l.source).outV += l.value;
      get(l.target).inV += l.value;
    }
    for (const n of nodes.values()) n.value = Math.max(n.inV, n.outV);
    const steps = [...new Set([...nodes.values()].map((n) => n.step))].sort((a, b) => a - b);
    const columns = steps.map((s) => [...nodes.values()].filter((n) => n.step === s).sort((a, b) => b.value - a.value));
    const maxCount = Math.max(1, ...columns.map((c) => c.length));
    return { nodes, columns, steps, maxCount };
  }, [links]);

  const width = size.width;
  const height = Math.max(340, Math.min(900, layout.maxCount * 44));
  const nodeW = 10;
  const pad = 14;
  const labelRoom = 140;
  const innerW = Math.max(100, width - labelRoom - 8);
  const cols = layout.columns.length;

  const bands: Band[] = useMemo(() => {
    if (width === 0) return [];
    const k = Math.min(
      ...layout.columns.map((c) => {
        const total = c.reduce((a, n) => a + n.value, 0);
        return (height - 24 - pad * (c.length - 1)) / Math.max(1, total);
      }),
    );
    layout.columns.forEach((col, ci) => {
      let y = 12;
      for (const n of col) {
        n.x = cols <= 1 ? 0 : (ci * (innerW - nodeW)) / (cols - 1);
        n.h = Math.max(3, n.value * k);
        n.y = y;
        y += n.h + pad;
      }
    });
    const outOff = new Map<string, number>();
    const inOff = new Map<string, number>();
    const sorted = [...links].sort((a, b) => {
      const sa = layout.nodes.get(a.source)!;
      const sb = layout.nodes.get(b.source)!;
      const ta = layout.nodes.get(a.target)!;
      const tb = layout.nodes.get(b.target)!;
      return sa.y - sb.y || ta.y - tb.y;
    });
    const out: Band[] = [];
    for (const link of sorted) {
      const s = layout.nodes.get(link.source)!;
      const t = layout.nodes.get(link.target)!;
      const w = Math.max(1, link.value * k);
      const so = outOff.get(s.id) ?? 0;
      out.push({ link, s, t, sy: s.y + so + w / 2, ty: 0, w });
      outOff.set(s.id, so + w);
    }
    // Incoming offsets ordered by source position so bands don't cross needlessly.
    for (const b of [...out].sort((a, c) => a.s.y - c.s.y)) {
      const to = inOff.get(b.t.id) ?? 0;
      b.ty = b.t.y + to + b.w / 2;
      inOff.set(b.t.id, to + b.w);
    }
    return out;
  }, [layout, links, width, height, innerW, cols]);

  // One label per node unless it would collide with the label above it.
  const labelled = new Set<string>();
  for (const col of layout.columns) {
    let lastY = -Infinity;
    for (const n of [...col].sort((a, b) => a.y - b.y)) {
      const y = n.y + Math.min(n.h / 2, 10);
      if (y - lastY >= 13) {
        labelled.add(n.id);
        lastY = y + (n.h > 24 ? 14 : 0);
      }
    }
  }
  const colGap = cols <= 1 ? innerW : (innerW - nodeW) / (cols - 1);
  const labelChars = Math.max(8, Math.floor((colGap - nodeW - 12) / 6.4));

  const isHot = (b: Band) =>
    hot === null ? null : hot.kind === "link" ? `${b.link.source}>${b.link.target}` === hot.id : b.s.id === hot.id || b.t.id === hot.id;

  return (
    <div className="chart" ref={ref}>
      <svg width={width} height={height} role="img" aria-label={`User paths across ${cols} steps, ${layout.nodes.size} nodes and ${links.length} flows. A list of the top flows follows.`}>
        {width > 0 && (
          <g transform="translate(4,0)">
            {layout.steps.map((s, ci) => (
              <text key={s} x={cols <= 1 ? 0 : (ci * (innerW - nodeW)) / (cols - 1)} y={6} style={{ fontSize: 10.5, fontWeight: 600, letterSpacing: "0.04em" }}>
                STEP {s}
              </text>
            ))}
            {bands.map((b) => {
              const x0 = b.s.x + nodeW;
              const x1 = b.t.x;
              const mx = (x0 + x1) / 2;
              const h = isHot(b);
              return (
                <path
                  key={`${b.link.source}>${b.link.target}`}
                  className={`sankey-link${h === true ? " hot" : h === false ? " dim" : ""}`}
                  d={`M${x0},${b.sy}C${mx},${b.sy} ${mx},${b.ty} ${x1},${b.ty}`}
                  stroke="var(--s1)"
                  strokeWidth={b.w}
                  onPointerEnter={() => setHot({ kind: "link", id: `${b.link.source}>${b.link.target}` })}
                  onPointerMove={(e) => {
                    const r = (e.currentTarget.ownerSVGElement as SVGSVGElement).getBoundingClientRect();
                    setTip({ x: e.clientX - r.left, y: e.clientY - r.top, band: b });
                  }}
                  onPointerLeave={() => {
                    setHot(null);
                    setTip(null);
                  }}
                />
              );
            })}
            {[...layout.nodes.values()].map((n) => {
              const lx = n.x + nodeW + 6;
              const ly = n.y + Math.min(n.h / 2, 10);
              const showLabel = labelled.has(n.id);
              return (
                <g
                  key={n.id}
                  className="sankey-node"
                  onPointerEnter={() => setHot({ kind: "node", id: n.id })}
                  onPointerLeave={() => setHot(null)}
                  onClick={() => onNodeClick?.(n.name)}
                  style={{ cursor: onNodeClick ? "pointer" : "default" }}
                >
                  <rect x={n.x} y={n.y} width={nodeW} height={n.h} rx={2} fill="var(--s1)" />
                  <title>{`${n.name}: ${fmtNumber(n.value)} persons`}</title>
                  {showLabel && (
                    <>
                      <text className="node-label" x={lx} y={ly} dy="0.32em">
                        {short(n.name, labelChars)}
                      </text>
                      {n.h > 24 && (
                        <text className="node-count" x={lx} y={ly + 14} dy="0.32em">
                          {fmtNumber(n.value)}
                        </text>
                      )}
                    </>
                  )}
                </g>
              );
            })}
          </g>
        )}
      </svg>
      {tip && (
        <Tip style={{ left: Math.min(tip.x + 14, Math.max(0, width - 260)), top: tip.y + 10 }}>
          <TipTitle>
            {short(tip.band.s.name, 40)} → {short(tip.band.t.name, 40)}
          </TipTitle>
          <TipRow label="Persons" value={fmtNumber(tip.band.link.value)} />
          <TipRow label={`Share of step ${tip.band.s.step}`} value={`${((tip.band.link.value / Math.max(1, tip.band.s.value)) * 100).toFixed(1)}%`} />
          <TipRow label="Avg time between" value={fmtDuration(tip.band.link.average_conversion_time_s)} />
        </Tip>
      )}
      <ul className="sr-only" aria-label="Top flows between steps">
        {[...links]
          .sort((a, b) => b.value - a.value)
          .slice(0, SR_FLOWS)
          .map((l) => (
            <li key={`${l.source}>${l.target}`}>
              {`${parse(l.source).name} to ${parse(l.target).name}: ${fmtNumber(l.value)} persons`}
            </li>
          ))}
      </ul>
    </div>
  );
}
