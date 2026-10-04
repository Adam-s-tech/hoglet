import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi, useBlocker } from "@tanstack/react-router";
import { Slider as SliderPrimitive } from "@base-ui/react/slider";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Icon } from "@/components/icons";
import { AppDialog, Confirm } from "@/components/dialogs";
import { DataTable, columnHelper } from "@/components/data-table";
import { Empty, ErrorState, Notice, Skeleton, SkeletonRows } from "@/components/feedback";
import { CardBar, CardPad, FormField, Page, PageHeader, Panel, SearchInput } from "@/components/page";
import { JsonView, Snippet } from "@/components/copy";
import { toast } from "@/components/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { ApiError, api, errorMessage } from "@/lib/api";
import { canEdit, useApp, usePath, useProjectId } from "@/lib/context";
import { fmtRelative } from "@/lib/format";
import { navigate } from "@/lib/nav";
import { describeFilter } from "@/lib/properties";
import { flagQuery, flagsQuery, qk } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { FeatureFlag } from "@/types/FeatureFlag";
import type { FeatureFlagInput } from "@/types/FeatureFlagInput";
import type { FlagConditionGroup } from "@/types/FlagConditionGroup";
import type { FlagEvaluation } from "@/types/FlagEvaluation";
import type { FlagFilters } from "@/types/FlagFilters";
import type { FlagVariant } from "@/types/FlagVariant";
import { PropertyFilters, completeFilters } from "@/insight/pickers";

const flagRouteApi = getRouteApi("/project/$projectId/flags/$id");

/** Categorical slot per variant, same palette as the charts; beyond eight, neutral. */
const variantColor = (i: number) => (i >= 0 && i < 8 ? `var(--s${i + 1})` : "var(--muted-foreground)");

function rolloutSummary(f: FeatureFlag): string {
  const groups = f.filters.groups;
  const mv = f.filters.multivariate?.variants.length ?? 0;
  if (groups.length === 0) return "No release conditions";
  let base: string;
  if (groups.length === 1) {
    const g = groups[0];
    const pct = g.rollout_percentage ?? 100;
    base = g.properties.length ? `${pct}% of matching persons` : `${pct}% of all persons`;
  } else base = `${groups.length} condition sets`;
  return mv ? `${base} · ${mv} variants` : base;
}

// ── List ─────────────────────────────────────────────────────────────────

const flagCol = columnHelper<FeatureFlag>();

