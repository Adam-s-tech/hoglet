// Theme: "system" (default) follows the OS, "light"/"dark" are an explicit,
// persisted choice. Applied as a `.dark` class on <html> (shadcn convention);
// index.html resolves it before first paint so there is no flash.

import { useSyncExternalStore } from "react";

export type Theme = "system" | "light" | "dark";
const KEY = "hoglet.theme";
const listeners = new Set<() => void>();
const media = window.matchMedia?.("(prefers-color-scheme: dark)");

function read(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}

export function effectiveDark(theme: Theme): boolean {
  if (theme === "dark") return true;
  if (theme === "light") return false;
  return media?.matches ?? false;
}

function apply(theme: Theme): void {
  document.documentElement.classList.toggle("dark", effectiveDark(theme));
}

export function setTheme(theme: Theme): void {
  try {
    if (theme === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, theme);
  } catch {
    // Storage disabled: the choice lasts for this page view only.
  }
  apply(theme);
  for (const l of listeners) l();
}

// Follow the OS while the choice is "system".
media?.addEventListener("change", () => {
  if (read() === "system") apply("system");
});

export function useTheme(): Theme {
  return useSyncExternalStore((l) => {
    listeners.add(l);
    return () => listeners.delete(l);
  }, read);
}
