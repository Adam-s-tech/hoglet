// Catalog-backed pickers: events, properties, values, filters, breakdowns.
// Search lists are Base UI comboboxes (arrows / Enter / Esc), fed by the
// debounced catalog queries; popovers return focus to their trigger on Esc.

import { useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Combobox as ComboboxPrimitive } from "@base-ui/react/combobox";
import { useQuery } from "@tanstack/react-query";
import type { Breakdown } from "@/types/Breakdown";
import type { CatalogProperty } from "@/types/CatalogProperty";
import type { PropertyFilter } from "@/types/PropertyFilter";
import type { PropertyOperator } from "@/types/PropertyOperator";
import type { PropertySource } from "@/types/PropertySource";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxChip,
  ComboboxChips,
  ComboboxChipsInput,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
  ComboboxValue,
} from "@/components/ui/combobox";
import { InputGroupAddon } from "@/components/ui/input-group";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Icon } from "@/components/icons";
import { ErrorState } from "@/components/feedback";
import { Seg } from "@/components/controls";
import { useProjectId } from "@/lib/context";
import { fmtCompact } from "@/lib/format";
import { useDebounced } from "@/lib/hooks";
import { catalogEventsQuery, catalogPropertiesQuery, catalogValuesQuery } from "@/lib/queries";
import { OPERATORS, completeFilters, describeFilter, eventLabel, operatorInfo, propertyLabel } from "@/lib/properties";
import { cn } from "@/lib/utils";

// ── Select with a typed option list ──────────────────────────────────────

export interface Option<T extends string | number> {
  value: T;
  label: ReactNode;
  disabled?: boolean;
  /** Options sharing a group name render under one heading. */
  group?: string;
}

/** shadcn Select over a plain option array; the trigger shows the selected option's label. */
export function OptionSelect<T extends string | number>({
  value,
  options,
  onChange,
  label,
  className,
  size,
  placeholder,
}: {
  value: T;
  options: Option<T>[];
  onChange: (v: T) => void;
  /** Accessible name (there is no visible label). */
  label: string;
  className?: string;
  size?: "sm" | "default";
  placeholder?: string;
}) {
  const grouped = options.some((o) => o.group);
  const groups = grouped ? [...new Set(options.map((o) => o.group ?? ""))] : [""];
  const item = (o: Option<T>) => (
    <SelectItem key={String(o.value)} value={o.value} disabled={o.disabled}>
      {o.label}
    </SelectItem>
  );
  return (
    <Select
      value={value}
      items={options.map((o) => ({ value: o.value, label: o.label }))}
      onValueChange={(v) => {
        if (v !== null) onChange(v as T);
      }}
    >
      <SelectTrigger aria-label={label} size={size} className={className}>
        <SelectValue placeholder={placeholder} />
      </SelectTrigger>
      <SelectContent alignItemWithTrigger={false} align="start" className="min-w-40 w-auto">
        {grouped
          ? groups.map((g) => (
              <SelectGroup key={g}>
                {g ? <SelectLabel>{g}</SelectLabel> : null}
                {options.filter((o) => (o.group ?? "") === g).map(item)}
              </SelectGroup>
            ))
          : options.map(item)}
      </SelectContent>
    </Select>
  );
}

// ── Searchable list (inline combobox) ────────────────────────────────────

interface PickerItem {
  value: string;
  label: string;
  /** Raw key shown under a friendly label. */
  sub?: string;
  /** Right-aligned detail: a count or a type. */
  meta?: string;
  custom?: boolean;
}

const sameItem = (a: PickerItem, b: PickerItem) => a.value === b.value;
const ALL_EVENTS = "\u0000all";

function ItemRow({ item }: { item: PickerItem }) {
  return (
    <>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate">{item.label}</span>
        {item.sub ? <span className="truncate font-mono text-xs text-muted-foreground">{item.sub}</span> : null}
      </span>
      {item.meta !== undefined ? <span className="num ml-auto flex-none text-xs text-muted-foreground">{item.meta}</span> : null}
    </>
  );
}

