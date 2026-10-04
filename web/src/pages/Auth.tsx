import { useState, type FormEvent } from "react";
import { ApiError, api, errorMessage, isMock, type Workspace } from "../lib/api";
import { Icon, Logo } from "../ui/icons";

function FieldError({ error, field }: { error: unknown; field: string }) {
  if (error instanceof ApiError && error.field === field) return <span className="error">This field is invalid.</span>;
  return null;
}

export function SetupPage({ onDone }: { onDone: (w: Workspace) => void }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [org, setOrg] = useState("");
  const [project, setProject] = useState("Default project");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onDone(await api.setup({ email: email.trim(), password, organization_name: org.trim(), project_name: project.trim() || "Default project" }));
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  const short = password.length > 0 && password.length < 8;

  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <div className="row">
          <Logo size={32} />
          <b style={{ fontSize: 17 }}>Hoglet</b>
        </div>
        <h1>Set up your Hoglet</h1>
        <p className="lede">One owner account, one organization, one project. Takes a minute; you can add more later.</p>
        <div className="col gap-16">
          <label className="field">
            <span>Your email</span>
            <input className="input" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            <FieldError error={error} field="email" />
          </label>
          <label className="field">
            <span>Password</span>
            <input className={`input${short ? " invalid" : ""}`} type="password" autoComplete="new-password" required minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} />
            <span className={short ? "error" : "hint"}>At least 8 characters.</span>
          </label>
          <div className="row gap-12" style={{ alignItems: "flex-start" }}>
            <label className="field grow">
              <span>Organization</span>
              <input className="input" required placeholder="Acme Inc." value={org} onChange={(e) => setOrg(e.target.value)} />
              <FieldError error={error} field="organization_name" />
            </label>
            <label className="field grow">
              <span>First project</span>
              <input className="input" required value={project} onChange={(e) => setProject(e.target.value)} />
              <FieldError error={error} field="project_name" />
            </label>
          </div>
          {error ? <div className="notice bad">{errorMessage(error)}</div> : null}
          <button className="btn primary large" type="submit" disabled={busy || short}>
            {busy ? "Creating…" : "Create workspace"} {!busy && <Icon name="arrowRight" size={14} />}
          </button>
        </div>
        <div className="auth-foot">Your data stays on this server. Nothing is sent anywhere else.</div>
      </form>
    </div>
  );
}

export function LoginPage({ onDone }: { onDone: (w: Workspace) => void }) {
  const [email, setEmail] = useState(isMock ? "demo@hoglet.dev" : "");
  const [password, setPassword] = useState(isMock ? "hoglet-demo" : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onDone(await api.login(email.trim(), password));
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  const bad = error instanceof ApiError && error.status === 401;
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <div className="row">
          <Logo size={32} />
          <b style={{ fontSize: 17 }}>Hoglet</b>
        </div>
        <h1>Sign in</h1>
        <p className="lede">Product analytics, on your own server.</p>
        <div className="col gap-16">
          <label className="field">
            <span>Email</span>
            <input className="input" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
          </label>
          <label className="field">
            <span>Password</span>
            <input className="input" type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
          </label>
          {error ? <div className="notice bad">{bad ? "That email and password don't match." : errorMessage(error)}</div> : null}
          <button className="btn primary large" type="submit" disabled={busy}>
            {busy ? "Signing in…" : "Sign in"}
          </button>
        </div>
        <div className="auth-foot">Sessions last 7 days on this browser.</div>
      </form>
    </div>
  );
}
