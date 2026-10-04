// Date range: presets (relative, always valid) plus a custom absolute range.
// Popover on Base UI: Esc closes, focus returns to the trigger.

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { RANGE_PRESETS, rangeLabel } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Icon } from "./icons";

export interface RangeValue {
  date_from: string;
  date_to: string | null;
}

function isoDay(d: Date): string {
  const z = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${z(d.getMonth() + 1)}-${z(d.getDate())}`;
}

export function DateRangePicker({
  value,
  onChange,
  small,
  presets = RANGE_PRESETS,
}: {
  value: RangeValue;
  onChange: (v: RangeValue) => void;
  small?: boolean;
  presets?: typeof RANGE_PRESETS;
}) {
  const [open, setOpen] = useState(false);
  const today = isoDay(new Date());
  const initialFrom = () => (/^\d{4}-/.test(value.date_from) ? value.date_from.slice(0, 10) : isoDay(new Date(Date.now() - 13 * 86_400_000)));
  const initialTo = () => (value.date_to && /^\d{4}-/.test(value.date_to) ? value.date_to.slice(0, 10) : today);
  const [from, setFrom] = useState(initialFrom);
  const [to, setTo] = useState(initialTo);

  const problem = !from || !to ? "Pick both dates." : from > to ? "The start date is after the end date." : to > today ? "Dates in the future have no data yet." : null;
  const days = problem ? 0 : Math.round((Date.parse(`${to}T00:00`) - Date.parse(`${from}T00:00`)) / 86_400_000) + 1;

  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        // Reopening starts from the range in force, not a half-edited one.
        if (o) {
          setFrom(initialFrom());
          setTo(initialTo());
        }
        setOpen(o);
      }}
    >
      <PopoverTrigger render={<Button variant="outline" size={small ? "sm" : "default"} />} aria-label={`Date range: ${rangeLabel(value.date_from, value.date_to)}`}>
        <Icon name="calendar" size={14} />
        {rangeLabel(value.date_from, value.date_to)}
        <Icon name="chevronDown" size={12} className="text-muted-foreground" />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 gap-0 p-1">
        <div role="listbox" aria-label="Date range presets" className="flex flex-col">
          {presets.map((p) => {
            const selected = p.date_from === value.date_from && (p.date_to ?? null) === (value.date_to ?? null);
            return (
              <button
                key={p.label}
                type="button"
                role="option"
                aria-selected={selected}
                className={cn(
                  "flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none hover:bg-accent focus-visible:bg-accent",
                  selected && "font-medium",
                )}
                onClick={() => {
                  onChange({ date_from: p.date_from, date_to: p.date_to });
                  setOpen(false);
                }}
              >
                <span className="grid w-4 place-items-center">{selected && <Icon name="check" size={14} strokeWidth={2.2} />}</span>
                {p.label}
                <span className="ml-auto text-xs text-muted-foreground">{p.short}</span>
              </button>
            );
          })}
        </div>
        <div className="my-1 h-px bg-border" />
        <div className="flex flex-col gap-2 px-2 pt-1.5 pb-2">
          <span className="text-xs font-semibold text-muted-foreground">Custom range</span>
          <div className="grid grid-cols-2 gap-2">
            <label className="flex min-w-0 flex-col gap-1 text-xs text-muted-foreground">
              From
              <Input type="date" className="h-8 min-w-0 px-1.5 text-xs text-foreground" value={from} max={to < today ? to : today} onChange={(e) => setFrom(e.target.value)} />
            </label>
            <label className="flex min-w-0 flex-col gap-1 text-xs text-muted-foreground">
              To
              <Input type="date" className="h-8 min-w-0 px-1.5 text-xs text-foreground" value={to} min={from} max={today} onChange={(e) => setTo(e.target.value)} />
            </label>
          </div>
          <p className={cn("min-h-4 text-xs", problem ? "text-destructive" : "text-muted-foreground")} role={problem ? "alert" : undefined}>
            {problem ?? `${days} ${days === 1 ? "day" : "days"} selected`}
          </p>
          <Button
            size="sm"
            disabled={problem !== null}
            onClick={() => {
              onChange({ date_from: from, date_to: to === today ? null : `${to}T23:59:59` });
              setOpen(false);
            }}
          >
            Apply
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
