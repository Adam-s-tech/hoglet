// Catalog-backed pickers: events, properties, values, filters, breakdowns.

import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Breakdown } from "../types/Breakdown";
import type { CatalogProperty } from "../types/CatalogProperty";
import type { PropertyFilter } from "../types/PropertyFilter";
import type { PropertyOperator } from "../types/PropertyOperator";
import type { PropertySource } from "../types/PropertySource";
import { api } from "../lib/api";
import { useProjectId } from "../lib/context";
import { fmtCompact } from "../lib/format";
import { useApi, useDebounced } from "../lib/hooks";
import { OPERATORS, describeFilter, eventLabel, operatorInfo, propertyLabel } from "../lib/properties";
import { Icon } from "../ui/icons";
import { ErrorState, Popover } from "../ui/kit";

// ── Searchable list with keyboard navigation ──────────────────────────────

interface ListItem {
  key: string;
  label: ReactNode;
  meta?: ReactNode;
  sub?: ReactNode;
  selected?: boolean;
}

function SearchList({
  items,
  search,
  setSearch,
  onPick,
  placeholder,
  loading,
  error,
  footer,
  header,
  allowFreeText,
}: {
  items: ListItem[];
  search: string;
  setSearch: (s: string) => void;
  onPick: (key: string) => void;
  placeholder: string;
  loading?: boolean;
  error?: unknown;
  footer?: ReactNode;
  header?: ReactNode;
  allowFreeText?: boolean;
}) {
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const showFree = allowFreeText && search.trim() && !items.some((i) => i.key === search.trim());
  const all: ListItem[] = showFree ? [...items, { key: search.trim(), label: <>Use “{search.trim()}”</>, meta: "custom" }] : items;
  useEffect(() => setActive(0), [search]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(all.length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = all[active];
      if (item) onPick(item.key);
    }
  };
  return (
    <div className="col" style={{ gap: 4, width: 320 }}>
      {header}
      <div className="search" style={{ padding: 4 }}>
        <Icon name="search" size={14} style={{ left: 13 }} />
        <input className="input" autoFocus placeholder={placeholder} value={search} onChange={(e) => setSearch(e.target.value)} onKeyDown={onKey} aria-label={placeholder} />
      </div>
      <div ref={listRef} style={{ maxHeight: 300, overflowY: "auto" }} role="listbox">
        {error ? (
          <div style={{ padding: 8 }}>
            <ErrorState error={error} compact />
          </div>
        ) : null}
        {all.map((item, i) => (
          <button
            key={item.key}
            data-index={i}
            role="option"
            className="menu-item"
            data-active={i === active}
            aria-selected={item.selected}
            onPointerMove={() => setActive(i)}
            onClick={() => onPick(item.key)}
          >
            <span className="col" style={{ gap: 0, minWidth: 0 }}>
              <span className="truncate">{item.label}</span>
              {item.sub && <span className="muted small mono truncate">{item.sub}</span>}
            </span>
            {item.meta !== undefined && <span className="meta">{item.meta}</span>}
          </button>
        ))}
        {!loading && !error && all.length === 0 && <div className="muted small" style={{ padding: "10px 12px" }}>No matches.</div>}
        {loading && all.length === 0 && <div className="muted small" style={{ padding: "10px 12px" }}>Loading…</div>}
      </div>
      {footer}
    </div>
  );
}

// ── Event picker ─────────────────────────────────────────────────────────

export function EventPickerList({ value, onPick, allowAll = true }: { value: string | null; onPick: (event: string | null) => void; allowAll?: boolean }) {
  const projectId = useProjectId();
  const [search, setSearch] = useState("");
  const q = useDebounced(search.trim(), 150);
  const { data, error, loading } = useApi(`catalog-events:${projectId}:${q}`, (signal) => api.catalogEvents(projectId, q, signal));
  const items: ListItem[] = [];
  if (allowAll && !q) items.push({ key: "\u0000all", label: <b>All events</b>, selected: value === null });
  for (const e of data ?? []) {
    if (q && !e.name.toLowerCase().includes(q.toLowerCase()) && !eventLabel(e.name).toLowerCase().includes(q.toLowerCase())) continue;
    items.push({
      key: e.name,
      label: eventLabel(e.name),
      sub: eventLabel(e.name) !== e.name ? e.name : undefined,
      meta: e.count ? fmtCompact(e.count) : undefined,
      selected: e.name === value,
    });
  }
  return (
    <SearchList
      items={items}
      search={search}
      setSearch={setSearch}
      onPick={(k) => onPick(k === "\u0000all" ? null : k)}
      placeholder="Search events…"
      loading={loading}
      error={error}
      allowFreeText
    />
  );
}

