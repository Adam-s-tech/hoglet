import { useState } from "react";
import { api, errorMessage, type CreatedKey, type KeyScope } from "../lib/api";
import { canEdit, projectPath, useApp, useProjectId } from "../lib/context";
import { fmtBytes, fmtDate, fmtNumber, fmtRelative } from "../lib/format";
import { invalidate, useApi } from "../lib/hooks";
import { navigate, useLocation } from "../lib/router";
import { Icon } from "../ui/icons";
import { Confirm, CopyButton, Empty, ErrorState, Modal, Seg, SkeletonRows, Tabs, toast } from "../ui/kit";
import { SnippetTabs, TokenBox } from "./Onboarding";
import { setTheme, useTheme, type Theme } from "../ui/theme";

function ProjectSettings() {
  const { project, organization, refreshWorkspace } = useApp();
  const projectId = useProjectId();
  const status = useApi(`status:${projectId}`, (s) => api.status(projectId, s));
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const create = async () => {
    try {
      const p = await api.createProject(organization.id, name.trim() || "New project");
      await refreshWorkspace();
      toast(`Created ${p.name}`);
      navigate(projectPath(p.id, "onboarding"));
    } catch (e) {
      toast(errorMessage(e), true);
    }
  };
  return (
    <div className="col gap-16">
      <div className="card card-pad col gap-12">
        <h2>Project</h2>
        <div className="kv" style={{ gridTemplateColumns: "180px 1fr" }}>
          <div>Name</div>
          <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>{project.name}</div>
          <div>Project ID</div>
          <div>{project.id}</div>
          <div>Organization</div>
          <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>
            {organization.name} <span className="badge">{organization.role}</span>
          </div>
          {status.data && (
            <>
              <div>Stored events</div>
              <div>{fmtNumber(status.data.stored_events)}</div>
              <div>Storage used</div>
              <div>{fmtBytes(status.data.stored_bytes)}</div>
              <div>Data range</div>
              <div>{status.data.first_day ? `${fmtDate(status.data.first_day)} – ${fmtDate(status.data.last_day)}` : "–"}</div>
            </>
          )}
        </div>
      </div>
      <div className="card card-pad col gap-12">
        <h2>Project token</h2>
        <p className="secondary">Used by SDKs to send events. Safe to ship in client code; it can't read data.</p>
        <TokenBox token={project.token} />
      </div>
      <div className="card card-pad col gap-12">
        <h2>Install snippets</h2>
        <SnippetTabs token={project.token} />
      </div>
      {canEdit(organization) && (
        <div className="card card-pad row">
          <div className="grow">
            <h2>Another project</h2>
            <p className="secondary small">Projects keep events, persons and flags fully separate. Each gets its own token.</p>
          </div>
          <button className="btn" onClick={() => setCreating(true)}>
            <Icon name="plus" size={14} /> New project
          </button>
        </div>
      )}
      {creating && (
        <Modal
          title="New project"
          onClose={() => setCreating(false)}
          footer={
            <>
              <button className="btn" onClick={() => setCreating(false)}>
                Cancel
              </button>
              <button className="btn primary" onClick={create}>
                Create project
              </button>
            </>
          }
        >
          <label className="field">
            <span>Name</span>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Marketing site" onKeyDown={(e) => e.key === "Enter" && create()} />
            <span className="hint">In {organization.name}.</span>
          </label>
        </Modal>
      )}
    </div>
  );
}

