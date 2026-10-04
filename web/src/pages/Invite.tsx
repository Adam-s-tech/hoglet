// /invite/$token: the page an invite link opens, with no session. The token is
// the credential; it is sent in request bodies only. A new address picks a
// name and password; an address that already has an account signs in with its
// password. Either way the response is a session, like the login form.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { useParams } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { Icon, Logo } from "@/components/icons";
import { Notice, Skeleton } from "@/components/feedback";
import { FormField } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ApiError, api, errorMessage, type InvitePreview, type Workspace } from "@/lib/api";
import { firstProject, projectPath } from "@/lib/context";
import { navigate } from "@/lib/nav";
import { bootKey, invitePreviewQuery } from "@/lib/queries";
import { clearSessionData } from "@/lib/query-client";

const MIN_PASSWORD = 12;

function Card({ children }: { children: ReactNode }) {
  return (
    <main className="grid min-h-screen place-items-center p-6">
      <div className="flex w-full max-w-[420px] flex-col gap-1 rounded-xl bg-card p-8 shadow-sm ring-1 ring-foreground/10">
        <div className="mb-3 flex items-center gap-2.5">
          <Logo size={32} />
          <b className="text-[17px]">Hoglet</b>
        </div>
        {children}
      </div>
    </main>
  );
}

function AcceptForm({ token, preview }: { token: string; preview: InvitePreview }) {
  const qc = useQueryClient();
  const accept = useMutation({
    mutationFn: (v: { name: string; password: string }) => api.acceptInvite({ token, name: preview.account_exists ? null : v.name.trim(), password: v.password }),
    onSuccess: (workspace: Workspace) => {
      clearSessionData();
      qc.setQueryData(bootKey, { state: "ready", workspace });
      const joined = workspace.organizations.find((o) => o.name === preview.organization_name && o.projects.length > 0);
      const target = joined ? { project: joined.projects[0] } : firstProject(workspace);
      navigate(target ? projectPath(target.project.id) : "/", { replace: true });
    },
  });
  const form = useForm({
    defaultValues: { name: "", password: "" },
    onSubmit: async ({ value }) => {
      await accept.mutateAsync(value).catch(() => undefined);
    },
  });
  const existing = preview.account_exists;
  const failure = accept.error;
  const message =
    failure instanceof ApiError && failure.status === 401
      ? "That password doesn't match this account."
      : failure instanceof ApiError && failure.status === 429
        ? "Too many attempts. Wait a minute, then try again."
        : failure
          ? errorMessage(failure)
          : null;

  return (
    <Card>
      <h1>Join {preview.organization_name}</h1>
      <p className="mb-4 text-muted-foreground">
        You're invited as <b className="text-foreground">{preview.role}</b>
        {preview.role === "member" ? " (read-only)" : ""}. {existing ? "Sign in with your Hoglet password to accept." : "Choose a name and password to create your account."}
      </p>
      <form
        noValidate
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          e.stopPropagation();
          void form.handleSubmit();
        }}
      >
        <FormField label="Email" htmlFor="invite-email">
          <Input id="invite-email" type="email" value={preview.email} readOnly autoComplete="username" />
        </FormField>
        {!existing && (
          <form.Field name="name" validators={{ onChange: ({ value }) => (value.trim() ? undefined : "Enter your name.") }}>
            {(f) => (
              <FormField label="Your name" htmlFor="invite-name" error={f.state.meta.isTouched && f.state.meta.errors[0] ? String(f.state.meta.errors[0]) : undefined}>
                <Input id="invite-name" autoFocus autoComplete="name" required value={f.state.value} onChange={(e) => f.handleChange(e.target.value)} onBlur={f.handleBlur} />
              </FormField>
            )}
          </form.Field>
        )}
        <form.Field name="password" validators={{ onChange: ({ value }) => (!existing && value.length > 0 && value.length < MIN_PASSWORD ? `At least ${MIN_PASSWORD} characters.` : undefined) }}>
          {(f) => (
            <FormField
              label={existing ? "Your password" : "Password"}
              htmlFor="invite-password"
              hint={existing ? undefined : `At least ${MIN_PASSWORD} characters.`}
              error={f.state.meta.errors[0] ? String(f.state.meta.errors[0]) : undefined}
            >
              <Input
                id="invite-password"
                type="password"
                autoFocus={existing}
                autoComplete={existing ? "current-password" : "new-password"}
                required
                value={f.state.value}
                onChange={(e) => f.handleChange(e.target.value)}
                onBlur={f.handleBlur}
                aria-invalid={f.state.meta.errors.length > 0}
              />
            </FormField>
          )}
        </form.Field>
        {message ? <Notice tone="bad">{message}</Notice> : null}
        <form.Subscribe selector={(s) => [s.values.name, s.values.password] as const}>
          {([name, password]) => (
            <Button type="submit" size="lg" className="h-10 text-[15px]" disabled={accept.isPending || !password || (!existing && (password.length < MIN_PASSWORD || !name.trim()))}>
              {accept.isPending ? "Joining…" : existing ? "Sign in and join" : "Create account and join"}
              {!accept.isPending && <Icon name="arrowRight" size={14} />}
            </Button>
          )}
        </form.Subscribe>
      </form>
      <p className="mt-5 text-center text-xs text-muted-foreground">This link works once and expires on its own.</p>
    </Card>
  );
}

export function InvitePage() {
  const { token } = useParams({ from: "/invite/$token" });
  const preview = useQuery(invitePreviewQuery(token));
  if (preview.isPending) {
    return (
      <Card>
        <Skeleton className="h-6 w-48" />
        <Skeleton className="mt-3 h-4 w-full" />
        <Skeleton className="mt-6 h-10 w-full" />
      </Card>
    );
  }
  if (preview.error || !preview.data) {
    const gone = preview.error instanceof ApiError && preview.error.status === 404;
    const throttled = preview.error instanceof ApiError && preview.error.status === 429;
    return (
      <Card>
        <h1>{gone ? "This invite link doesn't work" : throttled ? "Slow down a moment" : "Couldn't open this invite"}</h1>
        <p className="mb-4 text-muted-foreground">
          {gone
            ? "It may have expired, been used already, or been replaced by a newer link. Ask the person who invited you for a new one."
            : throttled
              ? "Too many attempts from this address. Wait a minute and reload."
              : errorMessage(preview.error)}
        </p>
        <Button variant="outline" onClick={() => navigate("/", { replace: true })}>
          Go to Hoglet
        </Button>
      </Card>
    );
  }
  return <AcceptForm token={token} preview={preview.data} />;
}
