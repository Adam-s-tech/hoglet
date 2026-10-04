import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState, type ReactNode } from "react";
import { Icon } from "@/components/icons";
import { CopyButton, Snippet } from "@/components/copy";
import { CardPad, Page, PageHeader, Panel } from "@/components/page";
import { toast } from "@/components/toast";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ApiError, api, errorMessage } from "@/lib/api";
import { projectPath, useApp, usePath, useProjectId } from "@/lib/context";
import { fmtNumber, fmtRelative } from "@/lib/format";
import { navigate } from "@/lib/nav";
import { qk, statusQuery } from "@/lib/queries";
import { SNIPPETS, hostOrigin } from "@/lib/snippets";

export function SnippetTabs({ token }: { token: string }) {
  const [tab, setTab] = useState(SNIPPETS[0].id);
  const host = hostOrigin();
  return (
    <Tabs value={tab} onValueChange={(v) => setTab(String(v))}>
      <TabsList className="h-auto max-w-full flex-wrap justify-start" aria-label="SDK">
        {SNIPPETS.map((s) => (
          <TabsTrigger key={s.id} value={s.id} className="flex-none px-3">
            {s.label}
          </TabsTrigger>
        ))}
      </TabsList>
      {SNIPPETS.map((s) => (
        <TabsContent key={s.id} value={s.id} className="flex flex-col gap-3 pt-1">
          {s.install && <Snippet code={s.install} language="sh" />}
          <Snippet code={s.code(token, host)} language={s.language} />
        </TabsContent>
      ))}
    </Tabs>
  );
}

