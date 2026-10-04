// A tiny History-API router: one location store, a pattern matcher, <Link>.

import { useSyncExternalStore, type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from "react";

type Listener = () => void;
const listeners = new Set<Listener>();

function emit(): void {
  for (const l of listeners) l();
}

window.addEventListener("popstate", emit);

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot(): string {
  return window.location.pathname + window.location.search + window.location.hash;
}

export interface Location {
  path: string;
  search: URLSearchParams;
  hash: string;
  href: string;
}

let cachedHref = "";
let cachedLocation: Location | null = null;

function currentLocation(): Location {
  const href = snapshot();
  if (cachedLocation && cachedHref === href) return cachedLocation;
  cachedHref = href;
  cachedLocation = {
    path: window.location.pathname.replace(/\/+$/, "") || "/",
    search: new URLSearchParams(window.location.search),
    hash: window.location.hash.replace(/^#/, ""),
    href,
  };
  return cachedLocation;
}

export function useLocation(): Location {
  useSyncExternalStore(subscribe, snapshot);
  return currentLocation();
}

export function navigate(to: string, options: { replace?: boolean; keepScroll?: boolean } = {}): void {
  if (to === snapshot()) return;
  if (options.replace) window.history.replaceState(null, "", to);
  else window.history.pushState(null, "", to);
  if (!options.keepScroll && !options.replace) window.scrollTo(0, 0);
  emit();
}

/** Match `/project/:pid/insights/:id` against a path; returns params or null. */
export function match(pattern: string, path: string): Record<string, string> | null {
  const p = pattern.split("/").filter(Boolean);
  const s = path.split("/").filter(Boolean);
  const rest = p[p.length - 1] === "*";
  if (rest ? s.length < p.length - 1 : s.length !== p.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < p.length; i++) {
    const seg = p[i];
    if (seg === "*") {
      params["*"] = s.slice(i).join("/");
      return params;
    }
    if (seg.startsWith(":")) params[seg.slice(1)] = decodeURIComponent(s[i]);
    else if (seg !== s[i]) return null;
  }
  return params;
}

export function isPlainClick(e: MouseEvent): boolean {
  return e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;
}

interface LinkProps extends AnchorHTMLAttributes<HTMLAnchorElement> {
  to: string;
  children?: ReactNode;
}

export function Link({ to, onClick, children, ...rest }: LinkProps) {
  return (
    <a
      href={to}
      onClick={(e) => {
        onClick?.(e);
        if (e.defaultPrevented || !isPlainClick(e) || rest.target === "_blank") return;
        e.preventDefault();
        navigate(to);
      }}
      {...rest}
    >
      {children}
    </a>
  );
}