export function FlagsPage() {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const queryClient = useQueryClient();
  const { data, error, isPending, refetch } = useQuery(flagsQuery(projectId));
  const [search, setSearch] = useState("");

  const toggle = useMutation({
    mutationFn: ({ flag, active }: { flag: FeatureFlag; active: boolean }) => api.updateFlag(projectId, flag.id, { active }),
    onMutate: async ({ flag, active }) => {
      await queryClient.cancelQueries({ queryKey: qk.flags(projectId), exact: true });
      const previous = queryClient.getQueryData<FeatureFlag[]>(qk.flags(projectId));
      queryClient.setQueryData<FeatureFlag[]>(qk.flags(projectId), (list) => list?.map((f) => (f.id === flag.id ? { ...f, active } : f)));
      return { previous };
    },
    onError: (e, _vars, ctx) => {
      if (ctx?.previous) queryClient.setQueryData(qk.flags(projectId), ctx.previous);
      toast(errorMessage(e), true);
    },
    onSuccess: (_saved, { flag, active }) => toast(`${flag.key} ${active ? "enabled" : "disabled"}`),
    // List and every flag detail share the qk.flags prefix.
    onSettled: () => queryClient.invalidateQueries({ queryKey: qk.flags(projectId) }),
  });
  const mutateToggle = toggle.mutate;

  const columns = useMemo(
    () => [
      flagCol.accessor("key", {
        header: "Flag",
        cell: (c) => (
          <Link
            to={path(`flags/${c.row.original.id}`)}
            className="font-mono font-semibold text-foreground hover:text-brand-foreground hover:underline"
            onClick={(e) => e.stopPropagation()}
          >
            {c.getValue()}
          </Link>
        ),
      }),
      flagCol.accessor("name", {
        header: "Description",
        cell: (c) => <span className="line-clamp-1 max-w-[420px] text-muted-foreground">{c.getValue() || "–"}</span>,
        meta: { className: "hidden md:table-cell" },
      }),
      flagCol.accessor((f) => rolloutSummary(f), {
        id: "rollout",
        header: "Release",
        cell: (c) => <span className="text-muted-foreground">{c.getValue()}</span>,
      }),
      flagCol.accessor("updated_at", {
        header: "Updated",
        cell: (c) => <span className="whitespace-nowrap text-muted-foreground">{fmtRelative(c.getValue())}</span>,
        meta: { className: "hidden sm:table-cell" },
      }),
      flagCol.accessor("active", {
        header: "Enabled",
        cell: (c) => {
          const f = c.row.original;
          return (
            <span onClick={(e) => e.stopPropagation()} className="flex h-5 items-center justify-end">
              <Switch checked={f.active} onCheckedChange={(v) => mutateToggle({ flag: f, active: v })} aria-label={`Enable ${f.key}`} disabled={!editable} />
            </span>
          );
        },
        meta: { align: "right", className: "w-24" },
        sortFn: "basic",
      }),
    ],
    [path, editable, mutateToggle],
  );

  const q = search.trim().toLowerCase();
  const list = useMemo(() => (data ?? []).filter((f) => !q || `${f.key} ${f.name}`.toLowerCase().includes(q)), [data, q]);
  const newFlag = editable ? (
    <Button nativeButton={false} render={<Link to={path("flags/new")} />}>
      <Icon name="plus" size={14} /> New flag
    </Button>
  ) : undefined;

  return (
    <Page>
      <PageHeader title="Feature flags" sub="Roll features out safely. PostHog SDKs evaluate these locally or via /flags." actions={newFlag} />
      <Panel>
        <CardBar>
          <SearchInput wrapperClassName="w-full sm:w-72" placeholder="Search flags…" value={search} onChange={(e) => setSearch(e.target.value)} aria-label="Search flags" />
          <span className="flex-1" />
          {data && (
            <span className="num text-xs text-muted-foreground">
              {data.filter((f) => f.active).length} active of {data.length}
            </span>
          )}
        </CardBar>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} />
        ) : isPending ? (
          <SkeletonRows rows={5} />
        ) : data && data.length === 0 ? (
          <Empty
            icon="flag"
            title="No feature flags yet"
            action={editable ? <Button nativeButton={false} render={<Link to={path("flags/new")} />}>Create your first flag</Button> : undefined}
          >
            Create a flag, then gate code with <code>posthog.isFeatureEnabled('my-flag')</code>. Flags evaluate in-process here; no extra service.
          </Empty>
        ) : list.length === 0 ? (
          <Empty icon="search" title="No flags match" />
        ) : (
          <DataTable label="Feature flags" columns={columns} data={list} getRowId={(f) => String(f.id)} sortable onRowClick={(f) => navigate(path(`flags/${f.id}`))} />
        )}
      </Panel>
    </Page>
  );
}

// ── Editor pieces ────────────────────────────────────────────────────────

const EMPTY: FeatureFlagInput = {
  key: "",
  name: "",
  active: true,
  filters: { groups: [{ properties: [], rollout_percentage: 100, variant: null }], multivariate: null, payloads: {} },
  ensure_experience_continuity: false,
};

function toInput(f: FeatureFlag): FeatureFlagInput {
  return { key: f.key, name: f.name, active: f.active, filters: f.filters, ensure_experience_continuity: f.ensure_experience_continuity };
}

const KEY_RE = /^[a-zA-Z0-9_-]+$/;
const near100 = (n: number) => Math.abs(n - 100) < 0.01;

function validateKey(key: string): string | undefined {
  if (!key) return "Key is required.";
  return KEY_RE.test(key) ? undefined : "Use letters, numbers, - and _ only.";
}

function validateFilters(f: FlagFilters): string | undefined {
  const mv = f.multivariate;
  if (!mv) return undefined;
  if (mv.variants.length < 2) return "Multiple variants need at least two variants.";
  if (mv.variants.some((v) => !v.key.trim())) return "Every variant needs a key.";
  const sum = mv.variants.reduce((a, v) => a + v.rollout_percentage, 0);
  return near100(sum) ? undefined : `Variant splits sum to ${sum}%, need 100%.`;
}

function Section({ title, sub, aside, children }: { title: string; sub?: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <Panel>
      <CardBar>
        <div className="flex min-w-0 flex-1 flex-col">
          <h3>{title}</h3>
          {sub && <span className="text-xs text-muted-foreground">{sub}</span>}
        </div>
        {aside}
      </CardBar>
      <CardPad>{children}</CardPad>
    </Panel>
  );
}

