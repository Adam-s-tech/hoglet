import { useState } from "react";
import type { RetentionCohort } from "@/types/RetentionCohort";
import type { RetentionPeriod } from "@/types/RetentionPeriod";
import { Seg } from "@/components/controls";
import { fmtNumber, fmtPercent } from "@/lib/format";
import { cn } from "@/lib/utils";
import { TimeSeriesChart } from "./TimeSeries";
import { seqColor } from "./scale";

const UNIT: Record<RetentionPeriod, string> = { day: "Day", week: "Week", month: "Month" };

const TH = "px-1.5 py-1 text-center text-[11.5px] font-semibold whitespace-nowrap text-muted-foreground";
const TD = "h-[34px] rounded px-1.5 text-center whitespace-nowrap num";

/**
 * Cohort triangle: rows = cohorts, columns = periods since, cells = % retained.
 * A heat map, so it is a hand-built table (colour per cell) rather than a DataTable.
 * Every drill-down cell is a real button, so it is keyboard and screen-reader reachable.
 */
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
    <div className="flex flex-col gap-4">
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
        <div className="flex items-center gap-2">
          <span className="flex-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Cohorts</span>
          <Seg
            label="Cell values"
            value={mode}
            onChange={setMode}
            options={[
              { value: "pct", label: "%" },
              { value: "count", label: "Count" },
            ]}
          />
        </div>
      )}
      <div className="w-full overflow-x-auto">
        <table className="w-full border-separate border-spacing-0.5 text-[12.5px]" aria-label="Retention by cohort">
          <thead>
            <tr>
              <th scope="col" className={cn(TH, "text-left")}>
                Cohort
              </th>
              <th scope="col" className={TH}>
                Persons
              </th>
              {Array.from({ length: periods }, (_, i) => (
                <th key={i} scope="col" className={TH}>
                  {UNIT[period]} {i}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            <tr>
              <th scope="row" className={cn(TD, "text-left font-semibold")}>
                Mean
              </th>
              <td className={cn(TD, "font-semibold")}>{fmtNumber(cohorts.reduce((a, c) => a + c.size, 0))}</td>
              {mean.map((m, i) => {
                const c = seqColor(m / 100);
                return (
                  <td key={i} className={cn(TD, "font-semibold")} style={{ background: c.bg, color: c.fg }}>
                    {fmtPercent(m)}
                  </td>
                );
              })}
            </tr>
            {cohorts.map((cohort) => (
              <tr key={cohort.date}>
                <th scope="row" className={cn(TD, "text-left font-normal text-muted-foreground")}>
                  {cohort.label}
                </th>
                <td className={cn(TD, "font-semibold")}>{fmtNumber(cohort.size)}</td>
                {Array.from({ length: periods }, (_, i) => {
                  if (i >= cohort.values.length) return <td key={i} />;
                  const v = cohort.values[i];
                  const pct = cohort.size ? (v / cohort.size) * 100 : 0;
                  const c = seqColor(pct / 100);
                  return (
                    <td key={i} className={cn(TD, "p-0")} style={{ background: c.bg, color: c.fg }}>
                      <button
                        type="button"
                        disabled={!onCellClick}
                        onClick={() => onCellClick?.(cohort, i)}
                        title={`${fmtNumber(v)} of ${fmtNumber(cohort.size)} persons (${fmtPercent(pct)}) · click to see persons`}
                        aria-label={`${cohort.label} cohort, ${UNIT[period]} ${i}: ${fmtNumber(v)} of ${fmtNumber(cohort.size)} persons, ${fmtPercent(pct)}`}
                        className="num h-full min-h-[34px] w-full rounded px-1 hover:outline-2 hover:-outline-offset-2 hover:outline-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-foreground disabled:cursor-default disabled:hover:outline-0"
                      >
                        {mode === "pct" ? fmtPercent(pct) : fmtNumber(v)}
                      </button>
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
