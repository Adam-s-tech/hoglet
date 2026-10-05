// Paths as a Sankey: step columns, nodes sized by persons, bands by flow.

import { useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { PathLink } from "@/types/PathLink";
import { fmtDuration, fmtNumber } from "@/lib/format";
import { useSize } from "@/lib/hooks";
import { DataTableToggle, Tip, TipRow, TipTitle } from "./parts";

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

function FlowList({ title, flows, pick, onOpen }: { title: string; flows: PathLink[]; pick: (l: PathLink) => string; onOpen?: (l: PathLink) => void }) {
  return (
    <div className="min-w-0">
      <div className="mb-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{title}</div>
      {flows.length === 0 ? (
        <p className="text-muted-foreground">Nothing: this is an {title === "Came from" ? "entry" : "exit"} point.</p>
      ) : (
        <ul>
          {flows.slice(0, 12).map((l) => (
            <li key={linkId(l)} className="flex gap-2 py-px">
              {onOpen ? (
                <button
                  type="button"
                  className="flex min-w-0 flex-1 gap-2 rounded text-left hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
                  onClick={() => onOpen(l)}
                  aria-label={`${pick(l)}: ${fmtNumber(l.value)} persons. Show the people.`}
                >
                  <span className="min-w-0 flex-1 truncate">{pick(l)}</span>
                  <span className="num font-semibold">{fmtNumber(l.value)}</span>
                </button>
              ) : (
                <>
                  <span className="min-w-0 flex-1 truncate">{pick(l)}</span>
                  <span className="num font-semibold">{fmtNumber(l.value)}</span>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function parse(id: string): { step: number; name: string } {
  const m = /^(\d+)_(.*)$/.exec(id);
  return m ? { step: Number(m[1]), name: m[2] } : { step: 1, name: id };
}

function short(s: string, n = 28): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

/** Least width per step column before the diagram scrolls horizontally. */
const MIN_STEP_W = 150;

const linkId = (l: PathLink) => `${l.source}>${l.target}`;

export function PathsSankey({ links, compact = false, onOpenLink }: { links: PathLink[]; compact?: boolean; onOpenLink?: (link: PathLink) => void }) {
  const [ref, size] = useSize<HTMLDivElement>();
  const [hot, setHot] = useState<{ kind: "node" | "link"; id: string } | null>(null);
  const [tip, setTip] = useState<{ x: number; y: number; band: Band } | null>(null);
  /** Roving tab stop: the one node or flow that Tab lands on; arrow keys move it. */
  const [active, setActive] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const detailId = useId();

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

  // Narrow screens scroll sideways instead of squeezing the steps together.
  const width = size.width === 0 ? 0 : Math.max(size.width, layout.columns.length * MIN_STEP_W);
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

  const isHot = (b: Band) => (hot === null ? null : hot.kind === "link" ? linkId(b.link) === hot.id : b.s.id === hot.id || b.t.id === hot.id);

  // Keyboard order: every node (step by step, top to bottom), then every flow (strongest first).
  const items = useMemo(
    () => [
      ...layout.columns.flatMap((c) => [...c].sort((a, b) => a.y - b.y).map((n) => ({ kind: "node" as const, id: n.id }))),
      ...[...links].sort((a, b) => b.value - a.value).map((l) => ({ kind: "link" as const, id: linkId(l) })),
    ],
    [layout, links],
  );
  const activeId = active !== null && items.some((it) => it.id === active) ? active : (items[0]?.id ?? null);

  const focusItem = (id: string) => {
    setActive(id);
    svgRef.current?.querySelector<SVGElement>(`[data-item="${CSS.escape(id)}"]`)?.focus();
  };
  const onItemKey = (e: KeyboardEvent<SVGElement>, kind: "node" | "link", id: string) => {
    const at = items.findIndex((it) => it.id === id);
    let to = -1;
    if (e.key === "ArrowDown" || e.key === "ArrowRight") to = Math.min(items.length - 1, at + 1);
    else if (e.key === "ArrowUp" || e.key === "ArrowLeft") to = Math.max(0, at - 1);
    else if (e.key === "Home") to = 0;
    else if (e.key === "End") to = items.length - 1;
    else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      // A flow opens the people on it (or, without a handler, the step it leads
      // to); a node opens its own flows.
      if (kind === "link" && onOpenLink) {
        const link = links.find((l) => linkId(l) === id);
        if (link) onOpenLink(link);
        return;
      }
      setSelected(kind === "node" ? id : (layout.nodes.get(id.slice(id.indexOf(">") + 1))?.id ?? null));
      return;
    } else if (e.key === "Escape") {
      setSelected(null);
      return;
    }
    if (to >= 0) {
      e.preventDefault();
      focusItem(items[to].id);
    }
  };
  const nodeLabel = (n: Node) => `Step ${n.step}, ${n.name}: ${fmtNumber(n.value)} persons. Press Enter for its flows.`;
  const flowLabel = (l: PathLink) => `${parse(l.source).name} to ${parse(l.target).name}: ${fmtNumber(l.value)} persons. Press Enter ${onOpenLink ? "to see the people" : "for the step it leads to"}.`;
  const focusTip = (b: Band, el: SVGElement) => {
    const box = svgRef.current?.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    setTip(box ? { x: r.left - box.left + r.width / 2, y: r.top - box.top + r.height / 2, band: b } : null);
  };
  const sel = selected ? layout.nodes.get(selected) : undefined;
  const selIn = sel ? links.filter((l) => l.target === sel.id).sort((a, b) => b.value - a.value) : [];
  const selOut = sel ? links.filter((l) => l.source === sel.id).sort((a, b) => b.value - a.value) : [];

  return (
    <div ref={ref}>
      <div className="overflow-x-auto pb-1">
      <div className="chart" style={{ width }}>
      <svg
        ref={svgRef}
        width={width}
        height={height}
        role="group"
        aria-roledescription="paths diagram"
        aria-label={`User paths across ${cols} steps, ${layout.nodes.size} nodes and ${links.length} flows. Tab into the diagram, then use arrow keys to move between steps and flows. The data table below lists every flow.`}
      >
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
              const id = linkId(b.link);
              return (
                <path
                  key={id}
                  data-item={id}
                  role="button"
                  tabIndex={activeId === id ? 0 : -1}
                  aria-label={flowLabel(b.link)}
                  className={`sankey-link${h === true ? " hot" : h === false ? " dim" : ""}`}
                  d={`M${x0},${b.sy}C${mx},${b.sy} ${mx},${b.ty} ${x1},${b.ty}`}
                  stroke="var(--s1)"
                  strokeWidth={b.w}
                  onKeyDown={(e) => onItemKey(e, "link", id)}
                  onClick={onOpenLink ? () => onOpenLink(b.link) : undefined}
                  style={onOpenLink ? { cursor: "pointer" } : undefined}
                  onFocus={(e) => {
                    setActive(id);
                    setHot({ kind: "link", id });
                    focusTip(b, e.currentTarget);
                  }}
                  onBlur={() => {
                    setHot(null);
                    setTip(null);
                  }}
                  onPointerEnter={() => setHot({ kind: "link", id })}
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
                  data-item={n.id}
                  className="sankey-node"
                  role="button"
                  tabIndex={activeId === n.id ? 0 : -1}
                  aria-label={nodeLabel(n)}
                  aria-expanded={selected === n.id}
                  aria-controls={detailId}
                  onKeyDown={(e) => onItemKey(e, "node", n.id)}
                  onFocus={() => {
                    setActive(n.id);
                    setHot({ kind: "node", id: n.id });
                  }}
                  onBlur={() => setHot(null)}
                  onPointerEnter={() => setHot({ kind: "node", id: n.id })}
                  onPointerLeave={() => setHot(null)}
                  onClick={() => {
                    setActive(n.id);
                    setSelected((cur) => (cur === n.id ? null : n.id));
                  }}
                  style={{ cursor: "pointer" }}
                >
                  <rect x={n.x} y={n.y} width={nodeW} height={n.h} rx={2} fill="var(--s1)" />
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
      </div>
      </div>
      <div id={detailId} aria-live="polite">
        {sel && (
          <section className="mt-3 rounded-lg border p-3 text-sm" aria-label={`Flows at ${sel.name}`}>
            <div className="mb-2 flex items-center gap-2">
              <h2 className="min-w-0 flex-1 truncate text-sm">
                Step {sel.step} · {sel.name}
              </h2>
              <span className="num text-muted-foreground">{fmtNumber(sel.value)} persons</span>
              <button type="button" className="rounded px-1 text-xs text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none" onClick={() => setSelected(null)}>
                Close
              </button>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <FlowList title="Came from" flows={selIn} pick={(l) => parse(l.source).name} onOpen={onOpenLink} />
              <FlowList title="Went to" flows={selOut} pick={(l) => parse(l.target).name} onOpen={onOpenLink} />
            </div>
          </section>
        )}
      </div>
      {!compact && (
      <DataTableToggle
        caption="Path flows"
        head={["From", "To", "Persons", "Share of source", "Avg time between"]}
        rows={[...links]
          .sort((a, b) => b.value - a.value)
          .map((l) => {
            const from = layout.nodes.get(l.source);
            return [
              `Step ${from?.step ?? "?"} · ${parse(l.source).name}`,
              parse(l.target).name,
              fmtNumber(l.value),
              `${((l.value / Math.max(1, from?.value ?? 1)) * 100).toFixed(1)}%`,
              fmtDuration(l.average_conversion_time_s),
            ];
          })}
      />
      )}
    </div>
  );
}
