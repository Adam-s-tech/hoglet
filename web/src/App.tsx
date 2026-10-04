import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { ApiError, api, onUnauthorized, type Workspace } from "./lib/api";
import { AppContext, findProject, firstProject, lastProject, projectPath, rememberProject, type AppState } from "./lib/context";
import { match, navigate, useLocation } from "./lib/router";
import { Shell } from "./Shell";
import { Icon, Logo } from "./ui/icons";
import { Empty, ErrorState, Toasts } from "./ui/kit";
import { LoginPage, SetupPage } from "./pages/Auth";
import { ActivityPage } from "./pages/Activity";
import { DashboardPage, DashboardsPage } from "./pages/Dashboards";
import { FlagPage, FlagsPage } from "./pages/Flags";
import { HomePage } from "./pages/Home";
import { InsightPage } from "./pages/Insight";
import { InsightsPage } from "./pages/Insights";
import { OnboardingPage } from "./pages/Onboarding";
import { PersonPage, PersonsPage } from "./pages/Persons";
import { SettingsPage } from "./pages/Settings";
import { SharePage } from "./pages/Share";
import { WebPage } from "./pages/Web";

type Boot = { state: "loading" } | { state: "setup" } | { state: "login" } | { state: "ready"; workspace: Workspace } | { state: "error"; error: unknown };

function Splash() {
  return (
    <div className="auth">
      <div className="col" style={{ alignItems: "center", gap: 12, opacity: 0.7 }}>
        <Logo size={40} />
        <span className="spinner" />
      </div>
    </div>
  );
}

function NotFound({ home }: { home: string }) {
  return (
    <div className="page narrow">
      <Empty
        icon="search"
        title="Nothing lives here"
        action={
          <button className="btn" onClick={() => navigate(home)}>
            <Icon name="home" size={14} /> Back home
          </button>
        }
      >
        The page you followed doesn't exist in this project.
      </Empty>
    </div>
  );
}

function ProjectRoutes({ sub, projectId }: { sub: string; projectId: string }): ReactNode {
  const path = `/${sub}`;
  let m: Record<string, string> | null;
  if (path === "/" || path === "") return <HomePage />;
  if (path === "/onboarding") return <OnboardingPage />;
  if (path === "/web") return <WebPage />;
  if (path === "/insights") return <InsightsPage />;
  if (path === "/insights/new") return <InsightPage key="new" id={null} />;
  if ((m = match("/insights/:id", path))) return <InsightPage key={m.id} id={m.id} />;
  if (path === "/activity") return <ActivityPage />;
  if (path === "/persons") return <PersonsPage />;
  if ((m = match("/persons/:id", path))) return <PersonPage key={m.id} id={m.id} />;
  if (path === "/flags") return <FlagsPage />;
  if (path === "/flags/new") return <FlagPage key="new" id={null} />;
  if ((m = match("/flags/:id", path)) && /^\d+$/.test(m.id)) return <FlagPage key={m.id} id={Number(m.id)} />;
  if (path === "/dashboards") return <DashboardsPage />;
  if ((m = match("/dashboards/:id", path))) return <DashboardPage key={m.id} id={m.id} />;
  if (path === "/settings") return <SettingsPage />;
  return <NotFound home={projectPath(projectId)} />;
}

function NoProjects({ workspace, onCreated }: { workspace: Workspace; onCreated: () => void }) {
  const org = workspace.organizations[0];
  const [name, setName] = useState("Default project");
  const [error, setError] = useState<unknown>(null);
  return (
    <div className="auth">
      <div className="auth-card col gap-16">
        <Logo size={32} />
        <h1>Create a project</h1>
        <p className="secondary">You're signed in as {workspace.user.email}, but there are no projects you can open yet.</p>
        {org ? (
          <>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} aria-label="Project name" />
            {error ? <ErrorState error={error} compact /> : null}
            <button
              className="btn primary"
              onClick={async () => {
                try {
                  await api.createProject(org.id, name.trim() || "Default project");
                  onCreated();
                } catch (e) {
                  setError(e);
                }
              }}
            >
              Create project in {org.name}
            </button>
          </>
        ) : (
          <p className="muted">Ask an organization owner to invite you.</p>
        )}
      </div>
    </div>
  );
}

export function App() {
  const loc = useLocation();
  const [boot, setBoot] = useState<Boot>({ state: "loading" });

  const load = useCallback(async (): Promise<Workspace | null> => {
    try {
      const { setup_required } = await api.bootstrap();
      if (setup_required) {
        setBoot({ state: "setup" });
        return null;
      }
      const workspace = await api.me();
      setBoot({ state: "ready", workspace });
      return workspace;
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setBoot({ state: "login" });
      else setBoot({ state: "error", error: e });
      return null;
    }
  }, []);

  useEffect(() => {
    if (loc.path.startsWith("/share/")) return;
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  useEffect(() => {
    onUnauthorized(() => setBoot({ state: "login" }));
    return () => onUnauthorized(null);
  }, []);

  const workspace = boot.state === "ready" ? boot.workspace : null;
  const projectMatch = match("/project/:pid/*", loc.path) ?? match("/project/:pid", loc.path);
  const current = workspace ? findProject(workspace, projectMatch?.pid ?? null) : null;

  // Land on a real project: the last one used, else the first.
  useEffect(() => {
    if (!workspace || current) return;
    const target = findProject(workspace, lastProject()) ?? firstProject(workspace);
    if (target) navigate(projectPath(target.project.id), { replace: true });
  }, [workspace, current]);

  useEffect(() => {
    if (current) rememberProject(current.project.id);
  }, [current]);

  const appState: AppState | null = useMemo(() => {
    if (!workspace || !current) return null;
    return {
      workspace,
      project: current.project,
      organization: current.organization,
      refreshWorkspace: async () => {
        try {
          const w = await api.me();
          setBoot({ state: "ready", workspace: w });
          return w;
        } catch {
          return null;
        }
      },
      logout: async () => {
        try {
          await api.logout();
        } finally {
          setBoot({ state: "login" });
          navigate("/", { replace: true });
        }
      },
    };
  }, [workspace, current]);

  const shareMatch = match("/share/:token", loc.path);
  let content: ReactNode;
  if (shareMatch) content = <SharePage token={shareMatch.token} />;
  else if (boot.state === "loading") content = <Splash />;
  else if (boot.state === "error")
    content = (
      <div className="auth">
        <div className="auth-card">
          <ErrorState error={boot.error} retry={() => void load()} />
        </div>
      </div>
    );
  else if (boot.state === "setup")
    content = (
      <SetupPage
        onDone={(w) => {
          setBoot({ state: "ready", workspace: w });
          const first = firstProject(w);
          if (first) navigate(projectPath(first.project.id, "onboarding"), { replace: true });
        }}
      />
    );
  else if (boot.state === "login") content = <LoginPage onDone={() => void load()} />;
  else if (workspace && !firstProject(workspace)) content = <NoProjects workspace={workspace} onCreated={() => void load()} />;
  else if (!appState) content = <Splash />;
  else {
    const sub = projectMatch?.["*"] ?? "";
    content = (
      <AppContext.Provider value={appState}>
        <Shell>
          <ProjectRoutes sub={sub} projectId={appState.project.id} />
        </Shell>
      </AppContext.Provider>
    );
  }

  return (
    <>
      {content}
      <Toasts />
    </>
  );
}