function PickerList({
  items,
  selected,
  search,
  onSearch,
  onPick,
  placeholder,
  loading,
  error,
  header,
  allowFreeText,
  inputRef,
}: {
  items: PickerItem[];
  selected: PickerItem | null;
  search: string;
  onSearch: (s: string) => void;
  onPick: (value: string) => void;
  placeholder: string;
  loading?: boolean;
  error?: unknown;
  header?: ReactNode;
  allowFreeText?: boolean;
  inputRef?: React.Ref<HTMLInputElement>;
}) {
  const typed = search.trim();
  const showFree = !!allowFreeText && typed !== "" && !items.some((i) => i.value === typed);
  const all: PickerItem[] = showFree ? [...items, { value: typed, label: `Use “${typed}”`, meta: "custom", custom: true }] : items;
  return (
    <Combobox<PickerItem>
      inline
      open
      items={all}
      filter={null}
      autoHighlight
      value={selected}
      isItemEqualToValue={sameItem}
      onValueChange={(item) => {
        if (item) onPick(item.value);
      }}
      inputValue={search}
      onInputValueChange={(v, details) => {
        if (details.reason === "input-change" || details.reason === "input-clear") onSearch(v);
      }}
    >
      <div className="flex flex-col gap-1">
        {header}
        <ComboboxInput ref={inputRef} showTrigger={false} autoFocus placeholder={placeholder} aria-label={placeholder} className="mx-1 mt-1 w-auto">
          <InputGroupAddon>
            <Icon name="search" size={14} />
          </InputGroupAddon>
        </ComboboxInput>
        {error ? (
          <div className="p-1">
            <ErrorState error={error} compact />
          </div>
        ) : null}
        <ComboboxList className="max-h-72">
          {(item: PickerItem) => (
            <ComboboxItem key={item.value} value={item}>
              <ItemRow item={item} />
            </ComboboxItem>
          )}
        </ComboboxList>
        {!loading && !error && all.length === 0 ? <div className="px-3 py-2.5 text-sm text-muted-foreground">No matches.</div> : null}
        {loading && all.length === 0 ? <div className="px-3 py-2.5 text-sm text-muted-foreground">Loading…</div> : null}
      </div>
    </Combobox>
  );
}

/** Popover shell shared by the picker buttons: ArrowDown opens, Esc closes and returns focus to the button. */
function PickerPopover({ trigger, children, label, width = "w-80" }: { trigger: ReactNode; children: (close: () => void) => ReactNode; label: string; width?: string }) {
  const [open, setOpen] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" && !open) {
      e.preventDefault();
      setOpen(true);
    }
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<Button variant="outline" className="w-full min-w-0 justify-start bg-transparent font-normal" />} aria-label={label} onKeyDown={onKeyDown}>
        {trigger}
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className={cn(width, "max-w-[calc(100vw-1rem)] gap-0 p-0")}
        initialFocus={() => body.current?.querySelector<HTMLElement>("input") ?? true}
      >
        <div ref={body}>{children(() => setOpen(false))}</div>
      </PopoverContent>
    </Popover>
  );
}

// ── Event picker ─────────────────────────────────────────────────────────

export function EventPickerList({ value, onPick, allowAll = true }: { value: string | null; onPick: (event: string | null) => void; allowAll?: boolean }) {
  const projectId = useProjectId();
  const [search, setSearch] = useState("");
  const q = useDebounced(search.trim(), 150);
  const { data, error, isPending } = useQuery(catalogEventsQuery(projectId, q));
  const live = search.trim().toLowerCase();
  const items: PickerItem[] = [];
  if (allowAll && !live) items.push({ value: ALL_EVENTS, label: "All events" });
  for (const e of data ?? []) {
    const friendly = eventLabel(e.name);
    if (live && !e.name.toLowerCase().includes(live) && !friendly.toLowerCase().includes(live)) continue;
    items.push({ value: e.name, label: friendly, sub: friendly !== e.name ? e.name : undefined, meta: e.count ? fmtCompact(e.count) : undefined });
  }
  const selected: PickerItem | null = value === null ? (allowAll ? { value: ALL_EVENTS, label: "All events" } : null) : { value, label: eventLabel(value) };
  return (
    <PickerList
      items={items}
      selected={selected}
      search={search}
      onSearch={setSearch}
      onPick={(k) => onPick(k === ALL_EVENTS ? null : k)}
      placeholder="Search events…"
      loading={isPending}
      error={error}
      allowFreeText
    />
  );
}