function Pct({ value, onChange, label, disabled, className }: { value: number; onChange: (v: number) => void; label: string; disabled?: boolean; className?: string }) {
  return (
    <div className={cn("relative w-[84px] flex-none", className)}>
      <Input
        type="number"
        inputMode="numeric"
        min={0}
        max={100}
        step={1}
        value={Number.isFinite(value) ? value : 0}
        disabled={disabled}
        onChange={(e) => onChange(Math.max(0, Math.min(100, Number(e.target.value) || 0)))}
        aria-label={label}
        className="num pr-6"
      />
      <span aria-hidden="true" className="pointer-events-none absolute top-1/2 right-2.5 -translate-y-1/2 text-xs text-muted-foreground">
        %
      </span>
    </div>
  );
}

/** shadcn's Slider doesn't expose the thumb's accessible name, so this composes the same primitive. */
function RolloutSlider({ value, onChange, label, disabled }: { value: number; onChange: (v: number) => void; label: string; disabled?: boolean }) {
  return (
    <SliderPrimitive.Root
      value={value}
      min={0}
      max={100}
      step={1}
      disabled={disabled}
      thumbAlignment="edge"
      onValueChange={(v) => onChange(Array.isArray(v) ? v[0] : v)}
      className="min-w-24 flex-1"
    >
      <SliderPrimitive.Control className="relative flex h-6 w-full touch-none items-center select-none data-disabled:opacity-50">
        <SliderPrimitive.Track className="relative h-1.5 w-full grow overflow-hidden rounded-full bg-muted">
          <SliderPrimitive.Indicator className="h-full bg-primary" />
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb
          getAriaLabel={() => label}
          className="relative block size-4 shrink-0 rounded-full border border-ring bg-background ring-ring/50 transition-[color,box-shadow] select-none after:absolute after:-inset-2 hover:ring-3 has-focus-visible:ring-3 active:ring-3 data-disabled:pointer-events-none"
        />
      </SliderPrimitive.Control>
    </SliderPrimitive.Root>
  );
}

const SPLIT = "__split__";

