// The root of the route tree: decides between boot screens (loading, setup,
// login), the public share view, and the app shell.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Navigate, Outlet, useParams, useRouterState } from "@tanstack/react-router";
import { lazy, Suspense, useEffect, useMemo, type ReactNode } from "react";
import { Icon, Logo } from "@/components/icons";
import { ErrorState, Skeleton } from "@/components/feedback";
import { Page } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, errorMessage, type Workspace } from "@/lib/api";
import { AppContext, findProject, firstProject, lastProject, projectPath, rememberProject, type AppState } from "@/lib/context";
import { clearSessionData } from "@/lib/query-client";
import { bootKey, bootQuery } from "@/lib/queries";
import { navigate } from "@/lib/nav";
import { LoginPage, SetupPage } from "@/pages/Auth";
import { Empty } from "@/components/feedback";
import { Notice } from "@/components/feedback";

// Heavy shell pieces load after the first (login/boot) paint.
const Shell = lazy(() => import("@/Shell").then((m) => ({ default: m.Shell })));
const Toaster = lazy(() => import("@/components/ui/sonner").then((m) => ({ default: m.Toaster })));

function Splash() {
  return (
    <div className="grid min-h-screen place-items-center">
      <div className="flex flex-col items-center gap-3 opacity-70">
        <Logo size={40} />
        <div className="size-5 animate-spin rounded-full border-2 border-muted-foreground/30 border-t-brand motion-reduce:animate-none" role="status" aria-label="Loading" />
      </div>
    </div>
  );
}

export function PageFallback() {
  return (
    <Page>
      <div className="flex flex-col gap-4" aria-busy="true" aria-label="Loading">
        <Skeleton className="h-7 w-48" />
        <Skeleton className="h-4 w-80" />
        <Skeleton className="h-64 w-full" />
      </div>
    </Page>
  );
}

export function RouteError({ error, reset }: { error: unknown; reset: () => void }) {
  return (
    <Page narrow>
      <ErrorState error={error} retry={reset} />
    </Page>
  );
}

export function NotFound() {
  const { projectId } = useParams({ strict: false });
  return (
    <Page narrow>
      <Empty
        icon="search"
        title="Nothing lives here"
        action={
          <Button variant="outline" onClick={() => navigate(projectId ? projectPath(projectId) : "/")}>
            <Icon name="home" size={14} /> Back home
          </Button>
        }
      >
        The page you followed doesn't exist in this project.
      </Empty>
    </Page>
  );
}

function NoProjects({ workspace }: { workspace: Workspace }) {
  const org = workspace.organizations[0];
  const qc = useQueryClient();
  const create = useMutation({
    mutationFn: (name: string) => api.createProject(org.id, name.trim() || "Default project"),
    onSuccess: () => qc.invalidateQueries({ queryKey: bootKey }),
  });
  return (
    <div className="grid min-h-screen place-items-center p-6">
      <form
        className="flex w-full max-w-sm flex-col gap-4 rounded-xl bg-card p-7 ring-1 ring-foreground/10"
        onSubmit={(e) => {
          e.preventDefault();
          create.mutate(String(new FormData(e.currentTarget).get("name") ?? ""));
        }}
      >
        <Logo size={32} />
        <h1>Create a project</h1>
        <p className="text-muted-foreground">You're signed in as {workspace.user.email}, but there are no projects you can open yet.</p>
        {org ? (
          <>
            <Input name="name" defaultValue="Default project" aria-label="Project name" />
            {create.error ? <Notice tone="bad">{errorMessage(create.error)}</Notice> : null}
            <Button type="submit" disabled={create.isPending}>
              Create project in {org.name}
            </Button>
          </>
        ) : (
          <p className="text-muted-foreground">Ask an organization owner to invite you.</p>
        )}
      </form>
    </div>
  );
}

/** Where `/` and unknown paths go: the last project used, else the first. */
export function Landing() {
  const boot = useQuery(bootQuery);
  if (boot.data?.state !== "ready") return null;
  const target = findProject(boot.data.workspace, lastProject()) ?? firstProject(boot.data.workspace);
  if (!target) return null;
  return <Navigate to={projectPath(target.project.id)} replace />;
}

function Gate() {
  const boot = useQuery(bootQuery);
  if (boot.isPending) return <Splash />;
  if (boot.isError)
    return (
      <div className="grid min-h-screen place-items-center p-6">
        <div className="w-full max-w-md rounded-xl bg-card ring-1 ring-foreground/10">
          <ErrorState error={boot.error} retry={() => void boot.refetch()} />
        </div>
      </div>
    );
  const state = boot.data;
  if (state.state === "setup") return <SetupPage />;
  if (state.state === "login") return <LoginPage />;
  if (!firstProject(state.workspace)) return <NoProjects workspace={state.workspace} />;
  return <Outlet />;
}

export function Root() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const isShare = pathname.startsWith("/share/");
  return (
    <>
      {isShare ? <Outlet /> : <Gate />}
      <Suspense fallback={null}>
        <Toaster position="bottom-right" />
      </Suspense>
    </>
  );
}

/** `/project/$projectId`: resolves the project from the session, provides it, renders the shell. */
export function ProjectLayout(): ReactNode {
  const { projectId } = useParams({ from: "/project/$projectId" });
  const qc = useQueryClient();
  const boot = useQuery(bootQuery);
  const workspace = boot.data?.state === "ready" ? boot.data.workspace : null;
  const current = workspace ? findProject(workspace, projectId) : null;

  useEffect(() => {
    if (current) rememberProject(current.project.id);
  }, [current]);

  const state: AppState | null = useMemo(() => {
    if (!workspace || !current) return null;
    return {
      workspace,
      project: current.project,
      organization: current.organization,
      refreshWorkspace: async () => {
        try {
          const w = await api.me();
          qc.setQueryData(bootKey, { state: "ready", workspace: w });
          return w;
        } catch {
          return null;
        }
      },
      logout: async () => {
        try {
          await api.logout();
        } finally {
          clearSessionData();
          qc.setQueryData(bootKey, { state: "login" });
          navigate("/", { replace: true });
        }
      },
    };
  }, [workspace, current, qc]);

  if (!workspace) return null;
  if (!state) {
    const target = findProject(workspace, lastProject()) ?? firstProject(workspace);
    return target ? <Navigate to={projectPath(target.project.id)} replace /> : null;
  }
  return (
    <AppContext.Provider value={state}>
      <Suspense fallback={<Splash />}>
        <Shell>
          <Outlet />
        </Shell>
      </Suspense>
    </AppContext.Provider>
  );
}
