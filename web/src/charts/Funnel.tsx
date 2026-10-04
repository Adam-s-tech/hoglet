import type { FunnelBreakdownResult } from "@/types/FunnelBreakdownResult";
import type { FunnelStepResult } from "@/types/FunnelStepResult";
import { fmtDuration, fmtNumber, fmtPercent } from "@/lib/format";
import { eventLabel } from "@/lib/properties";
import { cn } from "@/lib/utils";
import { Swatch } from "./parts";
import { seriesColor } from "./scale";

export function breakdownLabel(value: string): string {
  if (value === "$$_other") return "Other";
  if (value === "$$_none") return "(none)";
  return value;
}

const FOCUS = "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:outline-none";
const HATCH = "bg-muted [background-image:repeating-linear-gradient(135deg,transparent_0_5px,var(--axis)_5px_6px)]";
const LINK_BTN = "rounded-sm text-inherit hover:underline focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none";

/** Horizontal funnel: each step a track; solid = converted, hatched = dropped off. */
export function FunnelChart({
  steps,
  breakdowns,
  onSelect,
  compact,
}: {
  steps: FunnelStepResult[];
  breakdowns: FunnelBreakdownResult[];
  onSelect?: (step: number, converted: boolean, breakdownValue: string | null, label: string) => void;
  compact?: boolean;
}) {
  if (steps.length === 0) return null;
  const first = steps[0].count || 1;
  const last = steps[steps.length - 1];
  const showBreakdown = breakdowns.length > 0;
  const statLabel = "text-xs font-semibold tracking-wide text-muted-foreground uppercase";

  return (
    <div className={cn("flex flex-col", compact ? "gap-2.5" : "gap-4")}>
      {!compact && (
        <div className="flex flex-wrap gap-x-6 gap-y-3">
          <div>
            <div className={statLabel}>Total conversion</div>
            <div className="num text-2xl font-semibold">{fmtPercent(last.conversion_from_start)}</div>
          </div>
          <div>
            <div className={statLabel}>Converted</div>
            <div className="num text-2xl font-semibold">
              {fmtNumber(last.count)} <span className="text-sm font-medium text-muted-foreground">of {fmtNumber(steps[0].count)}</span>
            </div>
          </div>
          {last.median_conversion_time_s !== null && steps.length > 1 && (
            <div>
              <div className={statLabel}>Median time to convert</div>
              <div className="num text-2xl font-semibold">{fmtDuration(steps.slice(1).reduce((a, s) => a + (s.median_conversion_time_s ?? 0), 0))}</div>
            </div>
          )}
        </div>
      )}

      {showBreakdown && (
        <ul className="flex flex-wrap gap-x-3.5 gap-y-1 text-xs text-muted-foreground" aria-label="Breakdown values">
          {breakdowns.map((b, i) => (
            <li key={b.breakdown_value} className="flex items-center gap-1">
              <Swatch color={seriesColor(i)} />
              {breakdownLabel(b.breakdown_value)}
            </li>
          ))}
        </ul>
      )}

      <ol className={cn("flex flex-col", compact ? "gap-2.5" : "gap-[18px]")} aria-label="Funnel steps">
        {steps.map((s, i) => {
          const prev = i > 0 ? steps[i - 1] : null;
          const convPct = (s.count / first) * 100;
          const dropPct = prev ? ((prev.count - s.count) / first) * 100 : 0;
          return (
            <li key={i}>
              <div className={cn("flex items-baseline gap-2.5", compact ? "mb-[3px] text-[13px]" : "mb-1.5")}>
                <span className="inline-grid size-[22px] flex-none place-items-center self-center rounded-full bg-muted text-[11.5px] font-bold text-muted-foreground">
                  {i + 1}
                </span>
                <span className="min-w-0 truncate font-semibold">{eventLabel(s.name)}</span>
                <span className="flex-1" />
                <span className="num text-muted-foreground">
                  <b className="font-semibold text-foreground">{fmtNumber(s.count)}</b> persons
                </span>
              </div>
              {!showBreakdown ? (
                <div className={cn("relative flex overflow-hidden rounded bg-muted", compact ? "h-5" : "h-[34px]")}>
                  <button
                    type="button"
                    className={cn("h-full rounded-l transition-[filter] hover:brightness-110 motion-reduce:transition-none", FOCUS)}
                    style={{ width: `${convPct}%`, background: "var(--s1)" }}
                    onClick={() => onSelect?.(i, true, null, `Completed step ${i + 1}: ${s.name}`)}
                    aria-label={`${fmtNumber(s.count)} persons completed step ${i + 1}, ${eventLabel(s.name)}. Show persons.`}
                    title={`${fmtNumber(s.count)} completed · click to see persons`}
                  />
                  {prev && dropPct > 0 && (
                    <button
                      type="button"
                      className={cn("h-full opacity-90 transition-[filter] hover:brightness-95 motion-reduce:transition-none", HATCH, FOCUS)}
                      style={{ width: `${dropPct}%` }}
                      onClick={() => onSelect?.(i, false, null, `Dropped off before step ${i + 1}: ${s.name}`)}
                      aria-label={`${fmtNumber(s.dropped_off)} persons dropped off before step ${i + 1}, ${eventLabel(s.name)}. Show persons.`}
                      title={`${fmtNumber(s.dropped_off)} dropped off · click to see persons`}
                    />
                  )}
                </div>
              ) : (
                <div className="flex flex-col gap-[3px]">
                  {breakdowns.map((b, bi) => {
                    const bs = b.steps[i];
                    const base = b.steps[0]?.count || 1;
                    if (!bs) return null;
                    return (
                      <div key={b.breakdown_value} className="flex items-center gap-2.5">
                        <div className="flex h-[18px] flex-1 overflow-hidden rounded bg-muted">
                          <button
                            type="button"
                            className={cn("h-full rounded-l transition-[filter] hover:brightness-110 motion-reduce:transition-none", FOCUS)}
                            style={{ width: `${(bs.count / base) * 100}%`, background: seriesColor(bi) }}
                            onClick={() => onSelect?.(i, true, b.breakdown_value, `${breakdownLabel(b.breakdown_value)} · completed step ${i + 1}`)}
                            aria-label={`${breakdownLabel(b.breakdown_value)}: ${fmtNumber(bs.count)} persons completed step ${i + 1}. Show persons.`}
                            title={`${breakdownLabel(b.breakdown_value)}: ${fmtNumber(bs.count)}`}
                          />
                        </div>
                        <span className="num w-[120px] text-right text-xs">
                          {fmtNumber(bs.count)} · {fmtPercent(bs.conversion_from_start)}
                        </span>
                      </div>
                    );
                  })}
                </div>
              )}
              <div className={cn("flex flex-wrap gap-x-[18px] gap-y-1 text-muted-foreground", compact ? "mt-[3px] text-xs" : "mt-1.5 text-[12.5px]")}>
                {i === 0 ? (
                  <span>Entered the funnel</span>
                ) : (
                  <>
                    <span>
                      <b className="num font-semibold text-foreground">{fmtPercent(s.conversion_from_previous)}</b> from previous step
                    </span>
                    <span>
                      <b className="num font-semibold text-foreground">{fmtPercent(s.conversion_from_start)}</b> overall
                    </span>
                    <button
                      type="button"
                      className={LINK_BTN}
                      onClick={() => onSelect?.(i, false, null, `Dropped off before step ${i + 1}: ${s.name}`)}
                    >
                      <b className="num font-semibold text-destructive">{fmtNumber(s.dropped_off)}</b> dropped off
                    </button>
                    {s.median_conversion_time_s !== null && (
                      <span>
                        median <b className="num font-semibold text-foreground">{fmtDuration(s.median_conversion_time_s)}</b>
                        {s.average_conversion_time_s !== null && <> · avg {fmtDuration(s.average_conversion_time_s)}</>}
                      </span>
                    )}
                  </>
                )}
              </div>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
