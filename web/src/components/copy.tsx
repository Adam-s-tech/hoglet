import { useEffect, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Icon } from "./icons";

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  }
}

/** `label=""` renders an icon-only button (give it an aria-label via `title`). */
export function CopyButton({ text, label = "Copy", className, title }: { text: string; label?: string; className?: string; title?: string }) {
  const [done, setDone] = useState(false);
  useEffect(() => {
    if (!done) return;
    const id = window.setTimeout(() => setDone(false), 1400);
    return () => window.clearTimeout(id);
  }, [done]);
  return (
    <Button
      type="button"
      variant={label ? "outline" : "ghost"}
      size={label ? "sm" : "icon-sm"}
      className={className}
      onClick={async () => {
        if (await copyText(text)) setDone(true);
      }}
      aria-label={label || title || "Copy"}
    >
      <Icon name={done ? "check" : "copy"} size={14} />
      {label ? <span>{done ? "Copied" : label}</span> : null}
    </Button>
  );
}

export function Snippet({ code, language }: { code: string; language?: string }) {
  return (
    <div className="group relative rounded-lg bg-muted" data-lang={language}>
      <CopyButton text={code} className="absolute top-2 right-2 opacity-70 group-hover:opacity-100 focus-visible:opacity-100" />
      <pre className="m-0 overflow-x-auto p-3.5 pr-24 leading-relaxed">
        <code>{code}</code>
      </pre>
    </div>
  );
}

/** Syntax-colored, read-only JSON. */
export function JsonView({ value, className }: { value: unknown; className?: string }) {
  const text = JSON.stringify(value, null, 2) ?? "null";
  const parts: ReactNode[] = [];
  const re = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) parts.push(text.slice(last, m.index));
    if (m[1]) {
      parts.push(
        <span key={i++} className={m[2] ? "text-brand-foreground" : "text-good"}>
          {m[1]}
        </span>,
      );
      if (m[2]) parts.push(m[2]);
    } else if (m[3]) {
      parts.push(
        <span key={i++} className="text-warn">
          {m[3]}
        </span>,
      );
    } else if (m[4]) {
      parts.push(
        <span key={i++} className="text-chart-1">
          {m[4]}
        </span>,
      );
    }
    last = re.lastIndex;
  }
  parts.push(text.slice(last));
  return <pre className={cn("m-0 overflow-auto rounded-lg bg-muted p-3 font-mono whitespace-pre-wrap", className)}>{parts}</pre>;
}
