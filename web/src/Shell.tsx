// The app shell: sidebar navigation, project switcher, freshness, user menu,
// global keyboard shortcuts. On small screens the sidebar is a Sheet.

import { useQuery } from "@tanstack/react-query";
import { Link, useRouterState } from "@tanstack/react-router";
import { useEffect, useState, type ReactNode } from "react";
import { AppDialog } from "@/components/dialogs";
import { Icon, Logo, type IconName } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet";
import { ApiError } from "@/lib/api";
import { projectPath, useApp, useProjectId } from "@/lib/context";
import { fmtBytes, fmtDuration, fmtNumber, fmtRelative } from "@/lib/format";
import { isTypingTarget, useNow } from "@/lib/hooks";
import { navigate } from "@/lib/nav";
import { statusQuery } from "@/lib/queries";
import { setTheme, useTheme, type Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

const NAV: { sub: string; label: string; icon: IconName; key: string }[] = [
  { sub: "", label: "Home", icon: "home", key: "h" },
  { sub: "web", label: "Web analytics", icon: "globe", key: "w" },
  { sub: "insights", label: "Insights", icon: "trends", key: "i" },
  { sub: "activity", label: "Activity", icon: "activity", key: "a" },
  { sub: "persons", label: "Persons", icon: "users", key: "p" },
  { sub: "flags", label: "Feature flags", icon: "flag", key: "f" },
  { sub: "dashboards", label: "Dashboards", icon: "dashboard", key: "d" },
];

const THEMES: { value: Theme; label: string; icon: IconName }[] = [
  { value: "system", label: "System", icon: "monitor" },
  { value: "light", label: "Light", icon: "sun" },
  { value: "dark", label: "Dark", icon: "moon" },
];

/** Polls /status every 10s (paused while the tab is hidden): Live, behind, unreachable. */
function Freshness() {
  const projectId = useProjectId();
  const now = useNow(1000);
  const { data, error, isFetching } = useQuery(statusQuery(projectId, 10_000));

  let tone: "idle" | "live" | "behind" | "down" = "idle";
  let title = "Checking…";
  let detail: string | null = null;
  if (error && !data) {
    if (error instanceof ApiError && error.notAvailable) {
      title = "Freshness unknown";
      detail = "status not served";
    } else {
      tone = "down";
      title = "Server unreachable";
      detail = isFetching ? "retrying…" : "retrying every 10s";
    }
  } else if (data) {
    if (!data.has_events) {
      title = "No events yet";
      detail = "waiting for data";
    } else if (data.ingestion_lag_seconds < 5) {
      tone = "live";
      title = "Live";
      detail = `last event ${fmtRelative(data.last_event_at, now)}`;
    } else {
      tone = "behind";
      title = `${fmtDuration(data.ingestion_lag_seconds)} behind`;
      detail = "ingestion is catching up";
    }
  }

  return (
    <Popover>
      <PopoverTrigger
        aria-label={`Data freshness: ${title}`}
        className="flex w-full items-center gap-2 rounded-lg bg-card px-2.5 py-1.5 text-left text-xs text-muted-foreground ring-1 ring-foreground/10 outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
      >
        <span
          className={cn(
            "size-2 flex-none rounded-full",
            tone === "idle" && "bg-muted-foreground",
            tone === "live" && "bg-good motion-safe:animate-pulse",
            tone === "behind" && "bg-warn",
            tone === "down" && "bg-destructive",
          )}
        />
        <span className="flex min-w-0 flex-col">
          <b className="font-semibold text-foreground">{title}</b>
          {detail ? <span className="truncate">{detail}</span> : null}
        </span>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="w-72">
        <b>Data freshness</b>
        <p className="text-xs text-muted-foreground">
          Numbers include every event acknowledged up to the lag shown. If ingestion falls behind, this says so; nothing is silently missing.
        </p>
        {data ? (
          <dl className="grid grid-cols-[1fr_auto] gap-x-3 gap-y-1 font-mono text-xs">
            <dt className="text-muted-foreground">ingestion lag</dt>
            <dd className="text-right">{data.ingestion_lag_seconds < 1 ? "< 1s" : fmtDuration(data.ingestion_lag_seconds)}</dd>
            <dt className="text-muted-foreground">last event</dt>
            <dd className="text-right">{fmtRelative(data.last_event_at, now)}</dd>
            <dt className="text-muted-foreground">stored events</dt>
            <dd className="text-right">{fmtNumber(data.stored_events)}</dd>
            <dt className="text-muted-foreground">stored size</dt>
            <dd className="text-right">{fmtBytes(data.stored_bytes)}</dd>
          </dl>
        ) : null}
      </PopoverContent>
    </Popover>
  );
}

function ProjectMark({ name }: { name: string }) {
  return <span className="grid size-6 flex-none place-items-center rounded-md bg-brand text-[10px] font-bold text-white">{name.slice(0, 2).toUpperCase()}</span>;
}

function ProjectSwitcher({ onNavigate }: { onNavigate?: () => void }) {
  const { workspace, project, organization } = useApp();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={`Project: ${project.name}. Switch project`}
        className="flex h-10 w-full items-center gap-2 rounded-lg bg-card px-2 text-left ring-1 ring-foreground/10 outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring aria-expanded:bg-accent"
      >
        <ProjectMark name={project.name} />
        <span className="flex min-w-0 flex-1 flex-col leading-tight">
          <b className="truncate text-[13px]">{project.name}</b>
          <span className="truncate text-[11.5px] text-muted-foreground">{organization.name}</span>
        </span>
        <Icon name="chevronUpDown" size={14} className="text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-64">
        {workspace.organizations.map((o) => (
          <DropdownMenuGroup key={o.id}>
            <DropdownMenuLabel>{o.name}</DropdownMenuLabel>
            <DropdownMenuRadioGroup
              value={project.id}
              onValueChange={(id) => {
                onNavigate?.();
                navigate(projectPath(id));
              }}
            >
              {o.projects.map((p) => (
                <DropdownMenuRadioItem key={p.id} value={p.id}>
                  <ProjectMark name={p.name} />
                  <span className="truncate">{p.name}</span>
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuGroup>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => {
            onNavigate?.();
            navigate(`${projectPath(project.id, "settings")}?tab=project`);
          }}
        >
          <Icon name="plus" size={14} /> New project
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function UserMenu({ onNavigate }: { onNavigate?: () => void }) {
  const { workspace, logout, project } = useApp();
  const theme = useTheme();
  const email = workspace.user.email;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={`Account menu for ${email}`}
        className="flex h-9 w-full items-center gap-2 rounded-lg px-2 text-left font-medium outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring aria-expanded:bg-accent"
      >
        <span className="grid size-[22px] flex-none place-items-center rounded-full bg-brand text-[10px] font-semibold text-white" aria-hidden="true">
          {email.slice(0, 1).toUpperCase()}
        </span>
        <span className="truncate">{email}</span>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" className="w-60">
        <DropdownMenuGroup>
          <DropdownMenuLabel>Theme</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={theme} onValueChange={(v) => setTheme(v as Theme)}>
            {THEMES.map((t) => (
              <DropdownMenuRadioItem key={t.value} value={t.value}>
                <Icon name={t.icon} size={14} /> {t.label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => {
            onNavigate?.();
            navigate(`${projectPath(project.id, "settings")}?tab=keys`);
          }}
        >
          <Icon name="key" size={14} /> Personal API keys
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => void logout()}>
          <Icon name="logout" size={14} /> Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Shortcuts({ onClose }: { onClose: () => void }) {
  const rows: [string, ReactNode][] = [
    ...NAV.map((n): [string, ReactNode] => [
      `Go to ${n.label}`,
      <>
        <Kbd>g</Kbd> <Kbd>{n.key}</Kbd>
      </>,
    ]),
    [
      "Go to Settings",
      <>
        <Kbd>g</Kbd> <Kbd>s</Kbd>
      </>,
    ],
    ["Focus search", <Kbd key="slash">/</Kbd>],
    [
      "Save insight",
      <>
        <Kbd>Ctrl</Kbd> <Kbd>S</Kbd>
      </>,
    ],
    [
      "Run SQL",
      <>
        <Kbd>Ctrl</Kbd> <Kbd>Enter</Kbd>
      </>,
    ],
    ["Show this help", <Kbd key="help">?</Kbd>],
  ];
  return (
    <AppDialog title="Keyboard shortcuts" onClose={onClose}>
      <dl className="grid grid-cols-[1fr_auto] items-center gap-x-6 gap-y-2.5">
        {rows.map(([label, keys]) => (
          <div key={label} className="contents">
            <dt>{label}</dt>
            <dd className="m-0 flex items-center gap-1 justify-self-end">{keys}</dd>
          </div>
        ))}
      </dl>
    </AppDialog>
  );
}

function SidebarBody({ onNavigate }: { onNavigate?: () => void }) {
  const projectId = useProjectId();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const base = projectPath(projectId);
  const sub = pathname.startsWith(base) ? pathname.slice(base.length).replace(/^\//, "") : "";
  const section = sub.split("/")[0];

  const item = (n: { sub: string; label: string; icon: IconName; title?: string }) => {
    const active = section === n.sub;
    return (
      <Link
        key={n.sub}
        to={projectPath(projectId, n.sub)}
        onClick={onNavigate}
        aria-current={active ? "page" : undefined}
        title={n.title}
        className={cn(
          "flex h-8 items-center gap-2.5 rounded-md px-2.5 font-medium text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring",
          active && "bg-card text-foreground ring-1 ring-foreground/10 hover:bg-card",
        )}
      >
        <Icon name={n.icon} className={active ? "text-brand" : undefined} />
        {n.label}
      </Link>
    );
  };

  return (
    <div className="flex h-full min-h-0 flex-col gap-1 p-2.5 pt-3.5">
      <Link to={base} onClick={onNavigate} className="flex items-center gap-2 rounded-md px-2 pb-2.5 text-[15px] font-semibold tracking-tight outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <Logo size={26} />
        Hoglet
      </Link>
      <ProjectSwitcher onNavigate={onNavigate} />
      <nav aria-label="Project" className="mt-1.5 flex flex-col gap-px">
        {NAV.map((n) => item({ ...n, title: `${n.label} (g ${n.key})` }))}
        <div className="mx-2.5 mt-3.5 mb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">Project</div>
        {item({ sub: "onboarding", label: "Connect your app", icon: "terminal" })}
        {item({ sub: "settings", label: "Settings", icon: "settings", title: "Settings (g s)" })}
      </nav>
      <div className="mt-auto flex flex-col gap-1.5 pt-2.5">
        <Freshness />
        <UserMenu onNavigate={onNavigate} />
      </div>
    </div>
  );
}

export function Shell({ children }: { children: ReactNode }) {
  const projectId = useProjectId();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const [menuOpen, setMenuOpen] = useState(false);
  const [help, setHelp] = useState(false);

  useEffect(() => setMenuOpen(false), [pathname]);

  useEffect(() => {
    let pendingG = 0;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isTypingTarget(e.target)) return;
      // A dialog, menu or listbox is open: it owns the keyboard.
      if (document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]')) return;
      if (e.key === "/") {
        const input = document.querySelector<HTMLInputElement>("main input[type=search], main [data-search] input");
        if (input) {
          e.preventDefault();
          input.focus();
        }
        return;
      }
      if (e.key === "?") {
        setHelp(true);
        return;
      }
      if (e.key === "g") {
        pendingG = Date.now();
        return;
      }
      if (Date.now() - pendingG < 1200) {
        pendingG = 0;
        const target = e.key === "s" ? "settings" : NAV.find((n) => n.key === e.key)?.sub;
        if (target !== undefined) {
          e.preventDefault();
          navigate(projectPath(projectId, target));
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [projectId]);

  return (
    <div className="grid min-h-screen grid-cols-1 md:grid-cols-[232px_minmax(0,1fr)]">
      <a
        href="#main"
        className="sr-only z-50 rounded-md bg-primary px-3 py-2 text-primary-foreground focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
        onClick={(e) => {
          e.preventDefault();
          document.getElementById("main")?.focus();
        }}
      >
        Skip to content
      </a>
      <div className="sticky top-0 z-30 flex items-center gap-2.5 border-b bg-background px-4 py-2.5 md:hidden">
        <Button variant="ghost" size="icon" aria-label="Open navigation" onClick={() => setMenuOpen(true)}>
          <Icon name="menu" />
        </Button>
        <Logo size={22} />
        <b>Hoglet</b>
      </div>
      <aside aria-label="Main navigation" className="sticky top-0 hidden h-screen border-r border-sidebar-border bg-sidebar text-sidebar-foreground md:block">
        <SidebarBody />
      </aside>
      <Sheet open={menuOpen} onOpenChange={setMenuOpen}>
        <SheetContent side="left" showCloseButton={false} className="w-64 bg-sidebar p-0 md:hidden">
          <SheetTitle className="sr-only">Navigation</SheetTitle>
          <SheetDescription className="sr-only">Project navigation</SheetDescription>
          <SidebarBody onNavigate={() => setMenuOpen(false)} />
        </SheetContent>
      </Sheet>
      <main id="main" tabIndex={-1} className="flex min-w-0 flex-col outline-none">
        <TooltipProvider delay={300}>{children}</TooltipProvider>
      </main>
      {help && <Shortcuts onClose={() => setHelp(false)} />}
    </div>
  );
}
