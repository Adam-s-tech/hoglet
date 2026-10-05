// A plain textarea with a highlighted mirror underneath: no editor library.
// Ctrl/Cmd+Enter runs. Tab indents; press Escape first to let Tab leave the field.

import { useId, useMemo, useRef, type ReactNode } from "react";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

const KEYWORDS = new Set(
  "select from where group by order limit offset having as and or not in is null like ilike between case when then else end distinct count sum avg min max join left right inner outer on with union all asc desc interval day days hour hours week month year now date_trunc cast true false over partition".split(" "),
);

function highlight(code: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(--[^\n]*)|('(?:[^']|'')*')|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z0-9_]*)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = re.exec(code))) {
    if (m.index > last) out.push(code.slice(last, m.index));
    if (m[1])
      out.push(
        <span key={k++} className="text-muted-foreground italic">
          {m[1]}
        </span>,
      );
    else if (m[2])
      out.push(
        <span key={k++} className="text-good">
          {m[2]}
        </span>,
      );
    else if (m[3])
      out.push(
        <span key={k++} className="text-[var(--s2)]">
          {m[3]}
        </span>,
      );
    else if (m[4] && KEYWORDS.has(m[4].toLowerCase()))
      out.push(
        <span key={k++} className="font-semibold text-[var(--s7)]">
          {m[4]}
        </span>,
      );
    else out.push(m[0]);
    last = re.lastIndex;
  }
  out.push(code.slice(last));
  // Trailing newline needs a character to keep heights aligned.
  out.push("\n");
  return out;
}

const TEXT = "px-3.5 py-3 font-mono text-[13px] leading-[1.6] whitespace-pre-wrap [overflow-wrap:anywhere] [tab-size:2]";

export function SqlEditor({ value, onChange, onRun }: { value: string; onChange: (v: string) => void; onRun: () => void }) {
  const html = useMemo(() => highlight(value), [value]);
  const hintId = useId();
  // After Escape the next Tab moves focus instead of indenting (no keyboard trap).
  const releaseTab = useRef(false);
  return (
    <div className="relative min-h-40 rounded-lg border border-input bg-muted/40 transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
      <pre aria-hidden="true" className={cn(TEXT, "pointer-events-none m-0 min-h-40 text-foreground")}>
        {html}
      </pre>
      <Textarea
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        value={value}
        aria-label="SQL query"
        aria-describedby={hintId}
        className={cn(
          TEXT,
          "absolute inset-0 h-full min-h-0 resize-none rounded-lg border-0 bg-transparent text-transparent caret-foreground shadow-none field-sizing-fixed focus-visible:ring-0 md:text-[13px] dark:bg-transparent",
        )}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            onRun();
          } else if (e.key === "Escape") {
            releaseTab.current = true;
            return;
          } else if (e.key === "Tab" && !e.shiftKey && !releaseTab.current) {
            e.preventDefault();
            const t = e.currentTarget;
            const { selectionStart: a, selectionEnd: b } = t;
            onChange(`${value.slice(0, a)}  ${value.slice(b)}`);
            requestAnimationFrame(() => t.setSelectionRange(a + 2, a + 2));
          }
          if (e.key !== "Escape" && e.key !== "Tab") releaseTab.current = false;
        }}
      />
      <span id={hintId} className="sr-only">
        Press Control or Command plus Enter to run. Tab inserts spaces; press Escape, then Tab, to leave the editor.
      </span>
    </div>
  );
}
