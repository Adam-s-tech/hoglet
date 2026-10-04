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
  const [from, setFrom] = useState(() => (/^\d{4}-/.test(value.date_from) ? value.date_from.slice(0, 10) : isoDay(new Date(Date.now() - 13 * 86_400_000))));
  const [to, setTo] = useState(() => (value.date_to && /^\d{4}-/.test(value.date_to) ? value.date_to.slice(0, 10) : today));

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<Button variant="outline" size={small ? "sm" : "default"} />} aria-label={`Date range: ${rangeLabel(value.date_from, value.date_to)}`}>
        <Icon name="calendar" size={14} />
        {rangeLabel(value.date_from, value.date_to)}
        <Icon name="chevronDown" size={12} className="text-muted-foreground" />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 gap-0 p-1">
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
        <div className="flex flex-col gap-1.5 px-2 pt-1.5 pb-2">
          <span className="text-xs font-semibold text-muted-foreground">Custom range</span>
          <div className="flex items-center gap-1">
            <Input type="date" className="h-7 min-w-0 flex-1 px-1.5 text-xs" value={from} max={to} onChange={(e) => setFrom(e.target.value)} aria-label="From date" />
            <span className="text-muted-foreground">–</span>
            <Input type="date" className="h-7 min-w-0 flex-1 px-1.5 text-xs" value={to} min={from} max={today} onChange={(e) => setTo(e.target.value)} aria-label="To date" />
          </div>
          <Button
            size="sm"
            disabled={!from || !to || from > to}
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
