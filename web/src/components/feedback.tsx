// Loading, empty and error states, rendered the same way on every page.

import type { ReactNode } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { ApiError, errorMessage } from "@/lib/api";
import { cn } from "@/lib/utils";
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
      <h3>{title}</h3>
      {children ? <p className="max-w-md text-muted-foreground">{children}</p> : null}
      {action ? <div className="mt-2">{action}</div> : null}
    </div>
  );
}

/** Uniform error rendering; "not available" (endpoint missing) reads as a state, not a failure. */
export function ErrorState({ error, retry, compact }: { error: unknown; retry?: () => void; compact?: boolean }) {
  const unavailable = error instanceof ApiError && error.notAvailable;
  const requestId = error instanceof ApiError ? error.requestId : null;
  const title = unavailable ? "Not available on this server yet" : "Couldn't load this";
  const body = unavailable ? "This Hoglet build doesn't serve this data yet. Upgrade the binary to enable it." : errorMessage(error);
  const retryButton = retry ? (
    <Button variant="outline" size="sm" onClick={retry}>
      <Icon name="refresh" size={14} /> Retry
    </Button>
  ) : null;

  if (compact) {
    return (
      <Alert variant={unavailable ? "default" : "destructive"} className="flex items-center gap-3 [&>svg]:translate-y-0">
        <Icon name={unavailable ? "info" : "alert"} />
        <div className="min-w-0 flex-1">
          <AlertTitle>{title}</AlertTitle>
          <AlertDescription>
            {body}
            {requestId ? <span className="font-mono"> · {requestId.slice(-8)}</span> : null}
          </AlertDescription>
        </div>
        {retryButton}
      </Alert>
    );
  }
  return (
    <Empty icon={unavailable ? "clock" : "alert"} title={title} action={retryButton}>
      {body}
      {requestId ? <span className="font-mono"> · {requestId.slice(-8)}</span> : null}
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
