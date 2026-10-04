// The app shell: sidebar navigation, project switcher, freshness, user menu.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError, api } from "./lib/api";
import { projectPath, useApp, useProjectId } from "./lib/context";
import { fmtBytes, fmtDuration, fmtNumber, fmtRelative } from "./lib/format";
import { isTypingTarget, useApi, useNow } from "./lib/hooks";
import { Link, navigate, useLocation } from "./lib/router";
import { Icon, Logo, type IconName } from "./ui/icons";
import { Modal, Popover } from "./ui/kit";
import { setTheme, useTheme } from "./ui/theme";

const NAV: { sub: string; label: string; icon: IconName; key: string }[] = [
  { sub: "", label: "Home", icon: "home", key: "h" },
  { sub: "web", label: "Web analytics", icon: "globe", key: "w" },
  { sub: "insights", label: "Insights", icon: "trends", key: "i" },
  { sub: "activity", label: "Activity", icon: "activity", key: "a" },
  { sub: "persons", label: "Persons", icon: "users", key: "p" },
  { sub: "flags", label: "Feature flags", icon: "flag", key: "f" },
  { sub: "dashboards", label: "Dashboards", icon: "dashboard", key: "d" },
];

function Freshness() {
  const projectId = useProjectId();
  const now = useNow(1000);
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const { data, error, loading } = useApi(`status:${projectId}`, (s) => api.status(projectId, s), { pollMs: 10_000 });

  let dot = "dot";
  let title: ReactNode = "Checking…";
  let detail: ReactNode = null;
  if (error && !data) {
    if (error instanceof ApiError && error.notAvailable) {
      title = "Freshness unknown";
      detail = "status not served";
    } else {
      dot = "dot down";
      title = "Server unreachable";
      detail = loading ? "retrying…" : "retrying every 10s";
    }
  } else if (data) {
    if (!data.has_events) {
      title = "No events yet";
      detail = "waiting for data";
    } else if (data.ingestion_lag_seconds < 5) {
      dot = "dot live";
      title = "Live";
      detail = `last event ${fmtRelative(data.last_event_at, now)}`;
    } else {
      dot = "dot behind";
      title = `${fmtDuration(data.ingestion_lag_seconds)} behind`;
      detail = "ingestion is catching up";
    }
  }
  return (
    <>
      <button ref={ref} className="fresh" onClick={() => setOpen((o) => !o)} aria-label={`Data freshness: ${typeof title === "string" ? title : ""}`} aria-expanded={open}>
        <span className={dot} />
        <span className="col" style={{ gap: 0, minWidth: 0 }}>
          <b>{title}</b>
          {detail && <span className="truncate">{detail}</span>}
        </span>
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)} className="pad" width={280}>
        <div className="col" style={{ gap: 8 }}>
          <b>Data freshness</b>
          <p className="small secondary">
            Numbers include every event acknowledged up to the lag shown. If ingestion falls behind, this says so; nothing is silently missing.
          </p>
          {data && (
            <div className="kv" style={{ gridTemplateColumns: "1fr auto" }}>
              <div>ingestion lag</div>
              <div>{data.ingestion_lag_seconds < 1 ? "< 1s" : fmtDuration(data.ingestion_lag_seconds)}</div>
              <div>last event</div>
              <div>{fmtRelative(data.last_event_at, now)}</div>
              <div>stored events</div>
              <div>{fmtNumber(data.stored_events)}</div>
              <div>stored size</div>
              <div>{fmtBytes(data.stored_bytes)}</div>
            </div>
          )}
        </div>
      </Popover>
    </>
  );
}

function ProjectSwitcher() {
  const { workspace, project, organization } = useApp();
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button ref={ref} className="switcher" onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open}>
        <span className="project-mark">{project.name.slice(0, 2).toUpperCase()}</span>
        <span className="col grow" style={{ gap: 0, minWidth: 0 }}>
          <b className="truncate" style={{ fontSize: 13 }}>
            {project.name}
          </b>
          <span className="truncate muted" style={{ fontSize: 11.5 }}>
            {organization.name}
          </span>
        </span>
        <Icon name="chevronUpDown" size={14} style={{ color: "var(--ink-3)" }} />
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)} width={260}>
        {workspace.organizations.map((o) => (
          <div key={o.id}>
            <div className="menu-label">{o.name}</div>
            {o.projects.map((p) => (
              <button
                key={p.id}
                className="menu-item"
                aria-selected={p.id === project.id}
                onClick={() => {
                  setOpen(false);
                  navigate(projectPath(p.id));
                }}
              >
                <span className="project-mark">{p.name.slice(0, 2).toUpperCase()}</span>
                <span className="truncate">{p.name}</span>
                {p.id === project.id && <Icon name="check" size={14} style={{ marginLeft: "auto" }} />}
              </button>
            ))}
          </div>
        ))}
        <div className="menu-sep" />
        <button
          className="menu-item"
          onClick={() => {
            setOpen(false);
            navigate(`${projectPath(project.id, "settings")}?tab=project`);
          }}
        >
          <Icon name="plus" size={14} /> New project
        </button>
      </Popover>
    </>
  );
}

