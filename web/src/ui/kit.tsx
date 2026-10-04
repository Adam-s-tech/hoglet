// UI primitives: overlays, feedback states, small controls.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { ApiError, errorMessage } from "../lib/api";
import { useOnClickOutside } from "../lib/hooks";
import { Icon, type IconName } from "./icons";

// ── Modal ─────────────────────────────────────────────────────────────────

export function Modal({
  title,
  onClose,
  children,
  footer,
  wide,
  labelledBy,
}: {
  title: ReactNode;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  labelledBy?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>("input, textarea, select, button:not(.modal-x)");
    (first ?? ref.current)?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onCloseRef.current();
      }
    };
    document.addEventListener("keydown", onKey);
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = "";
      prev?.focus?.();
    };
  }, []);
  return createPortal(
    <div
      className="overlay"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className={`modal${wide ? " wide" : ""}`} role="dialog" aria-modal="true" aria-labelledby={labelledBy} ref={ref} tabIndex={-1}>
        <div className="modal-head">
          <h2 id={labelledBy}>{title}</h2>
          <button className="btn ghost icon small modal-x" onClick={onClose} aria-label="Close">
            <Icon name="x" />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-foot">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
}

export function Confirm({
  title,
  body,
  confirmLabel,
  danger,
  onConfirm,
  onClose,
}: {
  title: string;
  body: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => Promise<unknown> | void;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <Modal
      title={title}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className={`btn ${danger ? "danger solid" : "primary"}`}
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await onConfirm();
                onClose();
              } catch (e) {
                setError(errorMessage(e));
                setBusy(false);
              }
            }}
          >
            {confirmLabel}
          </button>
        </>
      }
    >
      <div className="col gap-12">
        <div className="secondary">{body}</div>
        {error && <div className="notice bad">{error}</div>}
      </div>
    </Modal>
  );
}

// ── Popover ───────────────────────────────────────────────────────────────

export function Popover({
  anchor,
  open,
  onClose,
  children,
  align = "start",
  className = "",
  width,
}: {
  anchor: RefObject<HTMLElement | null>;
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  align?: "start" | "end";
  className?: string;
  width?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<CSSProperties>({ visibility: "hidden", top: 0, left: 0 });
  useOnClickOutside([ref, anchor], onClose, open);

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const a = anchor.current?.getBoundingClientRect();
      const el = ref.current;
      if (!a || !el) return;
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      let left = align === "end" ? a.right - w : a.left;
      left = Math.max(8, Math.min(left, window.innerWidth - w - 8));
      let top = a.bottom + 4;
      if (top + h > window.innerHeight - 8 && a.top - h - 4 > 8) top = a.top - h - 4;
      setStyle({ top, left, width });
    };
    place();
    const ro = new ResizeObserver(place);
    if (ref.current) ro.observe(ref.current);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, anchor, align, width]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        anchor.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [open, onClose, anchor]);

  if (!open) return null;
  return createPortal(
    <div className={`popover ${className}`} ref={ref} style={style} role="dialog">
      {children}
    </div>,
    document.body,
  );
}

/** A button that toggles a popover menu. */
export function MenuButton({
  label,
  children,
  className = "btn",
  align = "start",
  title,
  width,
}: {
  label: ReactNode;
  children: (close: () => void) => ReactNode;
  className?: string;
  align?: "start" | "end";
  title?: string;
  width?: number;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button ref={ref} className={className} onClick={() => setOpen((o) => !o)} aria-expanded={open} aria-haspopup="menu" title={title} aria-label={title}>
        {label}
      </button>
      <Popover anchor={ref} open={open} onClose={() => setOpen(false)} align={align} width={width}>
        {children(() => setOpen(false))}
      </Popover>
    </>
  );
}

// ── Feedback ──────────────────────────────────────────────────────────────

export function Skeleton({ width = "100%", height = 14, style }: { width?: number | string; height?: number | string; style?: CSSProperties }) {
  return <div className="skeleton" style={{ width, height, ...style }} aria-hidden="true" />;
}

export function SkeletonRows({ rows = 6, height = 34 }: { rows?: number; height?: number }) {
  return (
    <div className="col" style={{ padding: 16, gap: 10 }} aria-busy="true" aria-label="Loading">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} height={height - 14} width={`${92 - ((i * 17) % 30)}%`} />
      ))}
    </div>
  );
}

