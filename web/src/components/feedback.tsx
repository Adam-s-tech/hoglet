// Loading, empty and error states, rendered the same way on every page.

import type { ReactNode } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { ApiError, errorMessage } from "@/lib/api";
import { cn } from "@/lib/utils";
import { CopyButton } from "./copy";
import { Icon, type IconName } from "./icons";

export { Skeleton };

export function SkeletonRows({ rows = 6, className }: { rows?: number; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-2.5 p-4", className)} aria-busy="true" aria-label="Loading">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-5" style={{ width: `${92 - ((i * 17) % 30)}%` }} />
      ))}
    </div>
  );
}

/** A thin indeterminate bar at the top of a card while a refetch runs. */
export function LoadingBar({ show }: { show: boolean }) {
  if (!show) return null;
  return (
    <div role="progressbar" aria-label="Loading" className="pointer-events-none absolute inset-x-0 top-0 h-0.5 overflow-hidden">
      <div className="h-full w-1/3 bg-brand motion-safe:animate-[hoglet-loading_1.1s_ease-in-out_infinite]" />
    </div>
  );
}

export function Empty({
  icon = "sparkle",
  title,
  children,
  action,
  className,
}: {
  icon?: IconName;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col items-center gap-2 px-6 py-12 text-center", className)}>
      <div className="mb-1 grid size-14 place-items-center rounded-full bg-muted text-muted-foreground">
        <Icon name={icon} size={26} strokeWidth={1.4} />
      </div>
      <h2 className="text-sm">{title}</h2>
      {children ? <p className="max-w-md text-muted-foreground">{children}</p> : null}
      {action ? <div className="mt-2">{action}</div> : null}
    </div>
  );
}

/** The id the server logged for this failure: what to quote when reporting it. */
function RequestId({ id }: { id: string }) {
  return (
    <span className="inline-flex items-center gap-1 align-middle font-mono text-xs text-muted-foreground">
      request id {id}
      <CopyButton label="" title="Copy request ID" text={id} />
    </span>
  );
}

function describe(error: unknown): { title: string; body: string; icon: IconName; soft: boolean } {
  if (error instanceof ApiError) {
    if (error.notAvailable) {
      return { title: "Not available on this server yet", body: "This Hoglet build doesn't serve this data yet. Upgrade the binary to enable it.", icon: "clock", soft: true };
    }
    if (error.status === 0) {
      return { title: "Can't reach the server", body: "Check your connection and that Hoglet is running, then retry.", icon: "alert", soft: false };
    }
    if (error.status === 403) return { title: "You don't have access to this", body: error.message, icon: "alert", soft: false };
    if (error.status === 404) return { title: "This doesn't exist anymore", body: error.message, icon: "search", soft: false };
  }
  return { title: "Couldn't load this", body: errorMessage(error), icon: "alert", soft: false };
}

/** Uniform error rendering: what failed, a retry, and the request id to quote. "Not available" (endpoint missing) reads as a state, not a failure. */
export function ErrorState({ error, retry, compact }: { error: unknown; retry?: () => void; compact?: boolean }) {
  const { title, body, icon, soft } = describe(error);
  const requestId = error instanceof ApiError ? error.requestId : null;
  const retryButton = retry ? (
    <Button variant="outline" size="sm" onClick={retry}>
      <Icon name="refresh" size={14} /> Retry
    </Button>
  ) : null;

  if (compact) {
    return (
      <Alert variant={soft ? "default" : "destructive"} className="flex items-center gap-3 [&>svg]:translate-y-0">
        <Icon name={soft ? "info" : "alert"} />
        <div className="min-w-0 flex-1">
          <AlertTitle>{title}</AlertTitle>
          <AlertDescription>
            {body} {requestId ? <RequestId id={requestId} /> : null}
          </AlertDescription>
        </div>
        {retryButton}
      </Alert>
    );
  }
  return (
    <Empty icon={icon} title={title} action={retryButton}>
      {body}
      {requestId ? (
        <>
          <br />
          <RequestId id={requestId} />
        </>
      ) : null}
    </Empty>
  );
}

/** An inline notice: info, success, warning or error, optionally with an icon override. */
export function Notice({
  tone = "info",
  icon,
  children,
  className,
}: {
  tone?: "info" | "good" | "warn" | "bad";
  icon?: IconName;
  children: ReactNode;
  className?: string;
}) {
  const toneClass = {
    info: "bg-muted text-foreground",
    good: "bg-good-wash text-good",
    warn: "bg-warn-wash text-warn",
    bad: "bg-bad-wash text-destructive",
  }[tone];
  const defaultIcon: IconName = tone === "bad" || tone === "warn" ? "alert" : tone === "good" ? "check" : "info";
  return (
    <div role={tone === "bad" ? "alert" : "status"} className={cn("flex items-start gap-2.5 rounded-lg px-3 py-2.5 text-sm", toneClass, className)}>
      <Icon name={icon ?? defaultIcon} className="mt-0.5 flex-none" />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}
