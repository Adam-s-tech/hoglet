// The query editor panel, one section per concern, for every InsightQuery kind.

import { useId, useState, type ReactNode } from "react";
import type { EventNode } from "@/types/EventNode";
import type { FunnelsQuery } from "@/types/FunnelsQuery";
import type { InsightQuery } from "@/types/InsightQuery";
import type { Math as MathKind } from "@/types/Math";
import type { PathsQuery } from "@/types/PathsQuery";
import type { RetentionQuery } from "@/types/RetentionQuery";
import type { WindowUnit } from "@/types/WindowUnit";
import { seriesColor } from "@/charts/scale";
import { Seg } from "@/components/controls";
import { Icon, type IconName } from "@/components/icons";
import { StatLabel } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { eventLabel } from "@/lib/properties";
import { MATHS, PROPERTY_MATHS, eventNode, letter } from "./defaults";
import { BreakdownPicker, EventPicker, OptionSelect, PropertyFilters, PropertyPicker } from "./pickers";

function Section({ label, children, aside }: { label: string; children: ReactNode; aside?: ReactNode }) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="border-b px-4 py-3.5 last:border-b-0">
      <div className="mb-2 flex min-h-6 items-center gap-2">
        <StatLabel id={id} className="flex-1">
          {label}
        </StatLabel>
        {aside}
      </div>
      {children}
    </section>
  );
}

/** Icon-only button with a tooltip (shown on hover and on keyboard focus). */
function IconAction({ label, icon, onClick, pressed }: { label: string; icon: IconName; onClick: () => void; pressed?: boolean }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={<Button variant={pressed ? "secondary" : "ghost"} size="icon-xs" aria-label={label} aria-pressed={pressed} onClick={onClick} />}
      >
        <Icon name={icon} size={13} />
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
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
    <div className="group/series -mx-2 mb-1 flex flex-col gap-1.5 rounded-lg p-2 hover:bg-muted/40 focus-within:bg-muted/40">
      <div className="flex items-center gap-1.5">
        {mode === "funnel" ? (
          <span className="num grid size-5 flex-none place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground" aria-hidden="true">
            {index + 1}
          </span>
        ) : (
          <span className="grid size-5 flex-none place-items-center rounded-[5px] text-[11px] font-semibold text-white" style={{ background: seriesColor(index) }} aria-hidden="true">
            {letter(index)}
          </span>
        )}
        <div className="min-w-0 flex-1">
          <EventPicker value={node.event} onChange={(event) => onChange({ ...node, event })} allowAll={mode !== "funnel" || index > 0} />
        </div>
        <div className="flex flex-none items-center opacity-0 transition-opacity group-focus-within/series:opacity-100 group-hover/series:opacity-100 pointer-coarse:opacity-100">
          <IconAction label="Filter this series" icon="filter" pressed={showFilters} onClick={() => setShowFilters((v) => !v)} />
          <IconAction label="Rename series" icon="edit" pressed={renaming} onClick={() => setRenaming((v) => !v)} />
          <IconAction label="Duplicate series" icon="copy" onClick={onDuplicate} />
          {canRemove && <IconAction label="Remove series" icon="trash" onClick={onRemove} />}
        </div>
      </div>
      {renaming && (
        <Input
          className="ml-[26px] h-7 w-auto"
          autoFocus
          placeholder={eventLabel(node.event)}
          value={node.custom_name ?? ""}
          onChange={(e) => onChange({ ...node, custom_name: e.target.value || null })}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === "Escape") setRenaming(false);
          }}
          aria-label="Series name"
        />
      )}
      {mode === "trends" && (
        <div className="ml-[26px] flex items-center gap-1.5">
          <OptionSelect<MathKind>
            label="Aggregation"
            className={propertyMath ? "flex-none" : "flex-1"}
            value={node.math}
            options={MATHS.map((m) => ({ value: m.value, label: m.label, group: m.group }))}
            onChange={(math) => onChange({ ...node, math, math_property: PROPERTY_MATHS.includes(math) ? node.math_property : null })}
          />
          {propertyMath && (
            <div className="min-w-0 flex-1">
              <PropertyPicker value={node.math_property} numericOnly sources={["event"]} placeholder="of property…" onChange={(key) => onChange({ ...node, math_property: key })} />
            </div>
          )}
        </div>
      )}
      {showFilters && (
        <div className="ml-[26px]">
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
        <Button variant="ghost" size="sm" onClick={() => onChange([...series, eventNode(mode === "funnel" ? null : "$pageview")])}>
          <Icon name="plus" size={13} /> {addLabel}
        </Button>
      )}
    </div>
  );
}

