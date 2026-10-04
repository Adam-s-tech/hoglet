import { useRef, useState } from "react";
import { RANGE_PRESETS, rangeLabel } from "../lib/format";
import { Icon } from "./icons";
import { Popover } from "./kit";

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
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const today = isoDay(new Date());
  const [from, setFrom] = useState(() => (/^\d{4}-/.test(value.date_from) ? value.date_from.slice(0, 10) : isoDay(new Date(Date.now() - 13 * 86_400_000))));
  const [to, setTo] = useState(() => (value.date_to && /^\d{4}-/.test(value.date_to) ? value.date_to.slice(0, 10) : today));

  return (
    <>
      <button ref={ref} className={`btn${small ? " small" : ""}`} onClick={() => setOpen((o) => !o)} aria-haspopup="dialog" aria-expanded={open}>
        <Icon name="calendar" size={14} />
        {rangeLabel(value.date_from, value.date_to)}
        <Icon name="chevronDown" size={12} />
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)} width={240}>
        {presets.map((p) => {
          const selected = p.date_from === value.date_from && (p.date_to ?? null) === (value.date_to ?? null);
          return (
            <button
              key={p.label}
              className="menu-item"
              aria-selected={selected}
              onClick={() => {
                onChange({ date_from: p.date_from, date_to: p.date_to });
                setOpen(false);
              }}
            >
              <span style={{ width: 16, display: "inline-grid" }}>{selected && <Icon name="check" size={14} strokeWidth={2.2} />}</span>
              {p.label}
              <span className="meta">{p.short}</span>
            </button>
          );
        })}
        <div className="menu-sep" />
        <div className="col" style={{ padding: "6px 8px 8px", gap: 6 }}>
          <span className="label">Custom range</span>
          <div className="row gap-4">
            <input className="input small grow" type="date" value={from} max={to} onChange={(e) => setFrom(e.target.value)} aria-label="From date" />
            <span className="muted">–</span>
            <input className="input small grow" type="date" value={to} min={from} max={today} onChange={(e) => setTo(e.target.value)} aria-label="To date" />
          </div>
          <button
            className="btn small primary"
            disabled={!from || !to || from > to}
            onClick={() => {
              onChange({ date_from: from, date_to: to === today ? null : `${to}T23:59:59` });
              setOpen(false);
            }}
          >
            Apply
          </button>
        </div>
      </Popover>
    </>
  );
}
