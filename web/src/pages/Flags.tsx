import { useEffect, useState, type ReactNode } from "react";
import type { FeatureFlag } from "../types/FeatureFlag";
import type { FeatureFlagInput } from "../types/FeatureFlagInput";
import type { FlagConditionGroup } from "../types/FlagConditionGroup";
import type { FlagEvaluation } from "../types/FlagEvaluation";
import type { FlagFilters } from "../types/FlagFilters";
import { api, errorMessage } from "../lib/api";
import { canEdit, useApp, usePath, useProjectId } from "../lib/context";
import { fmtRelative } from "../lib/format";
import { invalidate, useApi } from "../lib/hooks";
import { describeFilter } from "../lib/properties";
import { Link, navigate } from "../lib/router";
import { seriesColor } from "../charts/scale";
import { Icon } from "../ui/icons";
import { Confirm, Empty, ErrorState, JsonView, Seg, Skeleton, SkeletonRows, Snippet, Switch, toast } from "../ui/kit";
import { PropertyFilters, completeFilters } from "../insight/pickers";

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

export function FlagsPage() {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const { data, error, loading, reload, setData } = useApi(`flags:${projectId}`, (s) => api.flags(projectId, s));
  const [search, setSearch] = useState("");
  const list = (data ?? []).filter((f) => !search || `${f.key} ${f.name}`.toLowerCase().includes(search.toLowerCase()));

  const toggle = async (flag: FeatureFlag, active: boolean) => {
    if (!data) return;
    setData(data.map((f) => (f.id === flag.id ? { ...f, active } : f)));
    try {
      await api.updateFlag(projectId, flag.id, { active });
      invalidate(`flag:${projectId}:${flag.id}`);
      toast(`${flag.key} ${active ? "enabled" : "disabled"}`);
    } catch (e) {
      setData(data);
      toast(errorMessage(e), true);
    }
  };

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>Feature flags</h1>
          <div className="sub">Roll features out safely. PostHog SDKs evaluate these locally or via /flags.</div>
        </div>
        <div className="actions">
          {editable && (
            <Link className="btn primary" to={path("flags/new")}>
              <Icon name="plus" size={14} /> New flag
            </Link>
          )}
        </div>
      </div>
      <div className="card">
        <div className="card-head">
          <div className="search" style={{ width: 300 }}>
            <Icon name="search" size={14} />
            <input className="input" placeholder="Search flags…" value={search} onChange={(e) => setSearch(e.target.value)} aria-label="Search flags" />
          </div>
          <span className="spacer" />
          {data && <span className="muted small">{data.filter((f) => f.active).length} active of {data.length}</span>}
        </div>
        {error && !data ? (
          <ErrorState error={error} retry={reload} />
        ) : !data && loading ? (
          <SkeletonRows rows={5} />
        ) : data && data.length === 0 ? (
          <Empty icon="flag" title="No feature flags yet" action={editable && <Link className="btn primary" to={path("flags/new")}>Create your first flag</Link>}>
            Create a flag, then gate code with <code>posthog.isFeatureEnabled('my-flag')</code>. Flags evaluate in-process here; no extra service.
          </Empty>
        ) : list.length === 0 ? (
          <Empty icon="search" title="No flags match" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Flag</th>
                <th>Release</th>
                <th>Updated</th>
                <th className="r">Enabled</th>
              </tr>
            </thead>
            <tbody>
              {list.map((f) => (
                <tr key={f.id} className="clickable" onClick={() => navigate(path(`flags/${f.id}`))}>
                  <td>
                    <div className="col" style={{ gap: 0 }}>
                      <Link to={path(`flags/${f.id}`)} className="mono" style={{ fontWeight: 600 }} onClick={(e) => e.stopPropagation()}>
                        {f.key}
                      </Link>
                      {f.name && <span className="muted small truncate" style={{ maxWidth: 480 }}>{f.name}</span>}
                    </div>
                  </td>
                  <td className="secondary">{rolloutSummary(f)}</td>
                  <td className="muted nowrap">{fmtRelative(f.updated_at)}</td>
                  <td className="r" onClick={(e) => e.stopPropagation()}>
                    <Switch checked={f.active} onChange={(v) => toggle(f, v)} label={`Enable ${f.key}`} disabled={!editable} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

// ── Editor ───────────────────────────────────────────────────────────────

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

function Card({ title, sub, children, aside }: { title: string; sub?: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="card">
      <div className="card-head">
        <div className="col grow" style={{ gap: 0 }}>
          <h3>{title}</h3>
          {sub && <span className="muted small">{sub}</span>}
        </div>
        {aside}
      </div>
      <div className="card-body">{children}</div>
    </div>
  );
}

function Pct({ value, onChange, label }: { value: number; onChange: (v: number) => void; label: string }) {
  return (
    <div className="pct-input" style={{ width: 84 }}>
      <input className="input" type="number" min={0} max={100} step={1} value={Number.isFinite(value) ? value : 0} onChange={(e) => onChange(Math.max(0, Math.min(100, Number(e.target.value) || 0)))} aria-label={label} />
    </div>
  );
}

function ConditionGroup({
  group,
  index,
  variants,
  onChange,
  onRemove,
}: {
  group: FlagConditionGroup;
  index: number;
  variants: string[];
  onChange: (g: FlagConditionGroup) => void;
  onRemove: (() => void) | null;
}) {
  const pct = group.rollout_percentage ?? 100;
  const props = completeFilters(group.properties);
  return (
    <div className="condition">
      <div className="row" style={{ marginBottom: 10 }}>
        <b className="grow">Condition set {index + 1}</b>
        {onRemove && (
          <button className="btn ghost icon small" aria-label="Remove condition set" onClick={onRemove}>
            <Icon name="trash" size={13} />
          </button>
        )}
      </div>
      <div className="col gap-12">
        <div className="col" style={{ gap: 6 }}>
          <span className="label">Match persons where</span>
          <PropertyFilters value={group.properties} onChange={(properties) => onChange({ ...group, properties })} sources={["person"]} addLabel="Add condition" />
          {group.properties.length === 0 && <span className="muted small">No conditions: every person matches.</span>}
        </div>
        <div className="col" style={{ gap: 6 }}>
          <span className="label">Roll out to</span>
          <div className="row gap-12">
            <input type="range" min={0} max={100} value={pct} onChange={(e) => onChange({ ...group, rollout_percentage: Number(e.target.value) })} aria-label="Rollout percentage" />
            <Pct value={pct} onChange={(v) => onChange({ ...group, rollout_percentage: v })} label="Rollout percentage" />
          </div>
        </div>
        {variants.length > 0 && (
          <label className="row small secondary">
            Serve variant
            <select className="select small" value={group.variant ?? ""} onChange={(e) => onChange({ ...group, variant: e.target.value || null })}>
              <option value="">by the split below</option>
              {variants.map((v) => (
                <option key={v} value={v}>
                  {v}
                </option>
              ))}
            </select>
          </label>
        )}
        <div className="muted small">
          {pct}% of {props.length ? "persons where " + props.map((f) => { const d = describeFilter(f); return `${d.key} ${d.op} ${d.value}`; }).join(" and ") : "all persons"}.
        </div>
      </div>
    </div>
  );
}

function PayloadEditor({ value, onChange, label }: { value: unknown; onChange: (v: unknown | undefined) => void; label: string }) {
  const [text, setText] = useState(value === undefined ? "" : JSON.stringify(value, null, 2));
  const [invalid, setInvalid] = useState(false);
  return (
    <div className="col" style={{ gap: 4 }}>
      <span className="label mono">{label}</span>
      <textarea
        className={`textarea mono${invalid ? " invalid" : ""}`}
        rows={3}
        placeholder='{"color": "blue"}'
        value={text}
        aria-label={`Payload for ${label}`}
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
      {invalid && <span className="small" style={{ color: "var(--bad)" }}>Not valid JSON yet; the last valid value is kept.</span>}
    </div>
  );
}

function TestUser({ flagId }: { flagId: number }) {
  const projectId = useProjectId();
  const [distinctId, setDistinctId] = useState("");
  const [result, setResult] = useState<FlagEvaluation | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const run = async () => {
    if (!distinctId.trim()) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await api.evaluateFlag(projectId, flagId, distinctId.trim()));
    } catch (e) {
      setResult(null);
      setError(e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card title="Test a user" sub="See what one person gets, and why. Uses the saved version of this flag.">
      <div className="col gap-12">
        <div className="row">
          <input className="input grow" placeholder="distinct_id (e.g. user@example.com)" value={distinctId} onChange={(e) => setDistinctId(e.target.value)} onKeyDown={(e) => e.key === "Enter" && run()} aria-label="Distinct ID" />
          <button className="btn primary" onClick={run} disabled={busy || !distinctId.trim()}>
            {busy ? "Evaluating…" : "Evaluate"}
          </button>
        </div>
        {error ? <ErrorState error={error} compact /> : null}
        {result && (
          <div className={`notice ${result.enabled ? "good" : ""}`} style={{ flexDirection: "column", gap: 8 }}>
            <div className="row">
              <span className={`badge ${result.enabled ? "good" : ""}`}>{result.enabled ? "enabled" : "disabled"}</span>
              {result.variant && (
                <span>
                  variant <b className="mono">{result.variant}</b>
                </span>
              )}
            </div>
            <div className="small">
              Reason: <b className="mono">{result.reason}</b>
              {result.condition_index !== null && <> · matched condition set {result.condition_index + 1}</>}
            </div>
            {result.payload !== null && result.payload !== undefined && <JsonView value={result.payload} />}
          </div>
        )}
      </div>
    </Card>
  );
}

export function FlagPage({ id }: { id: number | null }) {
  const projectId = useProjectId();
  const path = usePath();
  const { organization } = useApp();
  const editable = canEdit(organization);
  const existing = useApi(id !== null ? `flag:${projectId}:${id}` : null, (s) => api.flag(projectId, id as number, s), { keepPrevious: false });
  const [form, setForm] = useState<FeatureFlagInput | null>(id === null ? EMPTY : null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    if (existing.data && !form) setForm(toInput(existing.data));
  }, [existing.data, form]);

  if (existing.error) {
    return (
      <div className="page narrow">
        <ErrorState error={existing.error} retry={existing.reload} />
      </div>
    );
  }
  if (!form) {
    return (
      <div className="page narrow col gap-16">
        <Skeleton height={28} width={260} />
        <Skeleton height={200} />
        <Skeleton height={260} />
      </div>
    );
  }

  const set = (patch: Partial<FeatureFlagInput>) => setForm({ ...form, ...patch });
  const setFilters = (patch: Partial<FlagFilters>) => set({ filters: { ...form.filters, ...patch } });
  const variants = form.filters.multivariate?.variants ?? [];
  const variantSum = variants.reduce((a, v) => a + v.rollout_percentage, 0);
  const keyValid = /^[a-zA-Z0-9_-]+$/.test(form.key);
  const valid = keyValid && (!form.filters.multivariate || (variants.length >= 2 && Math.abs(variantSum - 100) < 0.01 && variants.every((v) => v.key.trim())));
  const payloadKeys = form.filters.multivariate ? variants.map((v) => v.key).filter(Boolean) : ["true"];

  const save = async () => {
    setSaving(true);
    setSaveError(null);
    const body: FeatureFlagInput = {
      ...form,
      filters: {
        ...form.filters,
        groups: form.filters.groups.map((g) => ({ ...g, properties: completeFilters(g.properties) })),
        payloads: Object.fromEntries(Object.entries(form.filters.payloads).filter(([k, v]) => payloadKeys.includes(k) && v !== undefined)),
      },
    };
    try {
      const saved = id === null ? await api.createFlag(projectId, body) : await api.updateFlag(projectId, id, body);
      invalidate(`flags:${projectId}`);
      invalidate(`flag:${projectId}:${saved.id}`);
      toast(id === null ? `Created ${saved.key}` : `Saved ${saved.key}`);
      if (id === null) navigate(path(`flags/${saved.id}`), { replace: true });
      else existing.setData(saved);
    } catch (e) {
      setSaveError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const snippetKey = form.key || "my-flag";
  return (
    <div className="page narrow">
      <div className="row small muted" style={{ marginBottom: 10 }}>
        <Link to={path("flags")} className="link">
          Feature flags
        </Link>
        <Icon name="chevronRight" size={12} />
      </div>
      <div className="page-head">
        <div className="titles">
          <h1 className="title-mono">{id === null ? "New feature flag" : existing.data?.key}</h1>
          {existing.data && <div className="sub">Updated {fmtRelative(existing.data.updated_at)}</div>}
        </div>
        <div className="actions">
          {id !== null && editable && (
            <button className="btn danger" onClick={() => setDeleting(true)}>
              <Icon name="trash" size={14} /> Delete
            </button>
          )}
          {editable && (
            <button className="btn primary" onClick={save} disabled={saving || !valid}>
              <Icon name="save" size={14} /> {saving ? "Saving…" : id === null ? "Create flag" : "Save"}
            </button>
          )}
        </div>
      </div>
      {saveError && <div className="notice bad" style={{ marginBottom: 16 }}>{saveError}</div>}

      <fieldset disabled={!editable} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }} className="col gap-16">
        <Card title="Flag">
          <div className="col gap-16">
            <label className="field">
              <span>Key</span>
              <input
                className={`input mono${form.key && !keyValid ? " invalid" : ""}`}
                value={form.key}
                placeholder="new-checkout"
                onChange={(e) => set({ key: e.target.value.replace(/\s+/g, "-") })}
                disabled={id !== null}
              />
              <span className={form.key && !keyValid ? "error" : "hint"}>
                {id !== null ? "Keys can't change once SDKs depend on them." : "Letters, numbers, - and _. This is what your code checks."}
              </span>
            </label>
            <label className="field">
              <span>Description</span>
              <textarea className="textarea" rows={2} value={form.name} placeholder="What does this flag control?" onChange={(e) => set({ name: e.target.value })} />
            </label>
            <div className="row gap-24 wrap">
              <label className="row">
                <Switch checked={form.active} onChange={(active) => set({ active })} label="Enabled" />
                <span>
                  <b>{form.active ? "Enabled" : "Disabled"}</b>
                  <span className="muted small"> · disabled flags return false for everyone</span>
                </span>
              </label>
              <label className="row">
                <input type="checkbox" checked={form.ensure_experience_continuity} onChange={(e) => set({ ensure_experience_continuity: e.target.checked })} />
                <span>
                  Persist across identify
                  <span className="muted small"> · a person keeps their value after logging in</span>
                </span>
              </label>
            </div>
          </div>
        </Card>

        <Card
          title="Served value"
          aside={
            <Seg
              label="Flag type"
              value={form.filters.multivariate ? "multi" : "bool"}
              onChange={(v) =>
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
                })
              }
              options={[
                { value: "bool", label: "Boolean" },
                { value: "multi", label: "Multiple variants" },
              ]}
            />
          }
        >
          {!form.filters.multivariate ? (
            <p className="secondary">Matched persons get <code>true</code>; everyone else gets <code>false</code>.</p>
          ) : (
            <div className="col gap-12">
              <div className="variant-row small muted" style={{ fontWeight: 600 }}>
                <span>Variant key</span>
                <span>Description</span>
                <span>Split</span>
                <span />
              </div>
              {variants.map((v, i) => (
                <div key={i} className="variant-row">
                  <div className="row gap-4">
                    <span className="swatch" style={{ background: seriesColor(i) }} />
                    <input
                      className="input mono grow"
                      value={v.key}
                      placeholder="variant-key"
                      onChange={(e) => setFilters({ multivariate: { variants: variants.map((x, j) => (j === i ? { ...x, key: e.target.value.replace(/\s+/g, "-") } : x)) } })}
                      aria-label="Variant key"
                    />
                  </div>
                  <input
                    className="input"
                    value={v.name ?? ""}
                    placeholder="Optional"
                    onChange={(e) => setFilters({ multivariate: { variants: variants.map((x, j) => (j === i ? { ...x, name: e.target.value || null } : x)) } })}
                    aria-label="Variant description"
                  />
                  <Pct value={v.rollout_percentage} label="Variant split" onChange={(p) => setFilters({ multivariate: { variants: variants.map((x, j) => (j === i ? { ...x, rollout_percentage: p } : x)) } })} />
                  <button
                    className="btn ghost icon small"
                    disabled={variants.length <= 2}
                    aria-label="Remove variant"
                    onClick={() => setFilters({ multivariate: { variants: variants.filter((_, j) => j !== i) } })}
                  >
                    <Icon name="x" size={13} />
                  </button>
                </div>
              ))}
              <div className="split-bar" aria-hidden="true">
                {variants.map((v, i) => (
                  <span key={i} style={{ width: `${v.rollout_percentage}%`, background: seriesColor(i) }} />
                ))}
              </div>
              <div className="row">
                <button className="btn small" onClick={() => setFilters({ multivariate: { variants: [...variants, { key: `variant-${variants.length + 1}`, name: null, rollout_percentage: 0 }] } })}>
                  <Icon name="plus" size={13} /> Add variant
                </button>
                <button
                  className="btn ghost small"
                  onClick={() => {
                    const n = variants.length;
                    const base = Math.floor(100 / n);
                    setFilters({ multivariate: { variants: variants.map((v, i) => ({ ...v, rollout_percentage: i === 0 ? 100 - base * (n - 1) : base })) } });
                  }}
                >
                  Distribute equally
                </button>
                <span className="spacer" />
                <span className={`badge ${Math.abs(variantSum - 100) < 0.01 ? "good" : "bad"}`}>
                  {Math.abs(variantSum - 100) < 0.01 ? "Splits sum to 100%" : `Splits sum to ${variantSum}%, need 100%`}
                </span>
              </div>
            </div>
          )}
        </Card>

        <Card title="Release conditions" sub="A person gets the flag if they match any condition set and fall inside its rollout.">
          <div className="col">
            {form.filters.groups.map((g, i) => (
              <div key={i}>
                {i > 0 && <div className="or">OR</div>}
                <ConditionGroup
                  group={g}
                  index={i}
                  variants={variants.map((v) => v.key)}
                  onChange={(ng) => setFilters({ groups: form.filters.groups.map((x, j) => (j === i ? ng : x)) })}
                  onRemove={form.filters.groups.length > 1 ? () => setFilters({ groups: form.filters.groups.filter((_, j) => j !== i) }) : null}
                />
              </div>
            ))}
            <button className="btn small mt-16" style={{ alignSelf: "flex-start" }} onClick={() => setFilters({ groups: [...form.filters.groups, { properties: [], rollout_percentage: 100, variant: null }] })}>
              <Icon name="plus" size={13} /> Add condition set
            </button>
          </div>
        </Card>

        <Card title="Payloads" sub="Optional JSON returned with the flag value (getFeatureFlagPayload).">
          <div className="col gap-12">
            {payloadKeys.map((k) => (
              <PayloadEditor
                key={k}
                label={k}
                value={form.filters.payloads[k]}
                onChange={(v) => {
                  const next = { ...form.filters.payloads };
                  if (v === undefined) delete next[k];
                  else next[k] = v;
                  setFilters({ payloads: next });
                }}
              />
            ))}
          </div>
        </Card>
      </fieldset>

      <div className="col gap-16 mt-16">
        {id !== null && <TestUser flagId={id} />}
        <Card title="Use it in code">
          <Snippet
            language="js"
            code={
              form.filters.multivariate
                ? `const variant = posthog.getFeatureFlag('${snippetKey}')\nif (variant === '${variants[1]?.key ?? "test"}') {\n  // new experience\n}\nconst payload = posthog.getFeatureFlagPayload('${snippetKey}')`
                : `if (posthog.isFeatureEnabled('${snippetKey}')) {\n  // new experience\n}\nconst payload = posthog.getFeatureFlagPayload('${snippetKey}')`
            }
          />
        </Card>
      </div>

      {deleting && id !== null && (
        <Confirm
          title={`Delete ${form.key}?`}
          body="SDKs will treat this flag as missing (false) on their next refresh."
          confirmLabel="Delete flag"
          danger
          onClose={() => setDeleting(false)}
          onConfirm={async () => {
            await api.deleteFlag(projectId, id);
            invalidate(`flags:${projectId}`);
            toast(`Deleted ${form.key}`);
            navigate(path("flags"));
          }}
        />
      )}
    </div>
  );
}