const WINDOW_UNITS = ["minute", "hour", "day", "week"] as const;

function FunnelExtras({ q, set }: { q: FunnelsQuery; set: (q: FunnelsQuery) => void }) {
  const stepOptions = q.series.map((_, s) => ({ value: s, label: String(s + 1) }));
  return (
    <>
      <Section label="Conversion window">
        <div className="flex items-center gap-1.5">
          <Input
            className="w-20 flex-none"
            type="number"
            min={1}
            max={365}
            value={q.funnel_window.interval}
            onChange={(e) => set({ ...q, funnel_window: { ...q.funnel_window, interval: Math.max(1, Math.min(365, Number(e.target.value) || 1)) } })}
            aria-label="Window length"
          />
          <OptionSelect<WindowUnit>
            label="Window unit"
            className="flex-1"
            value={q.funnel_window.unit}
            options={WINDOW_UNITS.map((u) => ({ value: u, label: `${u}s` }))}
            onChange={(unit) => set({ ...q, funnel_window: { ...q.funnel_window, unit } })}
          />
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
        <div className="flex flex-col gap-2">
          {q.exclusions.map((ex, i) => (
            <div key={i} className="flex flex-col gap-1.5">
              <div className="flex items-center gap-1.5">
                <div className="min-w-0 flex-1">
                  <EventPicker value={ex.event || undefined} allowAll={false} onChange={(event) => set({ ...q, exclusions: q.exclusions.map((x, j) => (j === i ? { ...x, event: event ?? "" } : x)) })} />
                </div>
                <IconAction label="Remove exclusion" icon="x" onClick={() => set({ ...q, exclusions: q.exclusions.filter((_, j) => j !== i) })} />
              </div>
              <div className="flex items-center gap-1.5 text-muted-foreground">
                between step
                <OptionSelect
                  size="sm"
                  label="From step"
                  value={ex.from_step}
                  options={stepOptions}
                  onChange={(from_step) => set({ ...q, exclusions: q.exclusions.map((x, j) => (j === i ? { ...x, from_step } : x)) })}
                />
                and
                <OptionSelect
                  size="sm"
                  label="To step"
                  value={ex.to_step}
                  options={stepOptions.map((o) => ({ ...o, disabled: o.value <= ex.from_step }))}
                  onChange={(to_step) => set({ ...q, exclusions: q.exclusions.map((x, j) => (j === i ? { ...x, to_step } : x)) })}
                />
              </div>
            </div>
          ))}
          <Button
            variant="ghost"
            size="sm"
            className="self-start"
            onClick={() => set({ ...q, exclusions: [...q.exclusions, { event: "", from_step: 0, to_step: Math.max(1, q.series.length - 1) }] })}
          >
            <Icon name="plus" size={13} /> Add exclusion
          </Button>
        </div>
      </Section>
    </>
  );
}

