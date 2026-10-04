import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { useMemo, useState, type ReactNode } from "react";
import { Icon, type IconName } from "@/components/icons";
import { AppDialog, Confirm } from "@/components/dialogs";
import { DataTable, columnHelper } from "@/components/data-table";
import { Empty, ErrorState, Notice, SkeletonRows } from "@/components/feedback";
import { CopyButton } from "@/components/copy";
import { CardPad, FormField, KV, Page, PageHeader, Panel } from "@/components/page";
import { toast } from "@/components/toast";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ApiError, api, errorMessage, type CreatedKey, type KeyScope, type PersonalApiKey } from "@/lib/api";
import { canEdit, projectPath, useApp, useProjectId } from "@/lib/context";
import { fmtBytes, fmtDate, fmtNumber, fmtRelative } from "@/lib/format";
import { navigate } from "@/lib/nav";
import { keysQuery, qk, statusQuery } from "@/lib/queries";
import { setTheme, useTheme, type Theme } from "@/lib/theme";
import { ForwardingCard } from "./Forwarding";
import { LoadDemoButton, SnippetTabs, TokenBox } from "./Onboarding";

const settingsRouteApi = getRouteApi("/project/$projectId/settings");

type Tab = "project" | "keys" | "account";
const NAME_MAX = 80;

function SettingsCard({ title, description, action, children }: { title: string; description?: ReactNode; action?: ReactNode; children?: ReactNode }) {
  return (
    <Panel>
      <CardPad className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <div className="min-w-0 flex-1">
            <h2>{title}</h2>
            {description ? <p className="mt-0.5 text-muted-foreground">{description}</p> : null}
          </div>
          {action}
        </div>
        {children}
      </CardPad>
    </Panel>
  );
}