export function TokenBox({ token }: { token: string }) {
  return (
    <div className="flex items-center gap-2.5 rounded-lg border bg-muted/40 py-1.5 pr-1.5 pl-3">
      <Icon name="key" size={14} className="flex-none text-muted-foreground" />
      <code className="min-w-0 flex-1 truncate font-mono" aria-label="Project token">
        {token}
      </code>
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
  const queryClient = useQueryClient();
  const [progress, setProgress] = useState<string | null>(null);
  const load = useMutation({
    mutationFn: async () => {
      setProgress("Generating…");
      const { events } = await api.loadDemo(projectId);
      // Events are acknowledged once durable; wait until they are queryable
      // (the status endpoint reports the publication lag), at most a minute.
      const deadline = Date.now() + 60_000;
      while (Date.now() < deadline) {
        await new Promise((r) => window.setTimeout(r, 1000));
        try {
          const s = await api.status(projectId);
          setProgress(`Publishing… ${fmtNumber(s.stored_events)} stored`);
          if (s.has_events && s.ingestion_lag_seconds < 2) break;
        } catch {
          // Status unavailable: fall back to a fixed pause.
          await new Promise((r) => window.setTimeout(r, 2500));
          break;
        }
      }
      return events;
    },
    onSuccess: async (events) => {
      toast(`Loaded ${fmtNumber(events)} demo events`);
      onLoaded?.();
      await queryClient.invalidateQueries({ queryKey: qk.project(projectId) });
      navigate(projectPath(projectId, "web"));
    },
    onError: (e) => toast(errorMessage(e), true),
    onSettled: () => setProgress(null),
  });
  return (
    <Button
      type="button"
      variant={primary ? "default" : "outline"}
      size={small ? "sm" : "default"}
      disabled={load.isPending}
      title="Adds 90 days of sample product data to this project so you can explore every screen"
      onClick={() => load.mutate()}
    >
      {load.isPending ? <Spinner role="presentation" aria-hidden="true" className="size-3.5" /> : <Icon name="sparkle" size={14} />}
      <span aria-live="polite">{load.isPending ? (progress ?? "Loading demo data…") : "Load demo data"}</span>
    </Button>
  );
}

/** Polls /status until the first event lands, then relaxes to a slow refresh. */
export function FirstEventWatcher() {
  const projectId = useProjectId();
  const path = usePath();
  const { project } = useApp();
  const queryClient = useQueryClient();
  const status = useQuery({
    ...statusQuery(projectId, 3000),
    refetchInterval: (q) => (q.state.data?.has_events ? 10_000 : 3000),
  });
  const notAvailable = status.error instanceof ApiError && status.error.notAvailable;
  const has = status.data?.has_events ?? false;

  const sendTest = useMutation({
    mutationFn: () => api.captureTestEvent(project.token, `hoglet-onboarding-${Math.random().toString(36).slice(2, 8)}`),
    onSuccess: () => {
      toast("Test event sent. Watching for it…");
      void queryClient.invalidateQueries({ queryKey: qk.status(projectId) });
    },
    onError: (e) => toast(errorMessage(e), true),
  });

  if (has && status.data) {
    return (
      <Banner tone="done" icon={<Icon name="check" size={20} strokeWidth={2.4} className="text-good" />}>
        <div className="min-w-0 flex-1">
          <b>Events are arriving.</b>
          <div className="text-xs text-muted-foreground">
            {fmtNumber(status.data.stored_events)} stored · last one {fmtRelative(status.data.last_event_at)}
          </div>
        </div>
        <Button variant="outline" nativeButton={false} render={<Link to={path("activity")} />}>
          See live events
        </Button>
        <Button nativeButton={false} render={<Link to={path("web")} />}>
          Open web analytics <Icon name="arrowRight" size={13} />
        </Button>
      </Banner>
    );
  }
  return (
    <Banner
      tone="waiting"
      icon={notAvailable ? <Icon name="info" size={18} className="text-muted-foreground" /> : <Spinner role="presentation" aria-hidden="true" className="size-[18px] text-muted-foreground" />}
    >
      <div className="min-w-0 flex-1">
        <b>{notAvailable ? "Status isn't available on this server build" : "Waiting for your first event…"}</b>
        <div className="text-xs text-muted-foreground">
          {notAvailable ? "Send an event, then check Activity." : "Load a page with the snippet installed, or send a test event. This updates on its own."}
        </div>
      </div>
      <Button type="button" variant="outline" onClick={() => sendTest.mutate()} disabled={sendTest.isPending}>
        <Icon name="bolt" size={14} /> {sendTest.isPending ? "Sending…" : "Send a test event"}
      </Button>
      <LoadDemoButton primary onLoaded={() => void queryClient.invalidateQueries({ queryKey: qk.status(projectId) })} />
    </Banner>
  );
}

function Banner({ tone, icon, children }: { tone: "waiting" | "done"; icon: ReactNode; children: ReactNode }) {
  return (
    <div
      role="status"
      aria-live="polite"
      className={
        tone === "done"
          ? "flex flex-wrap items-center gap-3 rounded-lg border border-good/30 bg-good-wash px-4 py-3"
          : "flex flex-wrap items-center gap-3 rounded-lg border border-dashed bg-muted/40 px-4 py-3"
      }
    >
      <span className="flex-none">{icon}</span>
      {children}
    </div>
  );
}

function Step({ n, title, children }: { n: number; title: string; children: ReactNode }) {
  return (
    <Panel>
      <CardPad className="flex flex-col gap-3">
        <div className="flex items-center gap-2.5">
          <span aria-hidden="true" className="grid size-6 flex-none place-items-center rounded-full bg-brand-wash text-xs font-semibold text-brand-foreground">
            {n}
          </span>
          <h2>{title}</h2>
        </div>
        {children}
      </CardPad>
    </Panel>
  );
}

export function OnboardingPage() {
  const { project } = useApp();
  const path = usePath();
  return (
    <Page narrow>
      <PageHeader
        title="Connect your app"
        sub="Hoglet speaks PostHog's protocol. Use the stock PostHog SDKs and set the host to this server; nothing else changes."
        actions={
          <Button variant="ghost" nativeButton={false} render={<Link to={path("")} />}>
            Skip for now
          </Button>
        }
      />
      <div className="flex flex-col gap-4">
        <Step n={1} title="Your project token">
          <p className="text-muted-foreground">
            Public by design: it only lets clients send events to <b className="text-foreground">{project.name}</b>. It cannot read data.
          </p>
          <TokenBox token={project.token} />
        </Step>
        <Step n={2} title="Install the SDK">
          <p className="text-muted-foreground">
            Pick your stack. <code>api_host</code> is this server: <code>{hostOrigin()}</code>
          </p>
          <SnippetTabs token={project.token} />
        </Step>
        <Step n={3} title="Send an event">
          <FirstEventWatcher />
          <p className="text-xs text-muted-foreground">
            Just looking around? <b>Load demo data</b> fills this project with 90 days of a sample SaaS product: pageviews, signups, subscriptions, AI generations and errors.
          </p>
        </Step>
      </div>
    </Page>
  );
}
