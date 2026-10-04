// First-run setup and sign-in. Both end by writing the session into the boot
// query, which swaps the screen for the app.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import type { ReactNode } from "react";
import { Icon, Logo } from "@/components/icons";
import { Notice } from "@/components/feedback";
import { FormField } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ApiError, api, errorMessage, isMock, type Workspace } from "@/lib/api";
import { firstProject, projectPath } from "@/lib/context";
import { navigate } from "@/lib/nav";
import { bootKey, bootQuery } from "@/lib/queries";
import { clearSessionData } from "@/lib/query-client";

function AuthCard({ title, lede, foot, onSubmit, children }: { title: string; lede: string; foot: string; onSubmit: () => void; children: ReactNode }) {
  return (
    <div className="grid min-h-screen place-items-center p-6">
      <form
        noValidate
        className="flex w-full max-w-[420px] flex-col gap-1 rounded-xl bg-card p-8 shadow-sm ring-1 ring-foreground/10"
        onSubmit={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onSubmit();
        }}
      >
        <div className="mb-3 flex items-center gap-2.5">
          <Logo size={32} />
          <b className="text-[17px]">Hoglet</b>
        </div>
        <h1>{title}</h1>
        <p className="mb-4 text-muted-foreground">{lede}</p>
        <div className="flex flex-col gap-4">{children}</div>
        <p className="mt-5 text-center text-xs text-muted-foreground">{foot}</p>
      </form>
    </div>
  );
}

/** Server-side field errors (ApiError.field) shown under the matching input. */
function serverFieldError(error: unknown, field: string): string | undefined {
  return error instanceof ApiError && error.field === field ? "This field is invalid." : undefined;
}

function useSession() {
  const qc = useQueryClient();
  return (workspace: Workspace) => {
    clearSessionData();
    qc.setQueryData(bootKey, { state: "ready", workspace });
  };
}

export function SetupPage() {
  const start = useSession();
  const setup = useMutation({
    mutationFn: (v: { email: string; password: string; org: string; project: string }) =>
      api.setup({ email: v.email.trim(), password: v.password, organization_name: v.org.trim(), project_name: v.project.trim() || "Default project" }),
    onSuccess: (workspace) => {
      start(workspace);
      const first = firstProject(workspace);
      if (first) navigate(projectPath(first.project.id, "onboarding"), { replace: true });
    },
  });
  const form = useForm({
    defaultValues: { email: "", password: "", org: "", project: "Default project" },
    onSubmit: async ({ value }) => {
      await setup.mutateAsync(value).catch(() => undefined);
    },
  });
  const required = ({ value }: { value: string }) => (value.trim() ? undefined : "Required.");

  return (
    <AuthCard
      title="Set up your Hoglet"
      lede="One owner account, one organization, one project. Takes a minute; you can add more later."
      foot="Your data stays on this server. Nothing is sent anywhere else."
      onSubmit={() => void form.handleSubmit()}
    >
      <form.Field
        name="email"
        validators={{ onBlur: ({ value }) => (/^\S+@\S+\.\S+$/.test(value.trim()) ? undefined : "Enter a valid email address.") }}
      >
        {(f) => (
          <FormField label="Your email" htmlFor="setup-email" error={f.state.meta.errors[0] ? String(f.state.meta.errors[0]) : serverFieldError(setup.error, "email")}>
            <Input
              id="setup-email"
              type="email"
              autoComplete="email"
              autoFocus
              required
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
              onBlur={f.handleBlur}
              aria-invalid={f.state.meta.errors.length > 0}
            />
          </FormField>
        )}
      </form.Field>
      <form.Field name="password" validators={{ onChange: ({ value }) => (value.length > 0 && value.length < 8 ? "At least 8 characters." : undefined) }}>
        {(f) => (
          <FormField
            label="Password"
            htmlFor="setup-password"
            hint="At least 8 characters."
            error={f.state.meta.errors[0] ? String(f.state.meta.errors[0]) : undefined}
          >
            <Input
              id="setup-password"
              type="password"
              autoComplete="new-password"
              required
              minLength={8}
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
              onBlur={f.handleBlur}
              aria-invalid={f.state.meta.errors.length > 0}
            />
          </FormField>
        )}
      </form.Field>
      <div className="flex items-start gap-3">
        <form.Field name="org" validators={{ onBlur: required }}>
          {(f) => (
            <FormField className="min-w-0 flex-1" label="Organization" htmlFor="setup-org" error={f.state.meta.errors[0] ? String(f.state.meta.errors[0]) : serverFieldError(setup.error, "organization_name")}>
              <Input id="setup-org" required placeholder="Acme Inc." value={f.state.value} onChange={(e) => f.handleChange(e.target.value)} onBlur={f.handleBlur} aria-invalid={f.state.meta.errors.length > 0} />
            </FormField>
          )}
        </form.Field>
        <form.Field name="project">
          {(f) => (
            <FormField className="min-w-0 flex-1" label="First project" htmlFor="setup-project" error={serverFieldError(setup.error, "project_name")}>
              <Input id="setup-project" required value={f.state.value} onChange={(e) => f.handleChange(e.target.value)} onBlur={f.handleBlur} />
            </FormField>
          )}
        </form.Field>
      </div>
      {setup.error ? <Notice tone="bad">{errorMessage(setup.error)}</Notice> : null}
      <form.Subscribe selector={(s) => [s.canSubmit, s.values.password] as const}>
        {([canSubmit, password]) => (
          <Button type="submit" size="lg" className="h-10 text-[15px]" disabled={setup.isPending || !canSubmit || password.length < 8}>
            {setup.isPending ? "Creating…" : "Create workspace"}
            {!setup.isPending && <Icon name="arrowRight" size={14} />}
          </Button>
        )}
      </form.Subscribe>
    </AuthCard>
  );
}