export function EventPicker({ value, onChange, allowAll = true, placeholder = "Select an event" }: { value: string | null | undefined; onChange: (e: string | null) => void; allowAll?: boolean; placeholder?: string }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button ref={ref} className="picker-btn" onClick={() => setOpen((o) => !o)} aria-haspopup="listbox" aria-expanded={open}>
        <Icon name="bolt" size={14} style={{ color: "var(--ink-3)", flex: "none" }} />
        {value === undefined ? <span className="ph">{placeholder}</span> : <span>{eventLabel(value)}</span>}
        <Icon name="chevronDown" size={12} style={{ marginLeft: "auto", flex: "none", color: "var(--ink-3)" }} />
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)}>
        <EventPickerList
          value={value ?? null}
          allowAll={allowAll}
          onPick={(e) => {
            onChange(e);
            setOpen(false);
          }}
        />
      </Popover>
    </>
  );
}

// ── Property picker ──────────────────────────────────────────────────────

export function PropertyPickerList({
  onPick,
  sources = ["event", "person"],
  numericOnly,
  selected,
}: {
  onPick: (key: string, source: PropertySource) => void;
  sources?: PropertySource[];
  numericOnly?: boolean;
  selected?: string;
}) {
  const projectId = useProjectId();
  const [source, setSource] = useState<PropertySource>(sources[0]);
  const [search, setSearch] = useState("");
  const { data, error, loading } = useApi(`catalog-props:${projectId}:${source}`, (signal) => api.catalogProperties(projectId, source, "", signal));
  const q = search.trim().toLowerCase();
  const items: ListItem[] = (data ?? [])
    .filter((p: CatalogProperty) => !numericOnly || p.property_type === "number")
    .filter((p) => !q || p.key.toLowerCase().includes(q) || propertyLabel(p.key).toLowerCase().includes(q))
    .slice(0, 200)
    .map((p) => ({
      key: p.key,
      label: propertyLabel(p.key),
      sub: propertyLabel(p.key) !== p.key ? p.key : undefined,
      meta: p.property_type,
      selected: p.key === selected,
    }));
  return (
    <SearchList
      header={
        sources.length > 1 ? (
          <div className="seg" style={{ margin: "4px 4px 0" }} role="group" aria-label="Property type">
            {sources.map((s) => (
              <button key={s} aria-pressed={source === s} onClick={() => setSource(s)}>
                {s === "event" ? "Event properties" : "Person properties"}
              </button>
            ))}
          </div>
        ) : undefined
      }
      items={items}
      search={search}
      setSearch={setSearch}
      onPick={(k) => onPick(k, source)}
      placeholder={numericOnly ? "Search numeric properties…" : "Search properties…"}
      loading={loading}
      error={error}
      allowFreeText
    />
  );
}

export function PropertyPicker({
  value,
  onChange,
  placeholder = "Select a property",
  numericOnly,
  sources,
}: {
  value: string | null | undefined;
  onChange: (key: string, source: PropertySource) => void;
  placeholder?: string;
  numericOnly?: boolean;
  sources?: PropertySource[];
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button ref={ref} className="picker-btn" onClick={() => setOpen((o) => !o)} aria-haspopup="listbox" aria-expanded={open}>
        {value ? <span>{propertyLabel(value)}</span> : <span className="ph">{placeholder}</span>}
        <Icon name="chevronDown" size={12} style={{ marginLeft: "auto", flex: "none", color: "var(--ink-3)" }} />
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)}>
        <PropertyPickerList
          numericOnly={numericOnly}
          sources={sources}
          selected={value ?? undefined}
          onPick={(k, s) => {
            onChange(k, s);
            setOpen(false);
          }}
        />
      </Popover>
    </>
  );
}

