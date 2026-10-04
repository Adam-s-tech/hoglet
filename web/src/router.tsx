// TanStack Router, code-based (no files, no generator, no SSR): a client-only
// SPA. The Rust server answers every non-API path with index.html, so deep
// links just work; this tree decides what to render.
//
//   /                                   → last used project
//   /share/$token                       public share (no session)
//   /project/$projectId                 app shell
//     /  /onboarding /web /insights /activity /persons /flags /dashboards /settings
//     /insights/new?kind=&q=            builder state lives in typed search params
//     /insights/$id  /persons/$id  /flags/new  /flags/$id  /dashboards/$id
//
// Pages are lazy chunks, so first paint (login, home) never pays for the
// insight builder or the charts.

import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  lazyRouteComponent,
  notFound,
  redirect,
  type AnyRouter,
} from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";
import { decodeQuery, normalizeQuery } from "@/insight/defaults";
import type { InsightQuery } from "@/types/InsightQuery";
import { NotFound, PageFallback, ProjectLayout, Root, RouteError, Landing } from "@/App";
import { setRouter } from "@/lib/nav";

export interface RouterContext {
  queryClient: QueryClient;
}

const rootRoute = createRootRouteWithContext<RouterContext>()({
  component: Root,
  notFoundComponent: Landing,
});

const shareRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "share/$token",
  component: lazyRouteComponent(() => import("@/pages/Share"), "SharePage"),
});

const indexRoute = createRoute({ getParentRoute: () => rootRoute, path: "/", component: Landing });

const projectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "project/$projectId",
  component: ProjectLayout,
});

const child = <P extends string>(path: P) => ({ getParentRoute: () => projectRoute, path }) as const;

const homeRoute = createRoute({ ...child("/"), component: lazyRouteComponent(() => import("@/pages/Home"), "HomePage") });
const onboardingRoute = createRoute({ ...child("onboarding"), component: lazyRouteComponent(() => import("@/pages/Onboarding"), "OnboardingPage") });
const webRoute = createRoute({ ...child("web"), component: lazyRouteComponent(() => import("@/pages/Web"), "WebPage") });

const insightsRoute = createRoute({ ...child("insights"), component: lazyRouteComponent(() => import("@/pages/Insights"), "InsightsPage") });

/** `?kind=trends` picks the starting kind; `?q=` is the whole query (JSON, validated by normalizeQuery). */
export interface NewInsightSearch {
  kind?: string;
  q?: InsightQuery;
}

/** Old links carried the builder state in the hash (`#q=<encoded json>`); fold them into search params. */
function legacyHashRedirect(location: { hash?: string; pathname: string; search: unknown }): void {
  const hash = (location.hash ?? "").replace(/^#/, "");
  if (!hash.startsWith("q=")) return;
  const q = decodeQuery(hash.slice(2));
  const kind = (location.search as { kind?: unknown } | undefined)?.kind;
  throw redirect({
    to: location.pathname,
    search: { kind: typeof kind === "string" ? kind : undefined, q: q ?? undefined },
    hash: "",
    replace: true,
  });
}

const newInsightRoute = createRoute({
  ...child("insights/new"),
  validateSearch: (raw: Record<string, unknown>): NewInsightSearch => {
    let q = raw.q;
    if (typeof q === "string") {
      try {
        q = JSON.parse(q);
      } catch {
        q = undefined;
      }
    }
    return {
      kind: typeof raw.kind === "string" ? raw.kind : undefined,
      q: q === undefined ? undefined : (normalizeQuery(q) ?? undefined),
    };
  },
  beforeLoad: ({ location }) => legacyHashRedirect(location),
  component: lazyRouteComponent(() => import("@/pages/Insight"), "NewInsightPage"),
});

const insightRoute = createRoute({ ...child("insights/$id"), component: lazyRouteComponent(() => import("@/pages/Insight"), "SavedInsightPage") });

export interface ActivitySearch {
  event?: string;
  person_id?: string;
}
const activityRoute = createRoute({
  ...child("activity"),
  validateSearch: (raw: Record<string, unknown>): ActivitySearch => ({
    event: typeof raw.event === "string" && raw.event ? raw.event : undefined,
    person_id: typeof raw.person_id === "string" && raw.person_id ? raw.person_id : undefined,
  }),
  component: lazyRouteComponent(() => import("@/pages/Activity"), "ActivityPage"),
});

const personsRoute = createRoute({ ...child("persons"), component: lazyRouteComponent(() => import("@/pages/Persons"), "PersonsPage") });
const personRoute = createRoute({ ...child("persons/$id"), component: lazyRouteComponent(() => import("@/pages/Persons"), "PersonPage") });

const flagsRoute = createRoute({ ...child("flags"), component: lazyRouteComponent(() => import("@/pages/Flags"), "FlagsPage") });
const newFlagRoute = createRoute({ ...child("flags/new"), component: lazyRouteComponent(() => import("@/pages/Flags"), "NewFlagPage") });
const flagRoute = createRoute({
  getParentRoute: () => projectRoute,
  path: "flags/$id",
  // Flag ids are integers; anything else is a 404, not a request to the API.
  params: {
    parse: (p: { id: string }) => {
      if (!/^\d+$/.test(p.id)) throw notFound();
      return { id: Number(p.id) };
    },
    stringify: (p: { id: number }) => ({ id: String(p.id) }),
  },
  component: lazyRouteComponent(() => import("@/pages/Flags"), "EditFlagPage"),
});

const dashboardsRoute = createRoute({ ...child("dashboards"), component: lazyRouteComponent(() => import("@/pages/Dashboards"), "DashboardsPage") });
const dashboardRoute = createRoute({ ...child("dashboards/$id"), component: lazyRouteComponent(() => import("@/pages/Dashboards"), "DashboardPage") });

export interface SettingsSearch {
  tab?: "project" | "keys" | "account";
}
const settingsRoute = createRoute({
  ...child("settings"),
  validateSearch: (raw: Record<string, unknown>): SettingsSearch => ({
    tab: raw.tab === "project" || raw.tab === "keys" || raw.tab === "account" ? raw.tab : undefined,
  }),
  component: lazyRouteComponent(() => import("@/pages/Settings"), "SettingsPage"),
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  shareRoute,
  projectRoute.addChildren([
    homeRoute,
    onboardingRoute,
    webRoute,
    insightsRoute,
    newInsightRoute,
    insightRoute,
    activityRoute,
    personsRoute,
    personRoute,
    flagsRoute,
    newFlagRoute,
    flagRoute,
    dashboardsRoute,
    dashboardRoute,
    settingsRoute,
  ]),
]);

export function makeRouter(queryClient: QueryClient) {
  const router = createRouter({
    routeTree,
    context: { queryClient },
    scrollRestoration: true,
    defaultPendingComponent: PageFallback,
    defaultPendingMs: 120,
    defaultErrorComponent: RouteError,
    defaultNotFoundComponent: NotFound,
  });
  setRouter(router as unknown as AnyRouter);
  return router;
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof makeRouter>;
  }
}