function ConditionGroup({
  group,
  index,
  variants,
  readOnly,
  onChange,
  onRemove,
}: {
  group: FlagConditionGroup;
  index: number;
  variants: string[];
  readOnly: boolean;
  onChange: (g: FlagConditionGroup) => void;
  onRemove: (() => void) | null;
}) {
  const pct = group.rollout_percentage ?? 100;
  const props = completeFilters(group.properties);
  const n = index + 1;
  const variantItems = [{ value: SPLIT, label: "by the split above" }, ...variants.filter(Boolean).map((v) => ({ value: v, label: v }))];
  return (
    <div role="group" aria-label={`Condition set ${n}`} className="rounded-lg border bg-muted/30 p-3.5">
      <div className="mb-3 flex items-center gap-2">
        <b className="flex-1">Condition set {n}</b>
        {onRemove && !readOnly && (
          <Button type="button" variant="ghost" size="icon-sm" aria-label={`Remove condition set ${n}`} onClick={onRemove}>
            <Icon name="trash" size={13} />
          </Button>
        )}
      </div>
      <div className="flex flex-col gap-3.5">
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-semibold text-muted-foreground">Match persons where</span>
          <div inert={readOnly}>
            <PropertyFilters value={group.properties} onChange={(properties) => onChange({ ...group, properties })} sources={["person"]} addLabel="Add condition" />
          </div>
          {group.properties.length === 0 && <span className="text-xs text-muted-foreground">No conditions: every person matches.</span>}
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-semibold text-muted-foreground">Roll out to</span>
          <div className="flex items-center gap-3">
            <RolloutSlider value={pct} onChange={(v) => onChange({ ...group, rollout_percentage: v })} label={`Rollout percentage, condition set ${n}`} disabled={readOnly} />
            <Pct value={pct} onChange={(v) => onChange({ ...group, rollout_percentage: v })} label={`Rollout percentage for condition set ${n}`} disabled={readOnly} />
          </div>
        </div>
        {variants.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
            <span id={`serve-variant-${n}`}>Serve variant</span>
            <Select
              value={group.variant && variants.includes(group.variant) ? group.variant : SPLIT}
              items={variantItems}
              disabled={readOnly}
              onValueChange={(v) => onChange({ ...group, variant: v && v !== SPLIT ? v : null })}
            >
              <SelectTrigger size="sm" className="min-w-44" aria-labelledby={`serve-variant-${n}`}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {variantItems.map((o) => (
                  <SelectItem key={o.value} value={o.value}>
                    {o.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}
        <div className="text-xs text-muted-foreground">
          {pct}% of{" "}
          {props.length
            ? "persons where " +
              props
                .map((f) => {
                  const d = describeFilter(f);
                  return `${d.key} ${d.op} ${d.value}`;
                })
                .join(" and ")
            : "all persons"}
          .
        </div>
      </div>
    </div>
  );
}

function PayloadEditor({ value, onChange, label, disabled }: { value: unknown; onChange: (v: unknown | undefined) => void; label: string; disabled?: boolean }) {
  const [text, setText] = useState(value === undefined ? "" : JSON.stringify(value, null, 2));
  const [invalid, setInvalid] = useState(false);
  const id = `payload-${label}`;
  return (
    <FormField
      label={<span className="font-mono">{label}</span>}
      htmlFor={id}
      error={invalid ? "Not valid JSON yet; the last valid value is kept." : undefined}
    >
      <Textarea
        id={id}
        className="font-mono"
        rows={3}
        placeholder='{"color": "blue"}'
        value={text}
        disabled={disabled}
        aria-label={`Payload for ${label}`}
        aria-invalid={invalid || undefined}
        onChange={(e) => {
          setText(e.target.value);
          if (!e.target.value.trim()) {
            setInvalid(false);
            onChange(undefined);
            return;
          }
          try {
            onChange(JSON.parse(e.target.value));
            setInvalid(false);
          } catch {
            setInvalid(true);
          }
        }}
      />
    </FormField>
  );
}

function TestUser({ flagId }: { flagId: number }) {
  const projectId = useProjectId();
  const [distinctId, setDistinctId] = useState("");
  const evaluate = useMutation<FlagEvaluation, unknown, string>({ mutationFn: (id) => api.evaluateFlag(projectId, flagId, id) });
  const run = () => {
    if (distinctId.trim() && !evaluate.isPending) evaluate.mutate(distinctId.trim());
  };
  const result = evaluate.data;
  return (
    <Section title="Test a user" sub="See what one person gets, and why. Uses the saved version of this flag.">
      <div className="flex flex-col gap-3">
        <div className="flex gap-2">
          <Input
            placeholder="distinct_id (e.g. user@example.com)"
            value={distinctId}
            onChange={(e) => setDistinctId(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                run();
              }
            }}
            aria-label="Distinct ID"
            className="flex-1"
          />
          <Button type="button" onClick={run} disabled={evaluate.isPending || !distinctId.trim()}>
            {evaluate.isPending ? "Evaluating…" : "Evaluate"}
          </Button>
        </div>
        {evaluate.error ? <ErrorState error={evaluate.error} compact /> : null}
        {result && !evaluate.error && (
          <Notice tone={result.enabled ? "good" : "info"} icon={result.enabled ? "check" : "x"}>
            <div className="flex flex-col gap-2" aria-live="polite">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant={result.enabled ? "default" : "secondary"}>{result.enabled ? "enabled" : "disabled"}</Badge>
                {result.variant && (
                  <span>
                    variant <b className="font-mono">{result.variant}</b>
                  </span>
                )}
              </div>
              <div className="text-xs">
                Reason: <b className="font-mono">{result.reason}</b>
                {result.condition_index !== null && <> · matched condition set {result.condition_index + 1}</>}
              </div>
              {result.payload !== null && result.payload !== undefined && <JsonView value={result.payload} />}
            </div>
          </Notice>
        )}
      </div>
    </Section>
  );
}

// ── Editor ───────────────────────────────────────────────────────────────

const slugify = (s: string) => s.replace(/\s+/g, "-");

interface ServerError {
  field: string | null;
  message: string;
}

function FlagEditor({ id, initial, updatedAt }: { id: number | null; initial: FeatureFlagInput; updatedAt: string | null }) {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const readOnly = !editable;
  const queryClient = useQueryClient();
  const [serverError, setServerError] = useState<ServerError | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [savedAt, setSavedAt] = useState<string | null>(updatedAt);
  const dirtyRef = useRef(false);
  const bypassRef = useRef(false);

  const save = useMutation({
    mutationFn: (body: FeatureFlagInput) => (id === null ? api.createFlag(projectId, body) : api.updateFlag(projectId, id, body)),
    onError: (e) => setServerError({ field: e instanceof ApiError ? e.field : null, message: errorMessage(e) }),
  });

  const form = useForm({
    defaultValues: initial,
    onSubmit: async ({ value }) => {
      setServerError(null);
      const payloadKeys = value.filters.multivariate ? value.filters.multivariate.variants.map((v) => v.key).filter(Boolean) : ["true"];
      const body: FeatureFlagInput = {
        ...value,
        filters: {
          ...value.filters,
          groups: value.filters.groups.map((g) => ({ ...g, properties: completeFilters(g.properties) })),
          payloads: Object.fromEntries(Object.entries(value.filters.payloads).filter(([k, v]) => payloadKeys.includes(k) && v !== undefined)),
        },
      };
      let saved: FeatureFlag;
      try {
        saved = await save.mutateAsync(body);
      } catch {
        return; // onError surfaced it
      }
      queryClient.setQueryData(qk.flag(projectId, saved.id), saved);
      await queryClient.invalidateQueries({ queryKey: qk.flags(projectId) });
      toast(id === null ? `Created ${saved.key}` : `Saved ${saved.key}`);
      if (id === null) {
        bypassRef.current = true;
        navigate(path(`flags/${saved.id}`), { replace: true });
      } else {
        form.reset(toInput(saved));
        setSavedAt(saved.updated_at);
      }
    },
  });

  // Leave guard: unsaved edits survive neither a route change nor a tab close without asking.
  const blocker = useBlocker({
    shouldBlockFn: () => dirtyRef.current && !bypassRef.current && !deleting,
    enableBeforeUnload: () => dirtyRef.current && !bypassRef.current,
    withResolver: true,
  });

  const remove = useMutation({
    mutationFn: () => api.deleteFlag(projectId, id as number),
    onSuccess: async (_r, _v, _c) => {
      bypassRef.current = true;
      toast("Deleted flag");
      navigate(path("flags"));
      // Exact: invalidating the prefix would refetch the (now deleted) detail we are leaving.
      await queryClient.invalidateQueries({ queryKey: qk.flags(projectId), exact: true });
    },
  });

  const snippetKey = (k: string) => k || "my-flag";
  return (
    <Page narrow>
      <FlagHeader
        id={id}
        savedAt={savedAt}
        title={
          <form.Subscribe selector={(s) => s.values.key}>{(key) => (id === null ? "New feature flag" : key)}</form.Subscribe>
        }
        actions={
          editable && (
            <>
              {id !== null && (
                <Button type="button" variant="destructive" onClick={() => setDeleting(true)}>
                  <Icon name="trash" size={14} /> Delete
                </Button>
              )}
              <form.Subscribe selector={(s) => [s.canSubmit, s.isSubmitting, s.isDirty] as const}>
                {([canSubmit, isSubmitting, isDirty]) => {
                  dirtyRef.current = isDirty;
                  return (
                    <>
                      {isDirty && id !== null && <span className="text-xs text-muted-foreground">Unsaved changes</span>}
                      <Button type="button" onClick={() => void form.handleSubmit()} disabled={!canSubmit || isSubmitting || (id !== null && !isDirty)}>
                        <Icon name="save" size={14} /> {isSubmitting ? "Saving…" : id === null ? "Create flag" : "Save"}
                      </Button>
                    </>
                  );
                }}
              </form.Subscribe>
            </>
          )
        }
      />
      {serverError && !(serverError.field === "key" && id === null) && (
        <Notice tone="bad" className="mb-4">
          {serverError.message}
        </Notice>
      )}

      <div className="flex flex-col gap-4">
        <Section title="Flag">
          <div className="flex flex-col gap-4">
            <form.Field
              name="key"
              validators={{ onMount: ({ value }) => validateKey(value), onChange: ({ value }) => validateKey(value) }}
            >
              {(field) => {
                const value = field.state.value;
                const invalidKey = !!value && !KEY_RE.test(value);
                const serverKey = serverError?.field === "key" ? serverError.message : null;
                return (
                  <FormField
                    label="Key"
                    htmlFor="flag-key"
                    error={invalidKey ? validateKey(value) : serverKey}
                    hint={id !== null ? "Keys can't change once SDKs depend on them." : "Letters, numbers, - and _. This is what your code checks."}
                  >
                    <Input
                      id="flag-key"
                      className="font-mono"
                      value={value}
                      placeholder="new-checkout"
                      autoComplete="off"
                      spellCheck={false}
                      disabled={id !== null || readOnly}
                      aria-invalid={invalidKey || !!serverKey || undefined}
                      onChange={(e) => {
                        setServerError(null);
                        field.handleChange(slugify(e.target.value));
                      }}
                      onBlur={field.handleBlur}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && editable) {
                          e.preventDefault();
                          void form.handleSubmit();
                        }
                      }}
                    />
                  </FormField>
                );
              }}
            </form.Field>

            <form.Field name="name">
              {(field) => (
                <FormField label="Description" htmlFor="flag-name">
                  <Textarea
                    id="flag-name"
                    rows={2}
                    value={field.state.value}
                    placeholder="What does this flag control?"
                    disabled={readOnly}
                    onChange={(e) => field.handleChange(e.target.value)}
                    onBlur={field.handleBlur}
                  />
                </FormField>
              )}
            </form.Field>

            <div className="flex flex-wrap gap-x-8 gap-y-3">
              <form.Field name="active">
                {(field) => (
                  <div className="flex items-center gap-2.5">
                    <Switch id="flag-active" checked={field.state.value} onCheckedChange={field.handleChange} disabled={readOnly} aria-labelledby="flag-active-label" />
                    <Label id="flag-active-label" htmlFor="flag-active" className="flex-wrap gap-x-1">
                      <b>{field.state.value ? "Enabled" : "Disabled"}</b>
                      <span className="font-normal text-muted-foreground">· disabled flags return false for everyone</span>
                    </Label>
                  </div>
                )}
              </form.Field>
              <form.Field name="ensure_experience_continuity">
                {(field) => (
                  <div className="flex items-center gap-2.5">
                    <Checkbox id="flag-continuity" checked={field.state.value} onCheckedChange={(v) => field.handleChange(v === true)} disabled={readOnly} />
                    <Label htmlFor="flag-continuity" className="flex-wrap gap-x-1">
                      Persist across identify
                      <span className="font-normal text-muted-foreground">· a person keeps their value after logging in</span>
                    </Label>
                  </div>
                )}
              </form.Field>
            </div>
          </div>
        </Section>

        <form.Field name="filters" validators={{ onMount: ({ value }) => validateFilters(value), onChange: ({ value }) => validateFilters(value) }}>
          {(field) => {
            const filters = field.state.value;
            const setFilters = (patch: Partial<FlagFilters>) => field.handleChange({ ...filters, ...patch });
            const variants = filters.multivariate?.variants ?? [];
            const setVariants = (next: FlagVariant[]) => setFilters({ multivariate: { variants: next } });
            const patchVariant = (i: number, patch: Partial<FlagVariant>) => setVariants(variants.map((x, j) => (j === i ? { ...x, ...patch } : x)));
            const variantSum = variants.reduce((a, v) => a + v.rollout_percentage, 0);
            const payloadKeys = filters.multivariate ? variants.map((v) => v.key).filter(Boolean) : ["true"];
            return (
              <>
                <Section
                  title="Served value"
                  aside={
                    <ToggleGroup
                      aria-label="Flag type"
                      variant="outline"
                      size="sm"
                      spacing={0}
                      disabled={readOnly}
                      value={[filters.multivariate ? "multi" : "bool"]}
                      onValueChange={(next) => {
                        const v = next[0];
                        if (v === undefined) return;
                        setFilters({
                          multivariate:
                            v === "multi"
                              ? {
                                  variants: [
                                    { key: "control", name: null, rollout_percentage: 50 },
                                    { key: "test", name: null, rollout_percentage: 50 },
                                  ],
                                }
                              : null,
                        });
                      }}
                    >
                      <ToggleGroupItem value="bool">Boolean</ToggleGroupItem>
                      <ToggleGroupItem value="multi">Multiple variants</ToggleGroupItem>
                    </ToggleGroup>
                  }
                >
                  {!filters.multivariate ? (
                    <p className="text-muted-foreground">
                      Matched persons get <code>true</code>; everyone else gets <code>false</code>.
                    </p>
                  ) : (
                    <div className="flex flex-col gap-3">
                      <div className="overflow-x-auto">
                        <div className="flex min-w-[520px] flex-col gap-2">
                          <div className="grid grid-cols-[1fr_1fr_84px_32px] gap-2 text-xs font-semibold text-muted-foreground">
                            <span>Variant key</span>
                            <span>Description</span>
                            <span>Split</span>
                            <span />
                          </div>
                          {variants.map((v, i) => (
                            <div key={i} className="grid grid-cols-[1fr_1fr_84px_32px] items-center gap-2">
                              <div className="flex items-center gap-2">
                                <span aria-hidden="true" className="size-2.5 flex-none rounded-sm" style={{ background: variantColor(i) }} />
                                <Input
                                  className="min-w-0 flex-1 font-mono"
                                  value={v.key}
                                  placeholder="variant-key"
                                  disabled={readOnly}
                                  aria-label={`Variant ${i + 1} key`}
                                  aria-invalid={!v.key.trim() || undefined}
                                  onChange={(e) => patchVariant(i, { key: slugify(e.target.value) })}
                                />
                              </div>
                              <Input
                                value={v.name ?? ""}
                                placeholder="Optional"
                                disabled={readOnly}
                                aria-label={`Variant ${i + 1} description`}
                                onChange={(e) => patchVariant(i, { name: e.target.value || null })}
                              />
                              <Pct value={v.rollout_percentage} label={`Variant ${i + 1} split`} disabled={readOnly} onChange={(p) => patchVariant(i, { rollout_percentage: p })} />
                              <Button
                                type="button"
                                variant="ghost"
                                size="icon-sm"
                                disabled={variants.length <= 2 || readOnly}
                                aria-label={`Remove variant ${i + 1}`}
                                onClick={() => setVariants(variants.filter((_, j) => j !== i))}
                              >
                                <Icon name="x" size={13} />
                              </Button>
                            </div>
                          ))}
                        </div>
                      </div>
                      <div className="flex h-2 overflow-hidden rounded-full bg-muted" aria-hidden="true">
                        {variants.map((v, i) => (
                          <span key={i} style={{ width: `${Math.max(0, v.rollout_percentage)}%`, background: variantColor(i) }} />
                        ))}
                      </div>
                      <div className="flex flex-wrap items-center gap-2">
                        {!readOnly && (
                          <>
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              onClick={() => setVariants([...variants, { key: `variant-${variants.length + 1}`, name: null, rollout_percentage: 0 }])}
                            >
                              <Icon name="plus" size={13} /> Add variant
                            </Button>
                            <Button
                              type="button"
                              variant="ghost"
                              size="sm"
                              onClick={() => {
                                const n = variants.length;
                                const base = Math.floor(100 / n);
                                setVariants(variants.map((v, i) => ({ ...v, rollout_percentage: i === 0 ? 100 - base * (n - 1) : base })));
                              }}
                            >
                              Distribute equally
                            </Button>
                          </>
                        )}
                        <span className="flex-1" />
                        <Badge variant={near100(variantSum) ? "secondary" : "destructive"} className={cn(near100(variantSum) && "bg-good-wash text-good")} role="status">
                          {near100(variantSum) ? "Splits sum to 100%" : `Splits sum to ${variantSum}%, need 100%`}
                        </Badge>
                      </div>
                    </div>
                  )}
                </Section>

                <Section title="Release conditions" sub="A person gets the flag if they match any condition set and fall inside its rollout.">
                  <div className="flex flex-col gap-3">
                    {filters.groups.map((g, i) => (
                      <div key={i} className="flex flex-col gap-3">
                        {i > 0 && (
                          <div className="flex items-center gap-3 text-xs font-semibold text-muted-foreground" aria-hidden="true">
                            <span className="h-px flex-1 bg-border" />
                            OR
                            <span className="h-px flex-1 bg-border" />
                          </div>
                        )}
                        <ConditionGroup
                          group={g}
                          index={i}
                          readOnly={readOnly}
                          variants={variants.map((v) => v.key)}
                          onChange={(ng) => setFilters({ groups: filters.groups.map((x, j) => (j === i ? ng : x)) })}
                          onRemove={filters.groups.length > 1 ? () => setFilters({ groups: filters.groups.filter((_, j) => j !== i) }) : null}
                        />
                      </div>
                    ))}
                    {!readOnly && (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="self-start"
                        onClick={() => setFilters({ groups: [...filters.groups, { properties: [], rollout_percentage: 100, variant: null }] })}
                      >
                        <Icon name="plus" size={13} /> Add condition set
                      </Button>
                    )}
                  </div>
                </Section>

                <Section title="Payloads" sub="Optional JSON returned with the flag value (getFeatureFlagPayload).">
                  <div className="flex flex-col gap-3">
                    {payloadKeys.length === 0 && <span className="text-muted-foreground">Name a variant to attach a payload.</span>}
                    {payloadKeys.map((k) => (
                      <PayloadEditor
                        key={k}
                        label={k}
                        value={filters.payloads[k]}
                        disabled={readOnly}
                        onChange={(v) => {
                          const next = { ...filters.payloads };
                          if (v === undefined) delete next[k];
                          else next[k] = v;
                          setFilters({ payloads: next });
                        }}
                      />
                    ))}
                  </div>
                </Section>
                {field.state.meta.errors.length > 0 && filters.multivariate && (
                  <Notice tone="warn">{field.state.meta.errors.filter((e): e is string => typeof e === "string").join(" ")}</Notice>
                )}
              </>
            );
          }}
        </form.Field>

        {id !== null && <TestUser flagId={id} />}
        <Section title="Use it in code">
          <form.Subscribe selector={(s) => [s.values.key, s.values.filters.multivariate?.variants[1]?.key ?? "test", !!s.values.filters.multivariate] as const}>
            {([key, second, multi]) => (
              <Snippet
                language="js"
                code={
                  multi
                    ? `const variant = posthog.getFeatureFlag('${snippetKey(key)}')\nif (variant === '${second}') {\n  // new experience\n}\nconst payload = posthog.getFeatureFlagPayload('${snippetKey(key)}')`
                    : `if (posthog.isFeatureEnabled('${snippetKey(key)}')) {\n  // new experience\n}\nconst payload = posthog.getFeatureFlagPayload('${snippetKey(key)}')`
                }
              />
            )}
          </form.Subscribe>
        </Section>
      </div>

      {deleting && id !== null && (
        <Confirm
          title={`Delete ${initial.key}?`}
          body="SDKs will treat this flag as missing (false) on their next refresh."
          confirmLabel="Delete flag"
          danger
          onClose={() => setDeleting(false)}
          onConfirm={() => remove.mutateAsync()}
        />
      )}

      {blocker.status === "blocked" && (
        <AppDialog
          title="Discard unsaved changes?"
          description="You have edits to this flag that haven't been saved."
          onClose={() => blocker.reset()}
          footer={
            <>
              <Button variant="outline" onClick={() => blocker.reset()}>
                Keep editing
              </Button>
              <Button variant="destructive" onClick={() => blocker.proceed()}>
                Discard changes
              </Button>
            </>
          }
        >
          <span className="text-muted-foreground">Leaving now loses them.</span>
        </AppDialog>
      )}
    </Page>
  );
}