export function EventPicker({ value, onChange, allowAll = true, placeholder = "Select an event" }: { value: string | null | undefined; onChange: (e: string | null) => void; allowAll?: boolean; placeholder?: string }) {
  return (
    <PickerPopover
      label={value === undefined ? placeholder : `Event: ${eventLabel(value)}`}
      trigger={
        <>
          <Icon name="bolt" size={14} className="flex-none text-muted-foreground" />
          <span className={cn("truncate", value === undefined && "text-muted-foreground")}>{value === undefined ? placeholder : eventLabel(value)}</span>
          <Icon name="chevronDown" size={12} className="ml-auto flex-none text-muted-foreground" />
        </>
      }
    >
      {(close) => (
        <EventPickerList
          value={value ?? null}
          allowAll={allowAll}
          onPick={(e) => {
            onChange(e);
            close();
          }}
        />
      )}
    </PickerPopover>
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
  const input = useRef<HTMLInputElement>(null);
  const [source, setSource] = useState<PropertySource>(sources[0]);
  const [search, setSearch] = useState("");
  const q = useDebounced(search.trim(), 150);
  const { data, error, isPending } = useQuery({
    ...catalogPropertiesQuery(projectId, source, q),
    // Keep the previous results while typing, but never show the other source's list.
    placeholderData: (prev, prevQuery) => (prevQuery?.queryKey[4] === source ? prev : undefined),
  });
  const live = search.trim().toLowerCase();
  const items: PickerItem[] = (data ?? [])
    .filter((p: CatalogProperty) => !numericOnly || p.property_type === "number")
    .filter((p) => !live || p.key.toLowerCase().includes(live) || propertyLabel(p.key).toLowerCase().includes(live))
    .slice(0, 200)
    .map((p) => ({ value: p.key, label: propertyLabel(p.key), sub: propertyLabel(p.key) !== p.key ? p.key : undefined, meta: p.property_type }));
  return (
    <PickerList
      inputRef={input}
      header={
        sources.length > 1 ? (
          <div className="px-1 pt-1">
            <Seg
              label="Property type"
              value={source}
              onChange={(s) => {
                setSource(s);
                input.current?.focus();
              }}
              className="w-full"
              options={sources.map((s) => ({ value: s, label: s === "event" ? "Event properties" : "Person properties" }))}
            />
          </div>
        ) : undefined
      }
      items={items}
      selected={selected ? { value: selected, label: propertyLabel(selected) } : null}
      search={search}
      onSearch={setSearch}
      onPick={(k) => onPick(k, source)}
      placeholder={numericOnly ? "Search numeric properties…" : "Search properties…"}
      loading={isPending}
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
  return (
    <PickerPopover
      label={value ? `Property: ${propertyLabel(value)}` : placeholder}
      trigger={
        <>
          <span className={cn("truncate", !value && "text-muted-foreground")}>{value ? propertyLabel(value) : placeholder}</span>
          <Icon name="chevronDown" size={12} className="ml-auto flex-none text-muted-foreground" />
        </>
      }
    >
      {(close) => (
        <PropertyPickerList
          numericOnly={numericOnly}
          sources={sources}
          selected={value ?? undefined}
          onPick={(k, s) => {
            onChange(k, s);
            close();
          }}
        />
      )}
    </PickerPopover>
  );
}

// ── Value input with catalog suggestions ─────────────────────────────────

type Scalar = string | number | boolean;

function ValueEditor({ filter, onChange }: { filter: PropertyFilter; onChange: (v: PropertyFilter["value"]) => void }) {
  const projectId = useProjectId();
  const info = operatorInfo(filter.operator);
  const [text, setText] = useState("");
  const q = useDebounced(text.trim(), 150);
  const { data } = useQuery({ ...catalogValuesQuery(projectId, filter.key, filter.type, q), enabled: info.multi });
  if (!info.needsValue) return null;
  if (!info.multi) {
    const isDate = filter.operator === "is_date_before" || filter.operator === "is_date_after";
    return (
      <Input
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
  const taken = new Set(values.map(String));
  const typed = text.trim();
  const items: PickerItem[] = [];
  if (typed && !taken.has(typed)) items.push({ value: typed, label: `Add “${typed}”`, custom: true });
  for (const s of data ?? []) {
    if (!s.value || taken.has(s.value) || s.value === typed) continue;
    if (items.length >= 9) break;
    items.push({ value: s.value, label: s.value, meta: s.count > 0 ? fmtCompact(s.count) : undefined });
  }
  const selected: PickerItem[] = values.map((v) => ({ value: String(v), label: String(v) }));
  return (
    <Combobox<PickerItem, true>
      multiple
      inline
      open
      items={items}
      filter={null}
      autoHighlight
      value={selected}
      isItemEqualToValue={sameItem}
      onValueChange={(next) => {
        onChange(next.map((item) => values.find((v) => String(v) === item.value) ?? item.value));
        setText("");
      }}
      inputValue={text}
      onInputValueChange={(v, details) => {
        if (details.reason === "input-change" || details.reason === "input-clear") setText(v);
      }}
    >
      <div className="flex flex-col gap-1.5">
        <ComboboxChips className="max-h-28 overflow-y-auto">
          <ComboboxValue>
            {(chips: PickerItem[]) => (
              <>
                {chips.map((c) => (
                  <ComboboxChip key={c.value} showRemove={false} className="max-w-full">
                    <span className="truncate">{c.label}</span>
                    <ComboboxPrimitive.ChipRemove
                      aria-label={`Remove ${c.label}`}
                      data-slot="combobox-chip-remove"
                      render={<Button variant="ghost" size="icon-xs" className="-mr-0.5 size-5 opacity-60 hover:opacity-100" />}
                    >
                      <Icon name="x" size={10} />
                    </ComboboxPrimitive.ChipRemove>
                  </ComboboxChip>
                ))}
                <ComboboxChipsInput
                  autoFocus
                  aria-label="Value"
                  placeholder={chips.length ? "Add another value (any of)…" : "Type a value and press Enter"}
                />
              </>
            )}
          </ComboboxValue>
        </ComboboxChips>
        <ComboboxList className="max-h-44 rounded-md border p-1 data-empty:hidden">
          {(item: PickerItem) => (
            <ComboboxItem key={item.value} value={item}>
              <ItemRow item={item} />
            </ComboboxItem>
          )}
        </ComboboxList>
      </div>
    </Combobox>
  );
}

// ── Filter chips + editor ────────────────────────────────────────────────

function FilterEditor({ filter, onChange, onRemove, onDone, sources }: { filter: PropertyFilter; onChange: (f: PropertyFilter) => void; onRemove: () => void; onDone: () => void; sources: PropertySource[] }) {
  if (!filter.key) {
    return <PropertyPickerList sources={sources} onPick={(key, source) => onChange({ ...filter, key, type: source })} />;
  }
  return (
    <div className="flex flex-col gap-2.5 p-2.5">
      <div className="flex items-center gap-2">
        <Badge variant="secondary">{filter.type === "person" ? "Person" : "Event"}</Badge>
        <b className="min-w-0 flex-1 truncate">{propertyLabel(filter.key)}</b>
        <Button variant="ghost" size="sm" onClick={() => onChange({ ...filter, key: "" })}>
          Change
        </Button>
      </div>
      <OptionSelect<PropertyOperator>
        label="Operator"
        className="w-full"
        value={filter.operator}
        options={OPERATORS.map((o) => ({ value: o.value, label: o.label }))}
        onChange={(op) => {
          const next = operatorInfo(op);
          const prev = operatorInfo(filter.operator);
          let value = filter.value;
          if (!next.needsValue) value = null;
          else if (next.multi && !prev.multi) value = value === null || value === "" ? [] : Array.isArray(value) ? value : [value];
          else if (!next.multi && Array.isArray(value)) value = value[0] ?? "";
          onChange({ ...filter, operator: op, value });
        }}
      />
      <ValueEditor filter={filter} onChange={(value) => onChange({ ...filter, value })} />
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" onClick={onRemove}>
          <Icon name="trash" size={13} /> Remove
        </Button>
        <span className="flex-1" />
        <Button size="sm" onClick={onDone}>
          Done
        </Button>
      </div>
    </div>
  );
}

function FilterChip({ filter, onChange, onRemove, sources, initiallyOpen }: { filter: PropertyFilter; onChange: (f: PropertyFilter) => void; onRemove: () => void; sources: PropertySource[]; initiallyOpen?: boolean }) {
  const [open, setOpen] = useState(!!initiallyOpen);
  const d = describeFilter(filter);
  const summary = `${d.key} ${d.op} ${d.value}`.trim();
  const close = () => {
    setOpen(false);
    if (!filter.key) onRemove();
  };
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (next) setOpen(true);
        else close();
      }}
    >
      <span className="inline-flex max-w-full items-center rounded-lg border border-input bg-muted/50">
        <PopoverTrigger
          render={<Button variant="ghost" size="sm" className="min-w-0 rounded-r-none px-2 font-normal" />}
          title={summary}
          aria-label={filter.key ? `Edit filter: ${summary}` : "Edit new filter"}
        >
          {filter.key ? (
            <span className="truncate">
              <b>{d.key}</b> {d.op} {d.value ? <b>{d.value}</b> : null}
            </span>
          ) : (
            <span className="text-muted-foreground">New filter</span>
          )}
        </PopoverTrigger>
        <Button variant="ghost" size="icon-xs" className="mr-0.5 text-muted-foreground" aria-label={filter.key ? `Remove filter: ${summary}` : "Remove filter"} onClick={onRemove}>
          <Icon name="x" size={12} />
        </Button>
      </span>
      <PopoverContent align="start" className="w-80 max-w-[calc(100vw-1rem)] gap-0 p-0" initialFocus={false}>
        <FilterEditor filter={filter} sources={sources} onChange={onChange} onRemove={onRemove} onDone={close} />
      </PopoverContent>
    </Popover>
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
    <div className="flex flex-wrap items-center gap-1.5">
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
      <Button
        variant="outline"
        size="sm"
        className="border-dashed text-muted-foreground"
        onClick={() => {
          setFresh(value.length);
          onChange([...value, { key: "", type: sources[0], operator: "exact", value: [] }]);
        }}
      >
        <Icon name="plus" size={12} /> {addLabel}
      </Button>
    </div>
  );
}

export { completeFilters };

// ── Breakdown ────────────────────────────────────────────────────────────

export function BreakdownPicker({ value, onChange }: { value: Breakdown | null; onChange: (b: Breakdown | null) => void }) {
  return (
    <div className="flex items-center gap-1.5">
      <div className="min-w-0 flex-1">
        <PropertyPicker
          value={value?.property}
          placeholder="Add breakdown"
          onChange={(property, source) => onChange({ property, type: source, limit: value?.limit ?? 10 })}
        />
      </div>
      {value && (
        <>
          <OptionSelect
            label="Top values"
            className="w-24"
            value={value.limit}
            options={[5, 10, 25, 50].map((n) => ({ value: n, label: `Top ${n}` }))}
            onChange={(limit) => onChange({ ...value, limit })}
          />
          <Button variant="ghost" size="icon" onClick={() => onChange(null)} aria-label="Remove breakdown">
            <Icon name="x" />
          </Button>
        </>
      )}
    </div>
  );
}