// ── Value input with catalog suggestions ─────────────────────────────────

type Scalar = string | number | boolean;

function ValueEditor({ filter, onChange }: { filter: PropertyFilter; onChange: (v: PropertyFilter["value"]) => void }) {
  const projectId = useProjectId();
  const info = operatorInfo(filter.operator);
  const [text, setText] = useState("");
  const q = useDebounced(text.trim(), 150);
  const { data } = useApi(info.multi ? `catalog-values:${projectId}:${filter.type}:${filter.key}:${q}` : null, (signal) =>
    api.catalogValues(projectId, filter.key, filter.type, q, signal),
  );
  if (!info.needsValue) return null;
  if (!info.multi) {
    const isDate = filter.operator === "is_date_before" || filter.operator === "is_date_after";
    return (
      <input
        className="input"
        type={info.numeric ? "number" : isDate ? "date" : "text"}
        value={filter.value === null || Array.isArray(filter.value) ? "" : String(filter.value)}
        placeholder={info.numeric ? "Number" : "Value"}
        onChange={(e) => onChange(info.numeric ? (e.target.value === "" ? null : Number(e.target.value)) : e.target.value)}
        aria-label="Value"
        autoFocus
      />
    );
  }
  const values: Scalar[] = Array.isArray(filter.value) ? filter.value : filter.value === null || filter.value === "" ? [] : [filter.value];
  const add = (v: string) => {
    if (!v.trim() || values.map(String).includes(v)) return;
    onChange([...values, v]);
    setText("");
  };
  const suggestions = (data ?? []).filter((s) => !values.map(String).includes(s.value)).slice(0, 8);
  return (
    <div className="col" style={{ gap: 6 }}>
      {values.length > 0 && (
        <div className="row wrap gap-4">
          {values.map((v) => (
            <span key={String(v)} className="chip" style={{ cursor: "default" }}>
              <b>{String(v)}</b>
              <span
                className="x"
                role="button"
                tabIndex={0}
                aria-label={`Remove ${String(v)}`}
                onClick={() => onChange(values.filter((x) => x !== v))}
                onKeyDown={(e) => e.key === "Enter" && onChange(values.filter((x) => x !== v))}
              >
                <Icon name="x" size={10} />
              </span>
            </span>
          ))}
        </div>
      )}
      <input
        className="input"
        value={text}
        autoFocus
        placeholder={values.length ? "Add another value (any of)…" : "Type a value and press Enter"}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            add(text.trim());
          }
        }}
        aria-label="Value"
      />
      {suggestions.length > 0 && (
        <div style={{ maxHeight: 180, overflowY: "auto" }}>
          {suggestions.map((s) => (
            <button key={s.value} className="menu-item" onClick={() => add(s.value)}>
              <span className="truncate">{s.value || <span className="muted">(empty)</span>}</span>
              {s.count > 0 && <span className="meta">{fmtCompact(s.count)}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Filter chips + editor ────────────────────────────────────────────────

function FilterEditor({ filter, onChange, onRemove, onDone, sources }: { filter: PropertyFilter; onChange: (f: PropertyFilter) => void; onRemove: () => void; onDone: () => void; sources: PropertySource[] }) {
  if (!filter.key) {
    return (
      <PropertyPickerList
        sources={sources}
        onPick={(key, source) => onChange({ ...filter, key, type: source })}
      />
    );
  }
  return (
    <div className="col" style={{ gap: 10, width: 320, padding: 8 }}>
      <div className="row">
        <span className="badge">{filter.type === "person" ? "Person" : "Event"}</span>
        <b className="truncate grow">{propertyLabel(filter.key)}</b>
        <button className="btn ghost small" onClick={() => onChange({ ...filter, key: "" })}>
          Change
        </button>
      </div>
      <select
        className="select"
        value={filter.operator}
        aria-label="Operator"
        onChange={(e) => {
          const op = e.target.value as PropertyOperator;
          const next = operatorInfo(op);
          const prev = operatorInfo(filter.operator);
          let value = filter.value;
          if (!next.needsValue) value = null;
          else if (next.multi && !prev.multi) value = value === null || value === "" ? [] : Array.isArray(value) ? value : [value];
          else if (!next.multi && Array.isArray(value)) value = value[0] ?? "";
          onChange({ ...filter, operator: op, value });
        }}
      >
        {OPERATORS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      <ValueEditor filter={filter} onChange={(value) => onChange({ ...filter, value })} />
      <div className="row">
        <button className="btn ghost small danger" onClick={onRemove}>
          <Icon name="trash" size={13} /> Remove
        </button>
        <span className="spacer" />
        <button className="btn small primary" onClick={onDone}>
          Done
        </button>
      </div>
    </div>
  );
}

function FilterChip({ filter, onChange, onRemove, sources, initiallyOpen }: { filter: PropertyFilter; onChange: (f: PropertyFilter) => void; onRemove: () => void; sources: PropertySource[]; initiallyOpen?: boolean }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(!!initiallyOpen);
  const d = describeFilter(filter);
  const close = () => {
    setOpen(false);
    if (!filter.key) onRemove();
  };
  return (
    <>
      <button ref={ref} className="chip" onClick={() => setOpen(true)} title={`${d.key} ${d.op} ${d.value}`}>
        {filter.key ? (
          <span className="truncate">
            <b>{d.key}</b> {d.op} {d.value && <b>{d.value}</b>}
          </span>
        ) : (
          <span className="muted">New filter</span>
        )}
        <span
          className="x"
          role="button"
          tabIndex={0}
          aria-label="Remove filter"
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.stopPropagation();
              onRemove();
            }
          }}
        >
          <Icon name="x" size={10} />
        </span>
      </button>
      <Popover anchor={ref} open={open} onClose={close}>
        <FilterEditor filter={filter} sources={sources} onChange={onChange} onRemove={onRemove} onDone={close} />
      </Popover>
    </>
  );
}

export function PropertyFilters({
  value,
  onChange,
  sources = ["event", "person"],
  addLabel = "Add filter",
}: {
  value: PropertyFilter[];
  onChange: (v: PropertyFilter[]) => void;
  sources?: PropertySource[];
  addLabel?: string;
}) {
  const [fresh, setFresh] = useState<number | null>(null);
  return (
    <div className="row wrap gap-4">
      {value.map((f, i) => (
        <FilterChip
          key={i}
          filter={f}
          sources={sources}
          initiallyOpen={fresh === i}
          onChange={(nf) => onChange(value.map((x, j) => (j === i ? nf : x)))}
          onRemove={() => {
            setFresh(null);
            onChange(value.filter((_, j) => j !== i));
          }}
        />
      ))}
      <button
        className="chip add"
        onClick={() => {
          setFresh(value.length);
          onChange([...value, { key: "", type: sources[0], operator: "exact", value: [] }]);
        }}
      >
        <Icon name="plus" size={12} /> {addLabel}
      </button>
    </div>
  );
}

/** Drop half-built filters before a query is sent. */
export function completeFilters(filters: PropertyFilter[]): PropertyFilter[] {
  return filters.filter((f) => {
    if (!f.key) return false;
    const info = operatorInfo(f.operator);
    if (!info.needsValue) return true;
    if (Array.isArray(f.value)) return f.value.length > 0;
    return f.value !== null && f.value !== "";
  });
}

// ── Breakdown ────────────────────────────────────────────────────────────

export function BreakdownPicker({ value, onChange }: { value: Breakdown | null; onChange: (b: Breakdown | null) => void }) {
  return (
    <div className="row">
      <PropertyPicker
        value={value?.property}
        placeholder="Add breakdown"
        onChange={(property, source) => onChange({ property, type: source, limit: value?.limit ?? 10 })}
      />
      {value && (
        <>
          <select className="select" style={{ width: 90 }} value={value.limit} onChange={(e) => onChange({ ...value, limit: Number(e.target.value) })} aria-label="Top values">
            {[5, 10, 25, 50].map((n) => (
              <option key={n} value={n}>
                Top {n}
              </option>
            ))}
          </select>
          <button className="btn ghost icon" onClick={() => onChange(null)} aria-label="Remove breakdown">
            <Icon name="x" />
          </button>
        </>
      )}
    </div>
  );
}