/** One required name, validated, with the server's field error mapped back onto it. */
function NameDialog({
  title,
  description,
  label,
  placeholder,
  hint,
  submitLabel,
  onSubmit,
  onClose,
}: {
  title: string;
  description?: string;
  label: string;
  placeholder: string;
  hint?: string;
  submitLabel: string;
  onSubmit: (name: string) => Promise<unknown>;
  onClose: () => void;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const formId = `name-dialog-${label.toLowerCase().replace(/\W+/g, "-")}`;
  const form = useForm({
    defaultValues: { name: "" },
    onSubmit: async ({ value }) => {
      setServerError(null);
      try {
        await onSubmit(value.name.trim());
        onClose();
      } catch (e) {
        setServerError(errorMessage(e));
      }
    },
  });
  return (
    <AppDialog
      title={title}
      description={description}
      onClose={onClose}
      footer={
        <>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <form.Subscribe selector={(s) => [s.canSubmit, s.isSubmitting] as const}>
            {([canSubmit, isSubmitting]) => (
              <Button type="submit" form={formId} disabled={!canSubmit || isSubmitting}>
                {isSubmitting ? "Working…" : submitLabel}
              </Button>
            )}
          </form.Subscribe>
        </>
      }
    >
      <form
        id={formId}
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          e.stopPropagation();
          void form.handleSubmit();
        }}
      >
        <form.Field
          name="name"
          validators={{
            onMount: ({ value }) => (value.trim() ? undefined : "Name is required."),
            onChange: ({ value }) => (!value.trim() ? "Name is required." : value.length > NAME_MAX ? `At most ${NAME_MAX} characters.` : undefined),
          }}
        >
          {(field) => {
            const shown = field.state.meta.isTouched && field.state.meta.errors.length > 0 ? String(field.state.meta.errors[0]) : serverError;
            return (
              <FormField label={label} htmlFor={`${formId}-input`} hint={hint} error={shown}>
                <Input
                  id={`${formId}-input`}
                  autoFocus
                  value={field.state.value}
                  placeholder={placeholder}
                  maxLength={NAME_MAX + 20}
                  autoComplete="off"
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
      </form>
    </AppDialog>
  );
}

// ── Project ──────────────────────────────────────────────────────────────

function ProjectSettings() {
  const { project, organization, refreshWorkspace } = useApp();
  const projectId = useProjectId();
  const editable = canEdit(organization);
  const queryClient = useQueryClient();
  const status = useQuery(statusQuery(projectId));
  const [dialog, setDialog] = useState<"project" | "organization" | null>(null);

  const createProject = useMutation({
    mutationFn: (name: string) => api.createProject(organization.id, name),
    onSuccess: async (p) => {
      await refreshWorkspace();
      toast(`Created ${p.name}`);
      navigate(projectPath(p.id, "onboarding"));
    },
  });
  const createOrg = useMutation({
    mutationFn: (name: string) => api.createOrganization(name),
    onSuccess: async (o) => {
      await refreshWorkspace();
      toast(`Created ${o.name}`);
    },
  });

  const s = status.data;
  const items: [ReactNode, ReactNode][] = [
    ["Name", <span key="n" className="font-sans">{project.name}</span>],
    ["Project ID", project.id],
    [
      "Organization",
      <span key="o" className="inline-flex items-center gap-2 font-sans">
        {organization.name} <Badge variant="secondary">{organization.role}</Badge>
      </span>,
    ],
  ];
  if (s) {
    items.push(
      ["Stored events", <span key="e" className="num">{fmtNumber(s.stored_events)}</span>],
      ["Storage used", <span key="b" className="num">{fmtBytes(s.stored_bytes)}</span>],
      ["Data range", s.first_day ? `${fmtDate(s.first_day)} – ${fmtDate(s.last_day)}` : "–"],
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <SettingsCard title="Project">
        <KV items={items} />
      </SettingsCard>
      <SettingsCard title="Project token" description="Used by SDKs to send events. Safe to ship in client code; it can't read data.">
        <TokenBox token={project.token} />
      </SettingsCard>
      <SettingsCard title="Install snippets">
        <SnippetTabs token={project.token} />
      </SettingsCard>
      <SettingsCard
        title="Forward to PostHog (shadow mode)"
        description={
          <>
            Events still reach PostHog after Hoglet stores them, so you can run both side by side while you switch. Hoglet saves every event first and never waits on PostHog; if
            forwarding falls behind it drops and counts rather than slowing capture. Run <code>hoglet reconcile posthog</code> to compare the two.
          </>
        }
      >
        <ForwardingCard projectId={projectId} editable={editable} />
      </SettingsCard>
      {editable && (
        <SettingsCard
          title="Demo data"
          description="Adds 90 days of sample product data to this project so you can explore every screen."
          action={<LoadDemoButton onLoaded={() => void queryClient.invalidateQueries({ queryKey: qk.project(projectId) })} />}
        />
      )}
      {editable && (
        <SettingsCard
          title="Another project"
          description="Projects keep events, persons and flags fully separate. Each gets its own token."
          action={
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" onClick={() => setDialog("organization")}>
                <Icon name="plus" size={14} /> New organization
              </Button>
              <Button variant="outline" onClick={() => setDialog("project")}>
                <Icon name="plus" size={14} /> New project
              </Button>
            </div>
          }
        />
      )}
      {dialog === "project" && (
        <NameDialog
          title="New project"
          label="Name"
          placeholder="Marketing site"
          hint={`In ${organization.name}.`}
          submitLabel="Create project"
          onSubmit={(name) => createProject.mutateAsync(name)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "organization" && (
        <NameDialog
          title="New organization"
          description="An organization groups projects and the people who can manage them."
          label="Name"
          placeholder="Acme Inc."
          submitLabel="Create organization"
          onSubmit={(name) => createOrg.mutateAsync(name)}
          onClose={() => setDialog(null)}
        />
      )}
    </div>
  );
}

// ── API keys ─────────────────────────────────────────────────────────────

const SCOPES: { value: KeyScope; label: string; hint: string }[] = [
  { value: "read", label: "Read only", hint: "Query analytics and read resources." },
  { value: "write", label: "Read & write", hint: "Also create, change and delete resources." },
];

function CreateKeyDialog({ onClose }: { onClose: () => void }) {
  const queryClient = useQueryClient();
  const [created, setCreated] = useState<CreatedKey | null>(null);
  const [serverError, setServerError] = useState<{ field: string | null; message: string } | null>(null);
  const form = useForm({
    defaultValues: { name: "", scope: "read" as KeyScope },
    onSubmit: async ({ value }) => {
      setServerError(null);
      try {
        const k = await api.createKey(value.name.trim(), value.scope);
        setCreated(k);
        await queryClient.invalidateQueries({ queryKey: qk.keys });
      } catch (e) {
        setServerError({ field: e instanceof ApiError ? e.field : null, message: errorMessage(e) });
      }
    },
  });

  if (created) {
    return (
      <AppDialog
        title="Copy your new key"
        onClose={onClose}
        footer={<Button onClick={onClose}>I've stored it</Button>}
      >
        <div className="flex flex-col gap-3">
          <Notice tone="warn">This is the only time the secret is shown. Store it in your secrets manager now.</Notice>
          <div className="text-muted-foreground">
            Scope: <b className="text-foreground">{created.key.scope === "write" ? "read & write" : "read only"}</b>
          </div>
          <div className="flex items-center gap-2 rounded-lg border bg-muted/40 py-1.5 pr-1.5 pl-3">
            <code className="min-w-0 flex-1 font-mono break-all" aria-label="New API key secret">
              {created.secret}
            </code>
            <CopyButton text={created.secret} />
          </div>
        </div>
      </AppDialog>
    );
  }

  return (
    <AppDialog
      title="New personal API key"
      description="Keys act as you. Pick the narrowest scope the script needs."
      onClose={onClose}
      footer={
        <>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <form.Subscribe selector={(s) => [s.canSubmit, s.isSubmitting] as const}>
            {([canSubmit, isSubmitting]) => (
              <Button type="submit" form="create-key-form" disabled={!canSubmit || isSubmitting}>
                {isSubmitting ? "Creating…" : "Create key"}
              </Button>
            )}
          </form.Subscribe>
        </>
      }
    >
      <form
        id="create-key-form"
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          e.stopPropagation();
          void form.handleSubmit();
        }}
      >
        <form.Field
          name="name"
          validators={{
            onMount: ({ value }) => (value.trim() ? undefined : "Name is required."),
            onChange: ({ value }) => (!value.trim() ? "Name is required." : value.length > NAME_MAX ? `At most ${NAME_MAX} characters.` : undefined),
          }}
        >
          {(field) => {
            const shown =
              field.state.meta.isTouched && field.state.meta.errors.length > 0
                ? String(field.state.meta.errors[0])
                : serverError && (serverError.field === "name" || serverError.field === null)
                  ? serverError.message
                  : null;
            return (
              <FormField label="Name" htmlFor="key-name" error={shown}>
                <Input
                  id="key-name"
                  autoFocus
                  value={field.state.value}
                  placeholder="Nightly export"
                  autoComplete="off"
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
        <form.Field name="scope">
          {(field) => (
            <FormField label="Scope" htmlFor="key-scope" hint={SCOPES.find((s) => s.value === field.state.value)?.hint}>
              <Select
                value={field.state.value}
                items={SCOPES.map((s) => ({ value: s.value, label: s.label }))}
                onValueChange={(v) => field.handleChange(v === "write" ? "write" : "read")}
              >
                <SelectTrigger id="key-scope" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SCOPES.map((s) => (
                    <SelectItem key={s.value} value={s.value}>
                      {s.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </FormField>
          )}
        </form.Field>
      </form>
    </AppDialog>
  );
}

const keyCol = columnHelper<PersonalApiKey>();

function ApiKeys() {
  const queryClient = useQueryClient();
  const { data, error, isPending, refetch } = useQuery(keysQuery);
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<PersonalApiKey | null>(null);

  const revoke = useMutation({
    mutationFn: (id: string) => api.revokeKey(id),
    onSuccess: async () => {
      toast("Key revoked");
      await queryClient.invalidateQueries({ queryKey: qk.keys });
    },
  });

  const columns = useMemo(
    () => [
      keyCol.accessor("name", { header: "Name", cell: (c) => <span className="font-semibold">{c.getValue()}</span> }),
      keyCol.accessor((k) => k.scope ?? "read", {
        id: "scope",
        header: "Scope",
        cell: (c) => (c.getValue() === "write" ? <Badge className="bg-warn-wash text-warn">read &amp; write</Badge> : <Badge variant="secondary">read only</Badge>),
      }),
      keyCol.accessor("key_prefix", {
        header: "Key",
        cell: (c) => <span className="font-mono text-xs text-muted-foreground">{c.getValue()}…</span>,
        enableSorting: false,
      }),
      keyCol.accessor("created_at", { header: "Created", cell: (c) => <span className="text-muted-foreground">{fmtDate(c.getValue())}</span> }),
      keyCol.accessor((k) => k.last_used ?? 0, {
        id: "last_used",
        header: "Last used",
        cell: ({ row }) => <span className="text-muted-foreground">{row.original.last_used ? fmtRelative(row.original.last_used) : "never"}</span>,
      }),
      keyCol.display({
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        cell: ({ row }) => (
          <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" aria-label={`Revoke ${row.original.name}`} onClick={() => setRevoking(row.original)}>
            Revoke
          </Button>
        ),
        meta: { align: "right" },
      }),
    ],
    [],
  );

  return (
    <div className="flex flex-col gap-4">
      <SettingsCard
        title="Personal API keys"
        description={
          <>
            Read your data from scripts and agents: <code>Authorization: Bearer phx_…</code> against <code>/api/projects/&lt;id&gt;/…</code>. Keys act as you.
          </>
        }
        action={
          <Button onClick={() => setCreating(true)}>
            <Icon name="plus" size={14} /> Create key
          </Button>
        }
      />
      <Panel>
        {error && !data ? (
          <ErrorState error={error} retry={() => void refetch()} />
        ) : isPending ? (
          <SkeletonRows rows={3} />
        ) : data && data.length === 0 ? (
          <Empty icon={"key" satisfies IconName} title="No API keys">
            Create a key to query your analytics from scripts, notebooks or agents.
          </Empty>
        ) : (
          <DataTable label="Personal API keys" columns={columns} data={data ?? []} getRowId={(k) => k.id} sortable />
        )}
      </Panel>
      {creating && <CreateKeyDialog onClose={() => setCreating(false)} />}
      {revoking && (
        <Confirm
          title={`Revoke “${revoking.name}”?`}
          body="Anything using this key stops working immediately."
          confirmLabel="Revoke key"
          danger
          onClose={() => setRevoking(null)}
          onConfirm={() => revoke.mutateAsync(revoking.id)}
        />
      )}
    </div>
  );
}

// ── Account ──────────────────────────────────────────────────────────────

const THEMES: { value: Theme; label: string; icon: IconName }[] = [
  { value: "system", label: "System", icon: "monitor" },
  { value: "light", label: "Light", icon: "sun" },
  { value: "dark", label: "Dark", icon: "moon" },
];

function Account() {
  const { workspace, logout } = useApp();
  const theme = useTheme();
  return (
    <div className="flex flex-col gap-4">
      <SettingsCard title="Account">
        <KV
          items={[
            ["Email", workspace.user.email],
            [
              "Organizations",
              <span key="o" className="font-sans">
                {workspace.organizations.map((o) => `${o.name} (${o.role})`).join(", ")}
              </span>,
            ],
          ]}
        />
      </SettingsCard>
      <SettingsCard title="Appearance" description="System follows your OS setting.">
        <RadioGroup
          aria-label="Theme"
          value={theme}
          onValueChange={(v) => setTheme(v === "light" || v === "dark" ? v : "system")}
          className="grid-cols-3 gap-2 sm:max-w-md"
        >
          {THEMES.map((t) => (
            <Label key={t.value} htmlFor={`theme-${t.value}`} className="flex cursor-pointer items-center gap-2 rounded-lg border px-3 py-2.5 font-medium has-data-checked:border-ring has-data-checked:bg-brand-wash has-focus-visible:ring-3 has-focus-visible:ring-ring/50">
              <RadioGroupItem id={`theme-${t.value}`} value={t.value} />
              <Icon name={t.icon} size={14} className="text-muted-foreground" />
              {t.label}
            </Label>
          ))}
        </RadioGroup>
      </SettingsCard>
      <SettingsCard
        title="Sign out"
        description="Ends this browser session."
        action={
          <Button variant="outline" onClick={() => void logout()}>
            <Icon name="logout" size={14} /> Sign out
          </Button>
        }
      />
    </div>
  );
}

export function SettingsPage() {
  const { project } = useApp();
  const search = settingsRouteApi.useSearch();
  const tab: Tab = search.tab ?? "project";
  return (
    <Page narrow>
      <PageHeader title="Settings" />
      <Tabs value={tab} onValueChange={(t) => navigate(`${projectPath(project.id, "settings")}?tab=${String(t)}`, { replace: true })}>
        <TabsList variant="line" className="h-9 w-full justify-start gap-1 overflow-x-auto border-b">
          <TabsTrigger value="project" className="flex-none px-3">
            Project
          </TabsTrigger>
          <TabsTrigger value="keys" className="flex-none px-3">
            API keys
          </TabsTrigger>
          <TabsTrigger value="account" className="flex-none px-3">
            Account
          </TabsTrigger>
        </TabsList>
        <TabsContent value="project" className="pt-2">
          <ProjectSettings />
        </TabsContent>
        <TabsContent value="keys" className="pt-2">
          <ApiKeys />
        </TabsContent>
        <TabsContent value="account" className="pt-2">
          <Account />
        </TabsContent>
      </Tabs>
    </Page>
  );
}
