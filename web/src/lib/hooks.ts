import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { isAbort } from "./api";

// ── Data loading ───────────────────────────────────────────────────────────
//
// Stale-while-revalidate: results are cached by key (bounded), so revisiting
// a page paints instantly from cache while a fresh request runs. Previous
// data is kept on screen during reloads; no layout shift, no spinner flash.

const CACHE_LIMIT = 200;
const cache = new Map<string, unknown>();

function remember(key: string, value: unknown): void {
  cache.delete(key);
  cache.set(key, value);
  if (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
}

export function invalidate(prefix: string): void {
  for (const key of [...cache.keys()]) if (key.startsWith(prefix)) cache.delete(key);
}

export interface Async<T> {
  data: T | undefined;
  error: unknown;
  loading: boolean;
  reload: () => void;
  setData: (value: T) => void;
}

export function useApi<T>(
  key: string | null,
  load: (signal: AbortSignal) => Promise<T>,
  options: { pollMs?: number; keepPrevious?: boolean } = {},
): Async<T> {
  const { pollMs, keepPrevious = true } = options;
  const [state, setState] = useState<{ key: string | null; data: T | undefined; error: unknown; loading: boolean }>(() => ({
    key,
    data: key !== null ? (cache.get(key) as T | undefined) : undefined,
    error: null,
    loading: key !== null,
  }));
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;

  // Key changed: show cached data for the new key (or keep the old data).
  if (state.key !== key) {
    const cached = key !== null ? (cache.get(key) as T | undefined) : undefined;
    setState({ key, data: cached ?? (keepPrevious ? state.data : undefined), error: null, loading: key !== null });
  }

  useEffect(() => {
    if (key === null) return;
    const controller = new AbortController();
    setState((s) => (s.loading ? s : { ...s, loading: true }));
    loadRef
      .current(controller.signal)
      .then((data) => {
        if (controller.signal.aborted) return;
        remember(key, data);
        setState({ key, data, error: null, loading: false });
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted || isAbort(error)) return;
        setState((s) => ({ ...s, key, error, loading: false }));
      });
    return () => controller.abort();
  }, [key, tick]);

  useEffect(() => {
    const onChange = () => setTick((t) => t + 1);
    window.addEventListener("hoglet:data-changed", onChange);
    return () => window.removeEventListener("hoglet:data-changed", onChange);
  }, []);

  useEffect(() => {
    if (!pollMs || key === null) return;
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") setTick((t) => t + 1);
    }, pollMs);
    return () => window.clearInterval(id);
  }, [pollMs, key]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  const setData = useCallback(
    (value: T) => {
      if (key !== null) remember(key, value);
      setState((s) => ({ ...s, data: value }));
    },
    [key],
  );
  return { data: state.data, error: state.error, loading: state.loading, reload, setData };
}

// ── Small utilities ────────────────────────────────────────────────────────

export function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(value), ms);
    return () => window.clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

export function useSize<E extends HTMLElement>(): [RefObject<E | null>, { width: number; height: number }] {
  const ref = useRef<E>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setSize((s) => (Math.abs(s.width - r.width) < 0.5 && Math.abs(s.height - r.height) < 0.5 ? s : { width: r.width, height: r.height }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size];
}

export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}

export function useLocalStorage<T>(key: string, initial: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? initial : (JSON.parse(raw) as T);
    } catch {
      return initial;
    }
  });
  const set = useCallback(
    (next: T) => {
      setValue(next);
      try {
        localStorage.setItem(key, JSON.stringify(next));
      } catch {
        // Storage full or disabled: keep the in-memory value.
      }
    },
    [key],
  );
  return [value, set];
}

export function useOnClickOutside(refs: RefObject<HTMLElement | null>[], handler: () => void, active: boolean): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;
  useEffect(() => {
    if (!active) return;
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (refs.some((r) => r.current?.contains(target))) return;
      handlerRef.current();
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);
}

export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}
