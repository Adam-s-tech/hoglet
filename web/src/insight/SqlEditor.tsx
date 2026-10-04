// A plain textarea with a highlighted mirror underneath: no editor library.

import { useMemo, type ReactNode } from "react";

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
    if (m[1]) out.push(<span key={k++} className="cmt">{m[1]}</span>);
    else if (m[2]) out.push(<span key={k++} className="str">{m[2]}</span>);
    else if (m[3]) out.push(<span key={k++} className="numl">{m[3]}</span>);
    else if (m[4] && KEYWORDS.has(m[4].toLowerCase())) out.push(<span key={k++} className="kw">{m[4]}</span>);
    else out.push(m[0]);
    last = re.lastIndex;
  }
  out.push(code.slice(last));
  // Trailing newline needs a character to keep heights aligned.
  out.push("\n");
  return out;
}

export function SqlEditor({ value, onChange, onRun }: { value: string; onChange: (v: string) => void; onRun: () => void }) {
  const html = useMemo(() => highlight(value), [value]);
  return (
    <div className="code-editor">
      <pre aria-hidden="true">{html}</pre>
      <textarea
        spellCheck={false}
        value={value}
        aria-label="SQL query"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            onRun();
          } else if (e.key === "Tab" && !e.shiftKey) {
            e.preventDefault();
            const t = e.currentTarget;
            const { selectionStart: a, selectionEnd: b } = t;
            const next = `${value.slice(0, a)}  ${value.slice(b)}`;
            onChange(next);
            requestAnimationFrame(() => t.setSelectionRange(a + 2, a + 2));
          }
        }}
      />
    </div>
  );
}
