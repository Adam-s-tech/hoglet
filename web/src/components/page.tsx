// Page-level layout pieces shared by every screen: the same width, header,
// toolbar and card rhythm everywhere.

import type { ComponentProps, ReactNode } from "react";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import { Icon } from "./icons";

export function Page({ narrow, className, children }: { narrow?: boolean; className?: string; children: ReactNode }) {
  return (
    <div className={cn("mx-auto w-full px-3.5 pt-4 pb-12 md:px-7 md:pt-[22px] md:pb-16", narrow ? "max-w-[920px]" : "max-w-[1440px]", className)}>{children}</div>
  );
}

export function PageHeader({
  title,
  sub,
  actions,
  leading,
  className,
}: {
  title: ReactNode;
  sub?: ReactNode;
  actions?: ReactNode;
  leading?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("mb-[18px] flex min-h-9 flex-col items-start gap-3 md:flex-row md:items-center md:gap-4", className)}>
      {leading}
      <div className="min-w-0 flex-1">
        <h1 className="truncate">{title}</h1>
        {sub ? <div className="mt-0.5 text-muted-foreground">{sub}</div> : null}
      </div>
      {actions ? <div className="flex flex-wrap items-center gap-2 md:justify-end">{actions}</div> : null}
    </div>
  );
}

export function Toolbar({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("mb-4 flex flex-wrap items-center gap-2", className)} {...props} />;
}

/** A flush card: no inner padding, so tables and lists run edge to edge. Pair with CardBar. */
export function Panel({ className, ...props }: ComponentProps<typeof Card>) {
  return <Card className={cn("gap-0 py-0", className)} {...props} />;
}

/** The header row of a Panel: title, filters, counts. */
export function CardBar({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("flex min-h-12 flex-wrap items-center gap-2.5 border-b px-4 py-2.5", className)} {...props} />;
}

export function CardPad({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("p-4", className)} {...props} />;
}

export function SearchInput({ className, wrapperClassName, ...props }: ComponentProps<typeof Input> & { wrapperClassName?: string }) {
  return (
    <div className={cn("relative flex items-center", wrapperClassName)}>
      <Icon name="search" size={14} className="pointer-events-none absolute left-2.5 text-muted-foreground" />
      <Input type="search" className={cn("w-full pl-8 [&::-webkit-search-cancel-button]:hidden", className)} {...props} />
    </div>
  );
}

/** Label + control + hint/error, wired for accessibility via htmlFor/id. */
export function FormField({
  label,
  htmlFor,
  hint,
  error,
  className,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  hint?: ReactNode;
  error?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <Label htmlFor={htmlFor} className="text-xs font-semibold text-muted-foreground">
        {label}
      </Label>
      {children}
      {error ? (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      ) : hint ? (
        <span className="text-xs text-muted-foreground">{hint}</span>
      ) : null}
    </div>
  );
}

/** Two-column key/value list (properties, facts). */
export function KV({ items, className }: { items: [ReactNode, ReactNode][]; className?: string }) {
  return (
    <dl className={cn("grid grid-cols-[minmax(120px,max-content)_1fr] gap-x-4 gap-y-1.5 font-mono text-[12.5px]", className)}>
      {items.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="m-0 break-all">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Small uppercase stat label, e.g. above a KPI value. */
export function StatLabel({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("text-xs font-semibold tracking-wide text-muted-foreground uppercase", className)} {...props} />;
}

/** A rounded square holding an icon: insight kinds, list rows. */
export function IconBadge({ children, tone = "muted", className }: { children: ReactNode; tone?: "muted" | "brand"; className?: string }) {
  return (
    <span
      className={cn(
        "inline-grid size-7 flex-none place-items-center rounded-md",
        tone === "brand" ? "bg-brand-wash text-brand-foreground" : "bg-muted text-muted-foreground",
        className,
      )}
    >
      {children}
    </span>
  );
}
