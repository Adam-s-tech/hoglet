// Imperative navigation for places that are not links (row clicks, after a
// mutation, keyboard shortcuts). `to` is a full href: path, optional ?search.
// Typed `<Link>` / `useNavigate` from @tanstack/react-router work too; this
// just avoids threading a hook through event handlers.

import type { AnyRouter } from "@tanstack/react-router";

let current: AnyRouter | null = null;

export function setRouter(router: AnyRouter): void {
  current = router;
}

export function navigate(to: string, options: { replace?: boolean } = {}): void {
  if (!current) throw new Error("router not ready");
  if (options.replace) current.history.replace(to);
  else current.history.push(to);
}