function UserMenu() {
  const { workspace, logout, project } = useApp();
  const theme = useTheme();
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const email = workspace.user.email;
  return (
    <>
      <button ref={ref} className="btn ghost" style={{ justifyContent: "flex-start", width: "100%", height: 36 }} onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open}>
        <span className="avatar" style={{ background: "var(--accent)", width: 22, height: 22, fontSize: 10 }}>
          {email.slice(0, 1).toUpperCase()}
        </span>
        <span className="truncate" style={{ fontWeight: 500 }}>
          {email}
        </span>
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)} width={240}>
        <div className="menu-label">Theme</div>
        {(
          [
            ["system", "System", "monitor"],
            ["light", "Light", "sun"],
            ["dark", "Dark", "moon"],
          ] as const
        ).map(([value, label, icon]) => (
          <button key={value} className="menu-item" aria-selected={theme === value} onClick={() => setTheme(value)}>
            <Icon name={icon} size={14} /> {label}
            {theme === value && <Icon name="check" size={14} style={{ marginLeft: "auto" }} />}
          </button>
        ))}
        <div className="menu-sep" />
        <button
          className="menu-item"
          onClick={() => {
            setOpen(false);
            navigate(`${projectPath(project.id, "settings")}?tab=keys`);
          }}
        >
          <Icon name="key" size={14} /> Personal API keys
        </button>
        <button className="menu-item" onClick={() => void logout()}>
          <Icon name="logout" size={14} /> Sign out
        </button>
      </Popover>
    </>
  );
}

function Shortcuts({ onClose }: { onClose: () => void }) {
  return (
    <Modal title="Keyboard shortcuts" onClose={onClose}>
      <div className="kv" style={{ gridTemplateColumns: "1fr auto" }}>
        {NAV.map((n) => (
          <div key={n.key} style={{ display: "contents" }}>
            <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>Go to {n.label}</div>
            <div>
              <span className="kbd">g</span> <span className="kbd">{n.key}</span>
            </div>
          </div>
        ))}
        <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>Go to Settings</div>
        <div>
          <span className="kbd">g</span> <span className="kbd">s</span>
        </div>
        <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>Focus search</div>
        <div>
          <span className="kbd">/</span>
        </div>
        <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>Save insight</div>
        <div>
          <span className="kbd">Ctrl</span> <span className="kbd">S</span>
        </div>
        <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>Run SQL</div>
        <div>
          <span className="kbd">Ctrl</span> <span className="kbd">Enter</span>
        </div>
      </div>
    </Modal>
  );
}

export function Shell({ children }: { children: ReactNode }) {
  const projectId = useProjectId();
  const loc = useLocation();
  const [menuOpen, setMenuOpen] = useState(false);
  const [help, setHelp] = useState(false);
  const base = projectPath(projectId);
  const sub = loc.path.startsWith(base) ? loc.path.slice(base.length).replace(/^\//, "") : "";
  const section = sub.split("/")[0];

  useEffect(() => setMenuOpen(false), [loc.path]);

  useEffect(() => {
    let pendingG = 0;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isTypingTarget(e.target) || document.querySelector(".overlay")) return;
      if (e.key === "/") {
        const input = document.querySelector<HTMLInputElement>(".page .search input, .page input[type=search]");
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
    <div className="app">
      <div className="mobile-bar">
        <button className="btn ghost icon" aria-label="Open navigation" onClick={() => setMenuOpen(true)}>
          <Icon name="menu" />
        </button>
        <Logo size={22} />
        <b>Hoglet</b>
      </div>
      <aside className={`sidebar${menuOpen ? " open" : ""}`} aria-label="Main navigation">
        <Link to={base} className="brand">
          <Logo size={26} />
          Hoglet
        </Link>
        <ProjectSwitcher />
        <nav className="nav">
          {NAV.map((n) => (
            <Link key={n.sub} to={projectPath(projectId, n.sub)} aria-current={section === n.sub ? "page" : undefined} title={`${n.label} (g ${n.key})`}>
              <Icon name={n.icon} />
              {n.label}
            </Link>
          ))}
          <div className="nav-section">Project</div>
          <Link to={projectPath(projectId, "onboarding")} aria-current={section === "onboarding" ? "page" : undefined}>
            <Icon name="terminal" />
            Connect your app
          </Link>
          <Link to={projectPath(projectId, "settings")} aria-current={section === "settings" ? "page" : undefined} title="Settings (g s)">
            <Icon name="settings" />
            Settings
          </Link>
        </nav>
        <div className="sidebar-foot">
          <Freshness />
          <UserMenu />
        </div>
      </aside>
      {menuOpen && <div className="overlay" style={{ zIndex: 35, padding: 0 }} onClick={() => setMenuOpen(false)} />}
      <main className="main" id="main">
        {children}
      </main>
      {help && <Shortcuts onClose={() => setHelp(false)} />}
    </div>
  );
}
