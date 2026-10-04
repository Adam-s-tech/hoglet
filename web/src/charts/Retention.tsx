import { useState } from "react";
import type { RetentionCohort } from "../types/RetentionCohort";
import type { RetentionPeriod } from "../types/RetentionPeriod";
import { fmtNumber, fmtPercent } from "../lib/format";
import { TimeSeriesChart } from "./TimeSeries";
import { seqColor } from "./scale";

const UNIT: Record<RetentionPeriod, string> = { day: "Day", week: "Week", month: "Month" };

/** Cohort triangle: rows = cohorts, columns = periods since, cells = % retained. */
export function RetentionGrid({
  period,
  cohorts,
  onCellClick,
  compact,
}: {
  period: RetentionPeriod;
  cohorts: RetentionCohort[];
  onCellClick?: (cohort: RetentionCohort, interval: number) => void;
  compact?: boolean;
}) {
  const [mode, setMode] = useState<"pct" | "count">("pct");
  const periods = Math.max(0, ...cohorts.map((c) => c.values.length));
  // Size-weighted mean per period over cohorts that have reached it.
  const mean = Array.from({ length: periods }, (_, i) => {
    let num = 0;
    let den = 0;
    for (const c of cohorts) {
      if (c.values.length > i && c.size > 0) {
        num += c.values[i];
        den += c.size;
      }
    }
    return den > 0 ? (num / den) * 100 : 0;
  });

  return (
    <div className="col gap-16">
      {!compact && (
      <TimeSeriesChart
        kind="line"
        height={180}
        axisFormat={(n) => `${n}%`}
        labels={mean.map((_, i) => `${UNIT[period]} ${i}`)}
        series={[{ key: "mean", label: "Mean retention", color: "var(--s1)", data: mean }]}
        format={(n) => fmtPercent(n)}
        legend={false}
      />
      )}
      {!compact && (
      <div className="row">
        <span className="label grow">Cohorts</span>
        <div className="seg" role="group" aria-label="Cell values">
          <button aria-pressed={mode === "pct"} onClick={() => setMode("pct")}>
            %
          </button>
          <button aria-pressed={mode === "count"} onClick={() => setMode("count")}>
            Count
          </button>
        </div>
      </div>
      )}
      <div className="table-wrap">
        <table className="retention">
          <thead>
            <tr>
              <th>Cohort</th>
              <th>Persons</th>
              {Array.from({ length: periods }, (_, i) => (
                <th key={i}>
                  {UNIT[period]} {i}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            <tr>
              <td className="cohort">
                <b>Mean</b>
              </td>
              <td className="size">{fmtNumber(cohorts.reduce((a, c) => a + c.size, 0))}</td>
              {mean.map((m, i) => {
                const c = seqColor(m / 100);
                return (
                  <td key={i} style={{ background: c.bg, color: c.fg, fontWeight: 600 }}>
                    {fmtPercent(m)}
                  </td>
                );
              })}
            </tr>
            {cohorts.map((cohort) => (
              <tr key={cohort.date}>
                <td className="cohort nowrap">{cohort.label}</td>
                <td className="size">{fmtNumber(cohort.size)}</td>
                {Array.from({ length: periods }, (_, i) => {
                  if (i >= cohort.values.length) return <td key={i} />;
                  const v = cohort.values[i];
                  const pct = cohort.size ? (v / cohort.size) * 100 : 0;
                  const c = seqColor(pct / 100);
                  return (
                    <td
                      key={i}
                      className="cell"
                      style={{ background: c.bg, color: c.fg }}
                      onClick={() => onCellClick?.(cohort, i)}
                      title={`${fmtNumber(v)} of ${fmtNumber(cohort.size)} persons (${fmtPercent(pct)}) · click to see persons`}
                      tabIndex={0}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") onCellClick?.(cohort, i);
                      }}
                    >
                      {mode === "pct" ? fmtPercent(pct) : fmtNumber(v)}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
