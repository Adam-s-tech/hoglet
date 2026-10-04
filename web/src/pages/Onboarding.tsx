import { useState } from "react";
import { ApiError, api, errorMessage } from "../lib/api";
import { useApp, usePath, useProjectId } from "../lib/context";
import { fmtNumber, fmtRelative } from "../lib/format";
import { invalidate, useApi } from "../lib/hooks";
import { SNIPPETS, hostOrigin } from "../lib/snippets";
import { Link } from "../lib/router";
import { Icon } from "../ui/icons";
import { CopyButton, Snippet, Tabs, toast } from "../ui/kit";

export function SnippetTabs({ token }: { token: string }) {
  const [tab, setTab] = useState(SNIPPETS[0].id);
  const s = SNIPPETS.find((x) => x.id === tab) ?? SNIPPETS[0];
  const host = hostOrigin();
  return (
    <div>
      <Tabs value={tab} onChange={setTab} options={SNIPPETS.map((x) => ({ value: x.id, label: x.label }))} />
      <div className="col gap-12">
        {s.install && <Snippet code={s.install} language="sh" />}
        <Snippet code={s.code(token, host)} language={s.language} />
      </div>
    </div>
  );
}

export function TokenBox({ token }: { token: string }) {
  return (
    <div className="token-box">
      <Icon name="key" size={14} style={{ color: "var(--ink-3)" }} />
      <code>{token}</code>
      <CopyButton text={token} />
    </div>
  );
}

/**
 * Fills the project with 90 days of realistic demo data through the real
 * ingest path. Events become queryable a second or two after it returns.
 */
export function LoadDemoButton({ primary = false, small = false, onLoaded }: { primary?: boolean; small?: boolean; onLoaded?: () => void }) {
  const projectId = useProjectId();
  const [busy, setBusy] = useState(false);
  return (
    <button
      className={`btn${primary ? " accent" : ""}${small ? " small" : ""}`}
      disabled={busy}
      title="Adds 90 days of sample product data to this project so you can explore every screen"
      onClick={async () => {
        setBusy(true);
        try {
          const { events } = await api.loadDemo(projectId);
          // Give publication a moment, then drop every cached answer for this project.
          await new Promise((r) => window.setTimeout(r, 2500));
          invalidate("");
          toast(`Loaded ${fmtNumber(events)} demo events`);
          onLoaded?.();
          window.dispatchEvent(new Event("hoglet:data-changed"));
        } catch (e) {
          toast(errorMessage(e), true);
        } finally {
          setBusy(false);
        }
      }}
    >
      {busy ? <span className="spinner" style={{ width: 14, height: 14 }} /> : <Icon name="sparkle" size={14} />}
      {busy ? "Loading demo data…" : "Load demo data"}
    </button>
  );
}

/** Polls /status until the first event lands. */
export function FirstEventWatcher() {
  const projectId = useProjectId();
  const path = usePath();
  const { project } = useApp();
  const status = useApi(`status:${projectId}`, (s) => api.status(projectId, s), { pollMs: 3000 });
  const [sending, setSending] = useState(false);
  const notAvailable = status.error instanceof ApiError && status.error.notAvailable;
  const has = status.data?.has_events ?? false;

  const sendTest = async () => {
    setSending(true);
    try {
      await api.captureTestEvent(project.token, `hoglet-onboarding-${Math.random().toString(36).slice(2, 8)}`);
      toast("Test event sent. Watching for it…");
      status.reload();
    } catch (e) {
      toast(errorMessage(e), true);
    } finally {
      setSending(false);
    }
  };

  if (has && status.data) {
    return (
      <div className="waiting done">
        <Icon name="check" size={20} style={{ color: "var(--good)" }} strokeWidth={2.4} />
        <div className="grow">
          <b>Events are arriving.</b>
          <div className="small secondary">
            {fmtNumber(status.data.stored_events)} stored · last one {fmtRelative(status.data.last_event_at)}
          </div>
        </div>
        <Link className="btn" to={path("activity")}>
          See live events
        </Link>
        <Link className="btn primary" to={path("web")}>
          Open web analytics <Icon name="arrowRight" size={13} />
        </Link>
      </div>
    );
  }
  return (
    <div className="waiting">
      {notAvailable ? <Icon name="info" size={18} style={{ color: "var(--ink-3)" }} /> : <span className="spinner" aria-hidden="true" />}
      <div className="grow">
        <b>{notAvailable ? "Status isn't available on this server build" : "Waiting for your first event…"}</b>
        <div className="small secondary">
          {notAvailable ? (
            <>Send an event, then check Activity.</>
          ) : (
            <>Load a page with the snippet installed, or send a test event. This updates on its own.</>
          )}
        </div>
      </div>
      <button className="btn" onClick={sendTest} disabled={sending}>
        <Icon name="bolt" size={14} /> {sending ? "Sending…" : "Send a test event"}
      </button>
      <LoadDemoButton primary onLoaded={status.reload} />
    </div>
  );
}

export function OnboardingPage() {
  const { project } = useApp();
  const path = usePath();
  return (
    <div className="page narrow">
      <div className="page-head">
        <div className="titles">
          <h1>Connect your app</h1>
          <div className="sub">Hoglet speaks PostHog's protocol. Use the stock PostHog SDKs and set the host to this server; nothing else changes.</div>
        </div>
        <div className="actions">
          <Link className="btn ghost" to={path("")}>
            Skip for now
          </Link>
        </div>
      </div>
      <div className="col gap-16">
        <div className="card card-pad col gap-12">
          <div className="row">
            <span className="letter" style={{ background: "var(--accent)" }}>1</span>
            <h2>Your project token</h2>
          </div>
          <p className="secondary">
            Public by design: it only lets clients send events to <b>{project.name}</b>. It cannot read data.
          </p>
          <TokenBox token={project.token} />
        </div>
        <div className="card card-pad col gap-12">
          <div className="row">
            <span className="letter" style={{ background: "var(--accent)" }}>2</span>
            <h2>Install the SDK</h2>
          </div>
          <p className="secondary">
            Pick your stack. <code>api_host</code> is this server: <code>{hostOrigin()}</code>
          </p>
          <SnippetTabs token={project.token} />
        </div>
        <div className="card card-pad col gap-12">
          <div className="row">
            <span className="letter" style={{ background: "var(--accent)" }}>3</span>
            <h2>Send an event</h2>
          </div>
          <FirstEventWatcher />
          <p className="muted small">
            Just looking around? <b>Load demo data</b> fills this project with 90 days of a sample SaaS product: pageviews, signups, subscriptions, AI generations and errors.
          </p>
        </div>
      </div>
    </div>
  );
}
