// The query editor panel, one section per concern, for every InsightQuery kind.

import { useState, type ReactNode } from "react";
import type { EventNode } from "../types/EventNode";
import type { FunnelsQuery } from "../types/FunnelsQuery";
import type { InsightQuery } from "../types/InsightQuery";
import type { Math as MathKind } from "../types/Math";
import type { PathsQuery } from "../types/PathsQuery";
import type { RetentionQuery } from "../types/RetentionQuery";
import type { WindowUnit } from "../types/WindowUnit";
import { seriesColor } from "../charts/scale";
import { eventLabel } from "../lib/properties";
import { Icon } from "../ui/icons";
import { Seg } from "../ui/kit";
import { MATHS, PROPERTY_MATHS, eventNode, letter } from "./defaults";
import { BreakdownPicker, EventPicker, PropertyFilters, PropertyPicker } from "./pickers";

function Section({ label, children, aside }: { label: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="editor-section">
      <div className="row" style={{ marginBottom: 8 }}>
        <span className="label grow">{label}</span>
        {aside}
      </div>
      {children}
    </div>
  );
}

type SeriesMode = "trends" | "funnel" | "plain";

function SeriesRow({
  node,
  index,
  mode,
  onChange,
  onRemove,
  onDuplicate,
  canRemove,
}: {
  node: EventNode;
  index: number;
  mode: SeriesMode;
  onChange: (n: EventNode) => void;
  onRemove: () => void;
  onDuplicate: () => void;
  canRemove: boolean;
}) {
  const [showFilters, setShowFilters] = useState(node.properties.length > 0);
  const [renaming, setRenaming] = useState(false);
  const propertyMath = PROPERTY_MATHS.includes(node.math);
  return (
    <div className="series-row">
      <div className="row" style={{ gap: 6 }}>
        {mode === "funnel" ? (
          <span className="letter" style={{ background: "var(--surface-3)", color: "var(--ink-2)", borderRadius: "50%" }}>
            {index + 1}
          </span>
        ) : (
          <span className="letter" style={{ background: seriesColor(index) }}>
            {letter(index)}
          </span>
        )}
        <EventPicker value={node.event} onChange={(event) => onChange({ ...node, event })} allowAll={mode !== "funnel" || index > 0} />
        <div className="row row-actions" style={{ gap: 0 }}>
          <button className="btn ghost icon small" title="Filter this series" aria-label="Filter this series" onClick={() => setShowFilters((v) => !v)} aria-pressed={showFilters}>
            <Icon name="filter" size={13} />
          </button>
          <button className="btn ghost icon small" title="Rename" aria-label="Rename series" onClick={() => setRenaming((v) => !v)}>
            <Icon name="edit" size={13} />
          </button>
          <button className="btn ghost icon small" title="Duplicate" aria-label="Duplicate series" onClick={onDuplicate}>
            <Icon name="copy" size={13} />
          </button>
          {canRemove && (
            <button className="btn ghost icon small" title="Remove" aria-label="Remove series" onClick={onRemove}>
              <Icon name="trash" size={13} />
            </button>
          )}
        </div>
      </div>
      {renaming && (
        <input
          className="input small"
          style={{ marginLeft: 26 }}
          autoFocus
          placeholder={eventLabel(node.event)}
          value={node.custom_name ?? ""}
          onChange={(e) => onChange({ ...node, custom_name: e.target.value || null })}
          onKeyDown={(e) => e.key === "Enter" && setRenaming(false)}
          aria-label="Series name"
        />
      )}
      {mode === "trends" && (
        <div className="row" style={{ marginLeft: 26, gap: 6 }}>
          <select
            className="select small"
            value={node.math}
            aria-label="Aggregation"
            onChange={(e) => {
              const math = e.target.value as MathKind;
              onChange({ ...node, math, math_property: PROPERTY_MATHS.includes(math) ? node.math_property : null });
            }}
            style={{ flex: propertyMath ? "0 0 auto" : 1 }}
          >
            {(["Events", "Users", "Sessions", "Property"] as const).map((g) => (
              <optgroup key={g} label={g}>
                {MATHS.filter((m) => m.group === g).map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
          {propertyMath && (
            <PropertyPicker value={node.math_property} numericOnly sources={["event"]} placeholder="of property…" onChange={(key) => onChange({ ...node, math_property: key })} />
          )}
        </div>
      )}
      {showFilters && (
        <div style={{ marginLeft: 26 }}>
          <PropertyFilters value={node.properties} onChange={(properties) => onChange({ ...node, properties })} addLabel="Series filter" />
        </div>
      )}
    </div>
  );
}

function SeriesList({ series, onChange, mode, max = 12, addLabel }: { series: EventNode[]; onChange: (s: EventNode[]) => void; mode: SeriesMode; max?: number; addLabel: string }) {
  const min = mode === "funnel" ? 2 : 1;
  return (
    <div>
      {series.map((node, i) => (
        <SeriesRow
          key={i}
          node={node}
          index={i}
          mode={mode}
          canRemove={series.length > min}
          onChange={(n) => onChange(series.map((x, j) => (j === i ? n : x)))}
          onRemove={() => onChange(series.filter((_, j) => j !== i))}
          onDuplicate={() => onChange([...series.slice(0, i + 1), { ...node }, ...series.slice(i + 1)])}
        />
      ))}
      {series.length < max && (
        <button className="btn ghost small" onClick={() => onChange([...series, eventNode(mode === "funnel" ? null : "$pageview")])}>
          <Icon name="plus" size={13} /> {addLabel}
        </button>
      )}
    </div>
  );
}

function FunnelExtras({ q, set }: { q: FunnelsQuery; set: (q: FunnelsQuery) => void }) {
  return (
    <>
      <Section label="Conversion window">
        <div className="row">
          <input
            className="input"
            type="number"
            min={1}
            max={365}
            style={{ width: 80 }}
            value={q.funnel_window.interval}
            onChange={(e) => set({ ...q, funnel_window: { ...q.funnel_window, interval: Math.max(1, Math.min(365, Number(e.target.value) || 1)) } })}
            aria-label="Window length"
          />
          <select className="select grow" value={q.funnel_window.unit} onChange={(e) => set({ ...q, funnel_window: { ...q.funnel_window, unit: e.target.value as WindowUnit } })} aria-label="Window unit">
            {(["minute", "hour", "day", "week"] as const).map((u) => (
              <option key={u} value={u}>
                {u}s
              </option>
            ))}
          </select>
        </div>
      </Section>
      <Section label="Step order">
        <Seg
          label="Step order"
          value={q.funnel_order}
          onChange={(funnel_order) => set({ ...q, funnel_order })}
          options={[
            { value: "ordered", label: "Sequential", title: "Steps in order; other events may happen in between" },
            { value: "strict", label: "Strict", title: "Steps in order with nothing in between" },
            { value: "unordered", label: "Any order", title: "Steps in any order" },
          ]}
        />
      </Section>
      <Section label="Exclusion steps">
        <div className="col" style={{ gap: 8 }}>
          {q.exclusions.map((ex, i) => (
            <div key={i} className="col" style={{ gap: 6 }}>
              <div className="row">
                <EventPicker value={ex.event || undefined} allowAll={false} onChange={(event) => set({ ...q, exclusions: q.exclusions.map((x, j) => (j === i ? { ...x, event: event ?? "" } : x)) })} />
                <button className="btn ghost icon small" aria-label="Remove exclusion" onClick={() => set({ ...q, exclusions: q.exclusions.filter((_, j) => j !== i) })}>
                  <Icon name="x" size={13} />
                </button>
              </div>
              <div className="row small secondary">
                between step
                <select
                  className="select small"
                  value={ex.from_step}
                  onChange={(e) => set({ ...q, exclusions: q.exclusions.map((x, j) => (j === i ? { ...x, from_step: Number(e.target.value) } : x)) })}
                  aria-label="From step"
                >
                  {q.series.map((_, s) => (
                    <option key={s} value={s}>
                      {s + 1}
                    </option>
                  ))}
                </select>
                and
                <select
                  className="select small"
                  value={ex.to_step}
                  onChange={(e) => set({ ...q, exclusions: q.exclusions.map((x, j) => (j === i ? { ...x, to_step: Number(e.target.value) } : x)) })}
                  aria-label="To step"
                >
                  {q.series.map((_, s) => (
                    <option key={s} value={s} disabled={s <= ex.from_step}>
                      {s + 1}
                    </option>
                  ))}
                </select>
              </div>
            </div>
          ))}
          <button className="btn ghost small" style={{ alignSelf: "flex-start" }} onClick={() => set({ ...q, exclusions: [...q.exclusions, { event: "", from_step: 0, to_step: Math.max(1, q.series.length - 1) }] })}>
            <Icon name="plus" size={13} /> Add exclusion
          </button>
        </div>
      </Section>
    </>
  );
}

function RetentionEditor({ q, set }: { q: RetentionQuery; set: (q: RetentionQuery) => void }) {
  return (
    <>
      <Section label="Cohort: persons who performed">
        <div className="col" style={{ gap: 8 }}>
          <EventPicker value={q.target.event} onChange={(event) => set({ ...q, target: { ...q.target, event } })} />
          <select className="select" value={q.retention_type} onChange={(e) => set({ ...q, retention_type: e.target.value as RetentionQuery["retention_type"] })} aria-label="Cohort type">
            <option value="retention_recurring">in each period (recurring)</option>
            <option value="retention_first_time">for the first time</option>
          </select>
        </div>
      </Section>
      <Section label="Came back and performed">
        <EventPicker value={q.returning.event} onChange={(event) => set({ ...q, returning: { ...q.returning, event } })} />
      </Section>
      <Section label="Periods">
        <div className="row">
          <select className="select grow" value={q.period} onChange={(e) => set({ ...q, period: e.target.value as RetentionQuery["period"] })} aria-label="Period">
            <option value="day">Daily</option>
            <option value="week">Weekly</option>
            <option value="month">Monthly</option>
          </select>
          <input
            className="input"
            type="number"
            min={2}
            max={31}
            style={{ width: 80 }}
            value={q.total_intervals}
            onChange={(e) => set({ ...q, total_intervals: Math.max(2, Math.min(31, Number(e.target.value) || 8)) })}
            aria-label="Number of periods"
          />
        </div>
      </Section>
    </>
  );
}

function PathsEditor({ q, set }: { q: PathsQuery; set: (q: PathsQuery) => void }) {
  return (
    <>
      <Section label="Path nodes">
        <Seg
          label="Path nodes"
          value={q.paths_type}
          onChange={(paths_type) => set({ ...q, paths_type })}
          options={[
            { value: "pageviews", label: "Pageviews" },
            { value: "custom_events", label: "Custom events" },
            { value: "all", label: "Both" },
          ]}
        />
      </Section>
      <Section label="Starts at">
        <input className="input" style={{ width: "100%" }} placeholder={q.paths_type === "custom_events" ? "Event name (any)" : "/path (any)"} value={q.start_point ?? ""} onChange={(e) => set({ ...q, start_point: e.target.value || null })} aria-label="Start point" />
      </Section>
      <Section label="Ends at">
        <input className="input" style={{ width: "100%" }} placeholder={q.paths_type === "custom_events" ? "Event name (any)" : "/path (any)"} value={q.end_point ?? ""} onChange={(e) => set({ ...q, end_point: e.target.value || null })} aria-label="End point" />
      </Section>
      <Section label="Steps" aside={<span className="num small secondary">{q.step_limit}</span>}>
        <input type="range" min={2} max={10} value={q.step_limit} onChange={(e) => set({ ...q, step_limit: Number(e.target.value) })} aria-label="Maximum steps" />
      </Section>
      <Section label="Links shown">
        <select className="select" style={{ width: "100%" }} value={q.edge_limit} onChange={(e) => set({ ...q, edge_limit: Number(e.target.value) })} aria-label="Maximum links">
          {[20, 50, 100, 200].map((n) => (
            <option key={n} value={n}>
              Strongest {n}
            </option>
          ))}
        </select>
      </Section>
    </>
  );
}

export function QueryEditor({ query, onChange }: { query: InsightQuery; onChange: (q: InsightQuery) => void }) {
  const filters = "properties" in query ? (
    <Section label="Filters">
      <PropertyFilters value={query.properties} onChange={(properties) => onChange({ ...query, properties })} />
    </Section>
  ) : null;

  switch (query.kind) {
    case "TrendsQuery":
      return (
        <div className="editor">
          <Section label="Series">
            <SeriesList series={query.series} mode="trends" addLabel="Add series" max={8} onChange={(series) => onChange({ ...query, series })} />
          </Section>
          <Section
            label="Formula"
            aside={
              <button className="btn ghost small" onClick={() => onChange({ ...query, formula: query.formula === null ? (query.series.length > 1 ? "A / B" : "A * 1") : null })}>
                {query.formula === null ? "Add" : "Remove"}
              </button>
            }
          >
            {query.formula !== null ? (
              <div className="col" style={{ gap: 4 }}>
                <input className="input mono" value={query.formula} onChange={(e) => onChange({ ...query, formula: e.target.value })} placeholder="A / B * 100" aria-label="Formula" />
                <span className="muted small">Use series letters with + − × ÷ and parentheses.</span>
              </div>
            ) : (
              <span className="muted small">Combine series arithmetically, e.g. conversion rate A / B.</span>
            )}
          </Section>
          {filters}
          <Section label="Breakdown">
            <BreakdownPicker value={query.breakdown} onChange={(breakdown) => onChange({ ...query, breakdown })} />
          </Section>
        </div>
      );
    case "FunnelsQuery":
      return (
        <div className="editor">
          <Section label="Steps">
            <SeriesList series={query.series} mode="funnel" addLabel="Add step" max={20} onChange={(series) => onChange({ ...query, series })} />
          </Section>
          <FunnelExtras q={query} set={(q) => onChange({ ...q, kind: "FunnelsQuery" })} />
          {filters}
          <Section label="Breakdown">
            <BreakdownPicker value={query.breakdown} onChange={(breakdown) => onChange({ ...query, breakdown })} />
          </Section>
        </div>
      );
    case "RetentionQuery":
      return (
        <div className="editor">
          <RetentionEditor q={query} set={(q) => onChange({ ...q, kind: "RetentionQuery" })} />
          {filters}
        </div>
      );
    case "LifecycleQuery":
      return (
        <div className="editor">
          <Section label="Persons who performed">
            <SeriesList series={[query.series]} mode="plain" addLabel="" max={1} onChange={(s) => onChange({ ...query, series: s[0] ?? query.series })} />
          </Section>
          {filters}
          <div className="editor-section muted small">
            New: first time ever. Returning: active this and last period. Resurrecting: back after an inactive period. Dormant: active last period, not this one.
          </div>
        </div>
      );
    case "StickinessQuery":
      return (
        <div className="editor">
          <Section label="Series">
            <SeriesList series={query.series} mode="plain" addLabel="Add series" max={8} onChange={(series) => onChange({ ...query, series })} />
          </Section>
          {filters}
        </div>
      );
    case "PathsQuery":
      return (
        <div className="editor">
          <PathsEditor q={query} set={(q) => onChange({ ...q, kind: "PathsQuery" })} />
          {filters}
        </div>
      );
    case "SqlQuery":
      return null;
  }
}
