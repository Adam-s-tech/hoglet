import type { FunnelBreakdownResult } from "../types/FunnelBreakdownResult";
import type { FunnelStepResult } from "../types/FunnelStepResult";
import { fmtDuration, fmtNumber, fmtPercent } from "../lib/format";
import { eventLabel } from "../lib/properties";
import { seriesColor } from "./scale";

export function breakdownLabel(value: string): string {
  if (value === "$$_other") return "Other";
  if (value === "$$_none") return "(none)";
  return value;
}

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

  return (
    <div className={`col gap-16${compact ? " funnel-compact" : ""}`}>
      <div className="row gap-24 wrap funnel-summary">
        <div>
          <div className="label">Total conversion</div>
          <div style={{ fontSize: 24, fontWeight: 650 }} className="num">
            {fmtPercent(last.conversion_from_start)}
          </div>
        </div>
        <div>
          <div className="label">Converted</div>
          <div style={{ fontSize: 24, fontWeight: 650 }} className="num">
            {fmtNumber(last.count)} <span className="muted" style={{ fontSize: 14, fontWeight: 500 }}>of {fmtNumber(steps[0].count)}</span>
          </div>
        </div>
        {last.median_conversion_time_s !== null && steps.length > 1 && (
          <div>
            <div className="label">Median time to convert</div>
            <div style={{ fontSize: 24, fontWeight: 650 }} className="num">
              {fmtDuration(steps.slice(1).reduce((a, s) => a + (s.median_conversion_time_s ?? 0), 0))}
            </div>
          </div>
        )}
      </div>

      {showBreakdown && (
        <div className="legend" style={{ marginTop: 0 }}>
          {breakdowns.map((b, i) => (
            <span key={b.breakdown_value} className="row gap-4">
              <span className="swatch" style={{ background: seriesColor(i) }} />
              {breakdownLabel(b.breakdown_value)}
            </span>
          ))}
        </div>
      )}

      <div className="funnel">
        {steps.map((s, i) => {
          const prev = i > 0 ? steps[i - 1] : null;
          const convPct = (s.count / first) * 100;
          const dropPct = prev ? ((prev.count - s.count) / first) * 100 : 0;
          return (
            <div key={i}>
              <div className="fstep-head">
                <span className="idx">{i + 1}</span>
                <span className="name truncate">{eventLabel(s.name)}</span>
                <span className="spacer" />
                <span className="num secondary">
                  <b style={{ color: "var(--ink)" }}>{fmtNumber(s.count)}</b> persons
                </span>
              </div>
              {!showBreakdown ? (
                <div className="ftrack">
                  <button
                    className="conv"
                    style={{ width: `${convPct}%`, background: "var(--s1)" }}
                    onClick={() => onSelect?.(i, true, null, `Completed step ${i + 1}: ${s.name}`)}
                    aria-label={`${fmtNumber(s.count)} persons completed step ${i + 1}`}
                    title={`${fmtNumber(s.count)} completed · click to see persons`}
                  />
                  {prev && dropPct > 0 && (
                    <button
                      className="drop"
                      style={{ width: `${dropPct}%` }}
                      onClick={() => onSelect?.(i, false, null, `Dropped off before step ${i + 1}: ${s.name}`)}
                      aria-label={`${fmtNumber(s.dropped_off)} persons dropped off before step ${i + 1}`}
                      title={`${fmtNumber(s.dropped_off)} dropped off · click to see persons`}
                    />
                  )}
                </div>
              ) : (
                <div className="col" style={{ gap: 3 }}>
                  {breakdowns.map((b, bi) => {
                    const bs = b.steps[i];
                    const base = b.steps[0]?.count || 1;
                    if (!bs) return null;
                    return (
                      <div key={b.breakdown_value} className="row" style={{ gap: 10 }}>
                        <div className="ftrack grow" style={{ height: 18 }}>
                          <button
                            className="conv"
                            style={{ width: `${(bs.count / base) * 100}%`, background: seriesColor(bi) }}
                            onClick={() => onSelect?.(i, true, b.breakdown_value, `${breakdownLabel(b.breakdown_value)} · completed step ${i + 1}`)}
                            title={`${breakdownLabel(b.breakdown_value)}: ${fmtNumber(bs.count)}`}
                          />
                        </div>
                        <span className="num small" style={{ width: 120, textAlign: "right" }}>
                          {fmtNumber(bs.count)} · {fmtPercent(bs.conversion_from_start)}
                        </span>
                      </div>
                    );
                  })}
                </div>
              )}
              <div className="fstats">
                {i === 0 ? (
                  <span>Entered the funnel</span>
                ) : (
                  <>
                    <span>
                      <b>{fmtPercent(s.conversion_from_previous)}</b> from previous step
                    </span>
                    <span>
                      <b>{fmtPercent(s.conversion_from_start)}</b> overall
                    </span>
                    <button
                      className="link"
                      style={{ border: 0, background: "none", padding: 0, font: "inherit" }}
                      onClick={() => onSelect?.(i, false, null, `Dropped off before step ${i + 1}: ${s.name}`)}
                    >
                      <b style={{ color: "var(--bad)" }}>{fmtNumber(s.dropped_off)}</b> dropped off
                    </button>
                    {s.median_conversion_time_s !== null && (
                      <span>
                        median <b>{fmtDuration(s.median_conversion_time_s)}</b>
                        {s.average_conversion_time_s !== null && <> · avg {fmtDuration(s.average_conversion_time_s)}</>}
                      </span>
                    )}
                  </>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