function RetentionEditor({ q, set }: { q: RetentionQuery; set: (q: RetentionQuery) => void }) {
  return (
    <>
      <Section label="Cohort: persons who performed">
        <div className="flex flex-col gap-2">
          <EventPicker value={q.target.event} onChange={(event) => set({ ...q, target: { ...q.target, event } })} />
          <OptionSelect<RetentionQuery["retention_type"]>
            label="Cohort type"
            className="w-full"
            value={q.retention_type}
            options={[
              { value: "retention_recurring", label: "in each period (recurring)" },
              { value: "retention_first_time", label: "for the first time" },
            ]}
            onChange={(retention_type) => set({ ...q, retention_type })}
          />
        </div>
      </Section>
      <Section label="Came back and performed">
        <EventPicker value={q.returning.event} onChange={(event) => set({ ...q, returning: { ...q.returning, event } })} />
      </Section>
      <Section label="Periods">
        <div className="flex items-center gap-1.5">
          <OptionSelect<RetentionQuery["period"]>
            label="Period"
            className="flex-1"
            value={q.period}
            options={[
              { value: "day", label: "Daily" },
              { value: "week", label: "Weekly" },
              { value: "month", label: "Monthly" },
            ]}
            onChange={(period) => set({ ...q, period })}
          />
          <Input
            className="w-20 flex-none"
            type="number"
            min={2}
            max={31}
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
  const pointPlaceholder = q.paths_type === "custom_events" ? "Event name (any)" : "/path (any)";
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
        <Input placeholder={pointPlaceholder} value={q.start_point ?? ""} onChange={(e) => set({ ...q, start_point: e.target.value || null })} aria-label="Start point" />
      </Section>
      <Section label="Ends at">
        <Input placeholder={pointPlaceholder} value={q.end_point ?? ""} onChange={(e) => set({ ...q, end_point: e.target.value || null })} aria-label="End point" />
      </Section>
      <Section label="Steps" aside={<span className="num text-muted-foreground">{q.step_limit}</span>}>
        <Slider label="Maximum steps" min={2} max={10} value={q.step_limit} onValueChange={(step_limit) => set({ ...q, step_limit })} />
      </Section>
      <Section label="Links shown">
        <OptionSelect
          label="Maximum links"
          className="w-full"
          value={q.edge_limit}
          options={[20, 50, 100, 200].map((n) => ({ value: n, label: `Strongest ${n}` }))}
          onChange={(edge_limit) => set({ ...q, edge_limit })}
        />
      </Section>
    </>
  );
}

export function QueryEditor({ query, onChange }: { query: InsightQuery; onChange: (q: InsightQuery) => void }) {
  const filters =
    "properties" in query ? (
      <Section label="Filters">
        <PropertyFilters value={query.properties} onChange={(properties) => onChange({ ...query, properties })} />
      </Section>
    ) : null;

  switch (query.kind) {
    case "TrendsQuery":
      return (
        <div className="flex flex-col">
          <Section label="Series">
            <SeriesList series={query.series} mode="trends" addLabel="Add series" max={8} onChange={(series) => onChange({ ...query, series })} />
          </Section>
          <Section
            label="Formula"
            aside={
              <Button variant="ghost" size="xs" onClick={() => onChange({ ...query, formula: query.formula === null ? (query.series.length > 1 ? "A / B" : "A * 1") : null })}>
                {query.formula === null ? "Add" : "Remove"}
              </Button>
            }
          >
            {query.formula !== null ? (
              <div className="flex flex-col gap-1">
                <Input className="font-mono" value={query.formula} onChange={(e) => onChange({ ...query, formula: e.target.value })} placeholder="A / B * 100" aria-label="Formula" />
                <span className="text-muted-foreground">Use series letters with + − × ÷ and parentheses.</span>
              </div>
            ) : (
              <span className="text-muted-foreground">Combine series arithmetically, e.g. conversion rate A / B.</span>
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
        <div className="flex flex-col">
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
        <div className="flex flex-col">
          <RetentionEditor q={query} set={(q) => onChange({ ...q, kind: "RetentionQuery" })} />
          {filters}
        </div>
      );
    case "LifecycleQuery":
      return (
        <div className="flex flex-col">
          <Section label="Persons who performed">
            <SeriesList series={[query.series]} mode="plain" addLabel="" max={1} onChange={(s) => onChange({ ...query, series: s[0] ?? query.series })} />
          </Section>
          {filters}
          <div className="px-4 py-3.5 text-muted-foreground">
            New: first time ever. Returning: active this and last period. Resurrecting: back after an inactive period. Dormant: active last period, not this one.
          </div>
        </div>
      );
    case "StickinessQuery":
      return (
        <div className="flex flex-col">
          <Section label="Series">
            <SeriesList series={query.series} mode="plain" addLabel="Add series" max={8} onChange={(series) => onChange({ ...query, series })} />
          </Section>
          {filters}
        </div>
      );
    case "PathsQuery":
      return (
        <div className="flex flex-col">
          <PathsEditor q={query} set={(q) => onChange({ ...q, kind: "PathsQuery" })} />
          {filters}
        </div>
      );
    case "SqlQuery":
      return null;
  }
}