function ApiKeys() {
  const { data, error, loading, reload } = useApi("keys", (s) => api.listKeys(s));
  const [name, setName] = useState("");
  const [scope, setScope] = useState<KeyScope>("read");
  const [created, setCreated] = useState<CreatedKey | null>(null);
  const [revoking, setRevoking] = useState<{ id: string; name: string } | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <div className="col gap-16">
      <div className="card card-pad col gap-12">
        <h2>Personal API keys</h2>
        <p className="secondary">
          Read your data from scripts and agents: <code>Authorization: Bearer phx_…</code> against <code>/api/projects/&lt;id&gt;/…</code>. Keys act as you.
        </p>
        <div className="row gap-16" role="radiogroup" aria-label="Key scope">
          <label className="row small">
            <input type="radio" name="scope" checked={scope === "read"} onChange={() => setScope("read")} />
            <span>
              <b>Read only</b> <span className="muted">· query analytics and read resources</span>
            </span>
          </label>
          <label className="row small">
            <input type="radio" name="scope" checked={scope === "write"} onChange={() => setScope("write")} />
            <span>
              <b>Read &amp; write</b> <span className="muted">· also create, change and delete resources</span>
            </span>
          </label>
        </div>
        <div className="row">
          <input className="input grow" placeholder="Key name, e.g. Nightly export" value={name} onChange={(e) => setName(e.target.value)} aria-label="Key name" />
          <button
            className="btn primary"
            disabled={!name.trim() || busy}
            onClick={async () => {
              setBusy(true);
              try {
                const k = await api.createKey(name.trim(), scope);
                setCreated(k);
                setName("");
                invalidate("keys");
                reload();
              } catch (e) {
                toast(errorMessage(e), true);
              } finally {
                setBusy(false);
              }
            }}
          >
            Create key
          </button>
        </div>
      </div>
      <div className="card">
        {error && !data ? (
          <ErrorState error={error} retry={reload} />
        ) : !data && loading ? (
          <SkeletonRows rows={3} />
        ) : data && data.length === 0 ? (
          <Empty icon="key" title="No API keys" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Scope</th>
                <th>Key</th>
                <th>Created</th>
                <th>Last used</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {(data ?? []).map((k) => (
                <tr key={k.id}>
                  <td style={{ fontWeight: 600 }}>{k.name}</td>
                  <td>{k.scope === "write" ? <span className="badge warn">read &amp; write</span> : <span className="badge">read only</span>}</td>
                  <td className="mono small muted">{k.key_prefix}…</td>
                  <td className="muted">{fmtDate(k.created_at)}</td>
                  <td className="muted">{k.last_used ? fmtRelative(k.last_used) : "never"}</td>
                  <td className="r">
                    <button className="btn ghost small danger" onClick={() => setRevoking({ id: k.id, name: k.name })}>
                      Revoke
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      {created && (
        <Modal title="Copy your new key" onClose={() => setCreated(null)} footer={<button className="btn primary" onClick={() => setCreated(null)}>I've stored it</button>}>
          <div className="col gap-12">
            <div className="notice warn">
              <Icon name="alert" />
              This is the only time the secret is shown. Store it in your secrets manager now.
            </div>
            <div className="small secondary">
              Scope: <b>{created.key.scope === "write" ? "read & write" : "read only"}</b>
            </div>
            <div className="token-box">
              <code>{created.secret}</code>
              <CopyButton text={created.secret} />
            </div>
          </div>
        </Modal>
      )}
      {revoking && (
        <Confirm
          title={`Revoke “${revoking.name}”?`}
          body="Anything using this key stops working immediately."
          confirmLabel="Revoke key"
          danger
          onClose={() => setRevoking(null)}
          onConfirm={async () => {
            await api.revokeKey(revoking.id);
            invalidate("keys");
            reload();
            toast("Key revoked");
          }}
        />
      )}
    </div>
  );
}

function Account() {
  const { workspace, logout } = useApp();
  const theme = useTheme();
  return (
    <div className="col gap-16">
      <div className="card card-pad col gap-12">
        <h2>Account</h2>
        <div className="kv" style={{ gridTemplateColumns: "180px 1fr" }}>
          <div>Email</div>
          <div>{workspace.user.email}</div>
          <div>Organizations</div>
          <div style={{ fontFamily: "var(--font)", fontSize: 13 }}>{workspace.organizations.map((o) => `${o.name} (${o.role})`).join(", ")}</div>
        </div>
      </div>
      <div className="card card-pad row">
        <div className="grow">
          <h2>Appearance</h2>
          <p className="secondary small">System follows your OS setting.</p>
        </div>
        <Seg<Theme>
          label="Theme"
          value={theme}
          onChange={setTheme}
          options={[
            { value: "system", label: "System" },
            { value: "light", label: "Light" },
            { value: "dark", label: "Dark" },
          ]}
        />
      </div>
      <div className="card card-pad row">
        <div className="grow">
          <h2>Sign out</h2>
          <p className="secondary small">Ends this browser session.</p>
        </div>
        <button className="btn" onClick={() => void logout()}>
          <Icon name="logout" size={14} /> Sign out
        </button>
      </div>
    </div>
  );
}

export function SettingsPage() {
  const loc = useLocation();
  const tab = (loc.search.get("tab") as "project" | "keys" | "account" | null) ?? "project";
  return (
    <div className="page narrow">
      <div className="page-head">
        <div className="titles">
          <h1>Settings</h1>
        </div>
      </div>
      <Tabs
        value={tab}
        onChange={(t) => navigate(`${loc.path}?tab=${t}`, { replace: true })}
        options={[
          { value: "project", label: "Project" },
          { value: "keys", label: "API keys" },
          { value: "account", label: "Account" },
        ]}
      />
      {tab === "project" && <ProjectSettings />}
      {tab === "keys" && <ApiKeys />}
      {tab === "account" && <Account />}
    </div>
  );
}