export function LoginPage() {
  const start = useSession();
  const login = useMutation({
    mutationFn: (v: { email: string; password: string }) => api.login(v.email.trim(), v.password),
    onSuccess: start,
  });
  const form = useForm({
    defaultValues: { email: isMock ? "demo@hoglet.dev" : "", password: isMock ? "hoglet-demo" : "" },
    onSubmit: async ({ value }) => {
      await login.mutateAsync(value).catch(() => undefined);
    },
  });
  const bad = login.error instanceof ApiError && login.error.status === 401;
  const boot = useQuery(bootQuery).data;
  const expired = boot?.state === "login" && boot.expired === true;

  return (
    <AuthCard title="Sign in" lede="Product analytics, on your own server." foot="Sessions last 7 days on this browser." onSubmit={() => void form.handleSubmit()}>
      <form.Field name="email">
        {(f) => (
          <FormField label="Email" htmlFor="login-email">
            <Input id="login-email" type="email" autoComplete="email" autoFocus required value={f.state.value} onChange={(e) => f.handleChange(e.target.value)} onBlur={f.handleBlur} />
          </FormField>
        )}
      </form.Field>
      <form.Field name="password">
        {(f) => (
          <FormField label="Password" htmlFor="login-password">
            <Input
              id="login-password"
              type="password"
              autoComplete="current-password"
              required
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
              onBlur={f.handleBlur}
            />
          </FormField>
        )}
      </form.Field>
      {expired && !login.error ? <Notice tone="warn">Your session ended. Sign in again to pick up where you left off.</Notice> : null}
      {login.error ? <Notice tone="bad">{bad ? "That email and password don't match." : errorMessage(login.error)}</Notice> : null}
      <Button type="submit" size="lg" className="h-10 text-[15px]" disabled={login.isPending}>
        {login.isPending ? "Signing in…" : "Sign in"}
      </Button>
    </AuthCard>
  );
}
