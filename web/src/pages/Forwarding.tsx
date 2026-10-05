// Shadow mode: keep sending events to PostHog while Hoglet is checked against it.
// Reads GET /api/projects/{id}/forwarding (polled), writes PUT with the same shape.

import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ErrorState, Notice, SkeletonRows } from "@/components/feedback";
import { FormField } from "@/components/page";
import { toast } from "@/components/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { ApiError, api, errorMessage, type ForwardingConfig, type ForwardingStatus } from "@/lib/api";
import { fmtNumber } from "@/lib/format";
import { forwardingQuery, qk } from "@/lib/queries";

const US_HOST = "https://us.i.posthog.com";
const EU_HOST = "https://eu.i.posthog.com";
const HOST_MAX = 256;
const TOKEN_MAX = 128;

type HostChoice = "us" | "eu" | "custom";

function hostChoice(host: string): HostChoice {
  return host === EU_HOST ? "eu" : host === US_HOST || host === "" ? "us" : "custom";
}

const HOST_OPTIONS: { value: HostChoice; label: string }[] = [
  { value: "us", label: "US cloud (us.i.posthog.com)" },
  { value: "eu", label: "EU cloud (eu.i.posthog.com)" },
  { value: "custom", label: "Self-hosted or other" },
];

function validateHost(host: string): string | undefined {
  const h = host.trim();
  if (!/^https?:\/\/\S+$/.test(h)) return "Enter a full URL starting with https:// (or http://).";
  if (h.length > HOST_MAX) return `At most ${HOST_MAX} characters.`;
  return undefined;
}

function validateToken(token: string, required: boolean): string | undefined {
  const t = token.trim();
  if (!t) return required ? "Paste the PostHog project API key to forward to." : undefined;
  if (!t.startsWith("phc_")) return "A PostHog project API key starts with phc_.";
  if (t.length > TOKEN_MAX) return `At most ${TOKEN_MAX} characters.`;
  return undefined;
}

function Counter({ label, value, hint, warn }: { label: string; value: number; hint: string; warn?: boolean }) {
  return (
    <div className="min-w-0 rounded-lg border px-3 py-2" title={hint}>
      <div className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">{label}</div>
      <div className={warn && value > 0 ? "num text-lg font-semibold text-warn" : "num text-lg font-semibold"}>{fmtNumber(value)}</div>
    </div>
  );
}

function ForwardingStats({ status }: { status: ForwardingStatus }) {
  const enabled = status.config?.enabled === true;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        {enabled ? <Badge className="bg-good-wash text-good">Forwarding on</Badge> : <Badge variant="secondary">{status.config ? "Forwarding off" : "Not set up"}</Badge>}
        {status.config ? <span className="font-mono text-xs break-all text-muted-foreground">{status.config.host}</span> : null}
      </div>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        <Counter label="Forwarded" value={status.forwarded} hint="Events PostHog accepted since Hoglet last started." />
        <Counter label="Queued" value={status.queued} hint="Events waiting to be sent." />
        <Counter label="Failed" value={status.failed} hint="Events PostHog rejected or that gave up after 5 tries." warn />
        <Counter label="Dropped" value={status.dropped} hint="Events skipped because the forwarding queue was full." warn />
      </div>
      {status.last_error ? (
        <Notice tone="warn">
          Last error from PostHog: <span className="font-mono break-all">{status.last_error}</span>
        </Notice>
      ) : null}
      <p className="text-xs text-muted-foreground">Counters are since Hoglet last started and refresh every few seconds.</p>
    </div>
  );
}