export function Empty({ icon = "sparkle", title, children, action }: { icon?: IconName; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <div className="art">
        <Icon name={icon} size={28} strokeWidth={1.3} />
      </div>
      <h3>{title}</h3>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

/** Uniform error rendering; "not available" (endpoint missing) reads as a state, not a failure. */
export function ErrorState({ error, retry, compact }: { error: unknown; retry?: () => void; compact?: boolean }) {
  if (error instanceof ApiError && error.notAvailable) {
    return (
      <div className={compact ? "notice" : "empty"}>
        {!compact && (
          <div className="art">
            <Icon name="clock" size={28} strokeWidth={1.3} />
          </div>
        )}
        {compact && <Icon name="info" />}
        <div>
          <h3 style={compact ? { fontSize: 13 } : undefined}>Not available on this server yet</h3>
          <p className={compact ? "small" : undefined}>This Hoglet build doesn't serve this data yet. Upgrade the binary to enable it.</p>
        </div>
      </div>
    );
  }
  const message = errorMessage(error);
  const requestId = error instanceof ApiError ? error.requestId : null;
  return (
    <div className={compact ? "notice bad" : "empty"}>
      {!compact && (
        <div className="art">
          <Icon name="alert" size={28} strokeWidth={1.3} />
        </div>
      )}
      {compact && <Icon name="alert" />}
      <div className={compact ? "grow" : undefined}>
        <h3 style={compact ? { fontSize: 13 } : undefined}>Couldn't load this</h3>
        <p className={compact ? "small" : undefined}>
          {message}
          {requestId && <span className="muted mono"> · {requestId.slice(-8)}</span>}
        </p>
      </div>
      {retry && (
        <button className="btn small" onClick={retry}>
          <Icon name="refresh" size={14} /> Retry
        </button>
      )}
    </div>
  );
}

export function LoadingBar({ show }: { show: boolean }) {
  return show ? <div className="loading-bar" role="progressbar" aria-label="Loading" /> : null;
}

// ── Toasts ────────────────────────────────────────────────────────────────

interface ToastItem {
  id: number;
  text: string;
  bad?: boolean;
}
let toasts: ToastItem[] = [];
let toastId = 0;
const toastListeners = new Set<() => void>();
function emitToasts() {
  for (const l of toastListeners) l();
}
export function toast(text: string, bad = false): void {
  const id = ++toastId;
  toasts = [...toasts.slice(-3), { id, text, bad }];
  emitToasts();
  window.setTimeout(() => {
    toasts = toasts.filter((t) => t.id !== id);
    emitToasts();
  }, bad ? 6000 : 3000);
}
export function Toasts() {
  const list = useSyncExternalStore(
    (l) => {
      toastListeners.add(l);
      return () => toastListeners.delete(l);
    },
    () => toasts,
  );
  return (
    <div className="toasts" role="status" aria-live="polite">
      {list.map((t) => (
        <div key={t.id} className={`toast${t.bad ? " bad" : ""}`}>
          <Icon name={t.bad ? "alert" : "check"} />
          {t.text}
        </div>
      ))}
    </div>
  );
}

// ── Controls ──────────────────────────────────────────────────────────────

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      className="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation();
        onChange(!checked);
      }}
    />
  );
}

export function Seg<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: ReactNode; title?: string }[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div className="seg" role="group" aria-label={label}>
      {options.map((o) => (
        <button key={o.value} type="button" aria-pressed={o.value === value} onClick={() => onChange(o.value)} title={o.title}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Tabs<T extends string>({ value, options, onChange }: { value: T; options: { value: T; label: ReactNode }[]; onChange: (v: T) => void }) {
  return (
    <div className="tabs" role="tablist">
      {options.map((o) => (
        <button key={o.value} role="tab" aria-selected={o.value === value} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

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

export function CopyButton({ text, label = "Copy", className = "btn small" }: { text: string; label?: string; className?: string }) {
  const [done, setDone] = useState(false);
  useEffect(() => {
    if (!done) return;
    const id = window.setTimeout(() => setDone(false), 1400);
    return () => window.clearTimeout(id);
  }, [done]);
  return (
    <button
      type="button"
      className={className}
      onClick={async () => {
        if (await copyText(text)) setDone(true);
      }}
      aria-label={label}
    >
      <Icon name={done ? "check" : "copy"} size={14} />
      {label && <span>{done ? "Copied" : label}</span>}
    </button>
  );
}

export function Snippet({ code, language }: { code: string; language?: string }) {
  return (
    <div className="snippet" data-lang={language}>
      <CopyButton text={code} className="btn small copy" />
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}

/** Syntax-colored, read-only JSON. */
export function JsonView({ value }: { value: unknown }) {
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
        <span key={i++} className={m[2] ? "k" : "s"}>
          {m[1]}
        </span>,
      );
      if (m[2]) parts.push(m[2]);
    } else if (m[3]) parts.push(<span key={i++} className="b">{m[3]}</span>);
    else if (m[4]) parts.push(<span key={i++} className="n">{m[4]}</span>);
    last = re.lastIndex;
  }
  parts.push(text.slice(last));
  return <pre className="json mono">{parts}</pre>;
}

const AVATAR_COLORS = ["var(--s1)", "var(--s2)", "var(--s3)", "var(--s7)", "var(--s5)", "var(--s6)", "var(--s8)", "var(--s4)"];
export function Avatar({ name, id, large }: { name: string; id: string; large?: boolean }) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  const initialsText = name
    .replace(/@.*/, "")
    .split(/[ ._-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((s) => s[0]?.toUpperCase() ?? "")
    .join("");
  return (
    <span className={`avatar${large ? " large" : ""}`} style={{ background: AVATAR_COLORS[Math.abs(h) % AVATAR_COLORS.length] }} aria-hidden="true">
      {initialsText || "?"}
    </span>
  );
}

/** Click-to-edit text (insight and dashboard names). */
export function InlineEdit({
  value,
  onSave,
  placeholder,
  className = "",
  ariaLabel,
}: {
  value: string;
  onSave: (v: string) => void;
  placeholder?: string;
  className?: string;
  ariaLabel: string;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  if (!editing) {
    return (
      <button
        type="button"
        className={`inline-edit ${className}`}
        onClick={() => setEditing(true)}
        aria-label={`${ariaLabel}: ${value || placeholder}. Click to rename`}
        style={{ border: 0, background: "none", padding: 0, font: "inherit", color: "inherit", cursor: "text", textAlign: "left", maxWidth: "100%" }}
      >
        <span className="truncate" style={{ display: "block" }}>
          {value || <span className="muted">{placeholder}</span>}
        </span>
      </button>
    );
  }
  const commit = () => {
    setEditing(false);
    const next = draft.trim();
    if (next && next !== value) onSave(next);
    else setDraft(value);
  };
  return (
    <input
      className={`input ${className}`}
      autoFocus
      value={draft}
      placeholder={placeholder}
      aria-label={ariaLabel}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") {
          setDraft(value);
          setEditing(false);
        }
      }}
      style={{ fontSize: "inherit", fontWeight: "inherit", height: 34, width: "min(520px, 100%)" }}
    />
  );
}
