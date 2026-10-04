// Role-aware controls. The server is the authority (members get 403 on every
// write); the UI only stops offering what would be refused, and says why.

import { cloneElement, type ReactElement } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export const READ_ONLY_REASON = "Read-only access: only organization owners and admins can do this.";

/**
 * A write action. Owners and admins get `children` unchanged. A member sees the
 * same control, disabled, with the reason on hover and keyboard focus (a
 * disabled button takes no focus itself, so a wrapper does).
 */
export function Gated({ allowed, reason = READ_ONLY_REASON, children }: { allowed: boolean; reason?: string; children: ReactElement<Record<string, unknown>> }) {
  if (allowed) return children;
  return (
    <Tooltip>
      <TooltipTrigger render={<span tabIndex={0} className="inline-flex rounded-lg outline-none focus-visible:ring-3 focus-visible:ring-ring/50" />}>
        {cloneElement(children, { disabled: true, onClick: undefined, render: undefined, nativeButton: true })}
      </TooltipTrigger>
      <TooltipContent>{reason}</TooltipContent>
    </Tooltip>
  );
}