function ForwardingForm({ projectId, initial }: { projectId: string; initial: ForwardingConfig | null }) {
  const queryClient = useQueryClient();
  const [serverError, setServerError] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (config: ForwardingConfig) => api.setForwarding(projectId, config),
    onSuccess: (status) => queryClient.setQueryData(qk.forwarding(projectId), status),
  });
  const defaults = {
    enabled: initial?.enabled ?? false,
    choice: hostChoice(initial?.host ?? US_HOST),
    customHost: hostChoice(initial?.host ?? US_HOST) === "custom" ? (initial?.host ?? "") : "",
    token: initial?.posthog_token ?? "",
  };
  const form = useForm({
    defaultValues: defaults,
    onSubmit: async ({ value }) => {
      setServerError(null);
      const host = value.choice === "us" ? US_HOST : value.choice === "eu" ? EU_HOST : value.customHost.trim();
      try {
        const status = await save.mutateAsync({ enabled: value.enabled, host, posthog_token: value.token.trim() });
        const saved = status.config;
        form.reset({
          enabled: saved?.enabled ?? value.enabled,
          choice: value.choice,
          customHost: value.customHost,
          token: saved?.posthog_token ?? value.token,
        });
        toast(value.enabled ? "Forwarding to PostHog is on" : "Forwarding to PostHog is off");
      } catch (e) {
        setServerError(errorMessage(e) + (e instanceof ApiError && e.requestId ? ` (${e.requestId.slice(-8)})` : ""));
      }
    },
  });

  return (
    <form
      className="flex flex-col gap-4 border-t pt-4"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        e.stopPropagation();
        void form.handleSubmit();
      }}
    >
      <form.Field name="enabled">
        {(field) => (
          <div className="flex items-center gap-3">
            <Switch id="fwd-enabled" checked={field.state.value} onCheckedChange={field.handleChange} aria-labelledby="fwd-enabled-label" />
            <label id="fwd-enabled-label" htmlFor="fwd-enabled" className="font-medium">
              Forward events to PostHog
            </label>
          </div>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => [s.values.enabled, s.values.choice] as const}>
        {([enabled, choice]) => (
          <div className="grid gap-4 sm:grid-cols-2">
            <form.Field name="choice">
              {(field) => (
                <FormField label="PostHog region" htmlFor="fwd-host">
                  <Select
                    value={field.state.value}
                    items={HOST_OPTIONS}
                    onValueChange={(v) => field.handleChange(v === "eu" || v === "custom" ? v : "us")}
                  >
                    <SelectTrigger id="fwd-host" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {HOST_OPTIONS.map((o) => (
                        <SelectItem key={o.value} value={o.value}>
                          {o.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </FormField>
              )}
            </form.Field>
            <form.Field
              name="token"
              validators={{ onChange: ({ value }) => validateToken(value, enabled), onMount: ({ value }) => validateToken(value, enabled) }}
            >
              {(field) => {
                const shown = field.state.meta.isTouched && field.state.meta.errors.length > 0 ? String(field.state.meta.errors[0]) : null;
                return (
                  <FormField label="PostHog project API key" htmlFor="fwd-token" hint="Starts with phc_. Find it in PostHog under Project settings." error={shown}>
                    <Input
                      id="fwd-token"
                      className="font-mono"
                      value={field.state.value}
                      placeholder="phc_…"
                      maxLength={TOKEN_MAX + 20}
                      autoComplete="off"
                      spellCheck={false}
                      aria-invalid={!!shown || undefined}
                      onChange={(e) => {
                        setServerError(null);
                        field.handleChange(e.target.value);
                      }}
                      onBlur={field.handleBlur}
                    />
                  </FormField>
                );
              }}
            </form.Field>
            {choice === "custom" ? (
              <form.Field name="customHost" validators={{ onChange: ({ value }) => validateHost(value), onMount: ({ value }) => validateHost(value) }}>
                {(field) => {
                  const shown = field.state.meta.isTouched && field.state.meta.errors.length > 0 ? String(field.state.meta.errors[0]) : null;
                  return (
                    <FormField className="sm:col-span-2" label="PostHog host" htmlFor="fwd-custom-host" hint="Where /batch/ is served, e.g. https://posthog.example.com" error={shown}>
                      <Input
                        id="fwd-custom-host"
                        className="font-mono"
                        value={field.state.value}
                        placeholder="https://posthog.example.com"
                        maxLength={HOST_MAX + 20}
                        autoComplete="off"
                        spellCheck={false}
                        aria-invalid={!!shown || undefined}
                        onChange={(e) => field.handleChange(e.target.value)}
                        onBlur={field.handleBlur}
                      />
                    </FormField>
                  );
                }}
              </form.Field>
            ) : null}
          </div>
        )}
      </form.Subscribe>
      {serverError ? <Notice tone="bad">{serverError}</Notice> : null}
      <div className="flex items-center gap-2">
        <form.Subscribe selector={(s) => [s.canSubmit, s.isSubmitting, s.isDirty] as const}>
          {([canSubmit, isSubmitting, isDirty]) => (
            <>
              <Button type="submit" disabled={!canSubmit || isSubmitting || !isDirty}>
                {isSubmitting ? "Saving…" : "Save"}
              </Button>
              <Button type="button" variant="ghost" disabled={!isDirty || isSubmitting} onClick={() => form.reset()}>
                Discard changes
              </Button>
            </>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

/** The Settings → Project card. Everyone on the project sees the status; owners and admins edit. */
export function ForwardingCard({ projectId, editable }: { projectId: string; editable: boolean }) {
  const { data, error, isPending, refetch } = useQuery(forwardingQuery(projectId));
  if (error && !data) return <ErrorState compact error={error} retry={() => void refetch()} />;
  if (isPending) return <SkeletonRows rows={3} className="h-40" />;
  return (
    <div className="flex flex-col gap-4">
      <ForwardingStats status={data} />
      {editable ? (
        <ForwardingForm projectId={projectId} initial={data.config} />
      ) : (
        <p className="border-t pt-3 text-muted-foreground">Only organization owners and admins can change forwarding.</p>
      )}
    </div>
  );
}