function FlagHeader({ id, savedAt, title, actions }: { id: number | null; savedAt: string | null; title: ReactNode; actions: ReactNode }) {
  const path = usePath();
  return (
    <>
      <nav aria-label="Breadcrumb" className="mb-2 flex items-center gap-1.5 text-xs text-muted-foreground">
        <Link to={path("flags")} className="rounded-sm hover:text-foreground hover:underline">
          Feature flags
        </Link>
        <Icon name="chevronRight" size={12} />
        <span aria-current="page">{id === null ? "New" : "Edit"}</span>
      </nav>
      <PageHeader title={id === null ? title : <span className="font-mono">{title}</span>} sub={savedAt ? `Updated ${fmtRelative(savedAt)}` : undefined} actions={actions} />
    </>
  );
}

function FlagLoader({ id }: { id: number }) {
  const projectId = useProjectId();
  // Always edit the server's current version, never a cached copy.
  const { data, error, isFetching, refetch } = useQuery({ ...flagQuery(projectId, id), staleTime: 0, refetchOnMount: "always" });
  const [initial, setInitial] = useState<FeatureFlag | null>(null);
  useEffect(() => {
    if (!initial && data && !isFetching) setInitial(data);
  }, [initial, data, isFetching]);

  if (!initial) {
    if (error && !data) {
      return (
        <Page narrow>
          <ErrorState error={error} retry={() => void refetch()} />
        </Page>
      );
    }
    return (
      <Page narrow>
        <div className="flex flex-col gap-4" aria-busy="true" aria-label="Loading">
          <Skeleton className="h-7 w-64" />
          <Skeleton className="h-52" />
          <Skeleton className="h-64" />
        </div>
      </Page>
    );
  }
  return <FlagEditor id={id} initial={toInput(initial)} updatedAt={initial.updated_at} />;
}

export function NewFlagPage() {
  return <FlagEditor id={null} initial={EMPTY} updatedAt={null} />;
}

export function EditFlagPage() {
  const { id } = flagRouteApi.useParams();
  return <FlagLoader key={id} id={id} />;
}
