// Every read the dashboard makes, as TanStack Query options.
//
// Key shape: ["p", projectId, <resource>, ...args] for project data, so one
// `invalidateQueries({ queryKey: qk.project(id) })` refreshes everything for a
// project and `qk.flags(id)` refreshes the flag list and every flag detail.
// Every queryFn receives the abort signal, so leaving a page (or typing the
// next character into a search box) cancels the in-flight request.
//
// staleTime is tuned per endpoint: cheap/changing reads (status, live feed)
// are seconds, saved objects are 30s, the event/property catalog and
// insight results are minutes.

import { infiniteQueryOptions, keepPreviousData, queryOptions } from "@tanstack/react-query";
import type { ActorsRequest } from "../types/ActorsRequest";
import type { QueryRequest } from "../types/QueryRequest";
import type { WebDimension } from "../types/WebDimension";
import type { WebQuery } from "../types/WebQuery";
import { ApiError, api, type Workspace } from "./api";

// ── Session ──────────────────────────────────────────────────────────────

export type Boot = { state: "setup" } | { state: "login" } | { state: "ready"; workspace: Workspace };

export const bootKey = ["boot"] as const;

/** First paint: is this a fresh install, a signed-out browser, or a live session? */
export const bootQuery = queryOptions({
  queryKey: bootKey,
  queryFn: async ({ signal }): Promise<Boot> => {
    const { setup_required } = await api.bootstrap(signal);
    if (setup_required) return { state: "setup" };
    try {
      return { state: "ready", workspace: await api.me(signal) };
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) return { state: "login" };
      throw e;
    }
  },
  staleTime: Infinity,
  gcTime: Infinity,
  retry: false,
});

// ── Keys ─────────────────────────────────────────────────────────────────

export const qk = {
  project: (pid: string) => ["p", pid] as const,
  status: (pid: string) => ["p", pid, "status"] as const,
  insights: (pid: string) => ["p", pid, "insights"] as const,
  insight: (pid: string, id: string) => ["p", pid, "insights", id] as const,
  dashboards: (pid: string) => ["p", pid, "dashboards"] as const,
  dashboard: (pid: string, id: string) => ["p", pid, "dashboards", id] as const,
  shares: (pid: string) => ["p", pid, "shares"] as const,
  flags: (pid: string) => ["p", pid, "flags"] as const,
  flag: (pid: string, id: number) => ["p", pid, "flags", id] as const,
  persons: (pid: string) => ["p", pid, "persons"] as const,
  person: (pid: string, id: string) => ["p", pid, "person", id] as const,
  events: (pid: string) => ["p", pid, "events"] as const,
  query: (pid: string) => ["p", pid, "query"] as const,
  web: (pid: string) => ["p", pid, "web"] as const,
  catalog: (pid: string) => ["p", pid, "catalog"] as const,
  keys: ["keys"] as const,
  share: (token: string) => ["share", token] as const,
};

// ── Project health ───────────────────────────────────────────────────────

/** `pollMs`: the freshness pill polls every 10s, onboarding every 3s while it waits for a first event. */
export const statusQuery = (pid: string, pollMs?: number) =>
  queryOptions({
    queryKey: qk.status(pid),
    queryFn: ({ signal }) => api.status(pid, signal),
    staleTime: 5_000,
    refetchInterval: pollMs ?? false,
  });

// ── Saved objects ────────────────────────────────────────────────────────

export const insightsQuery = (pid: string) =>
  queryOptions({ queryKey: qk.insights(pid), queryFn: ({ signal }) => api.insights(pid, signal) });

export const insightQuery = (pid: string, id: string) =>
  queryOptions({ queryKey: qk.insight(pid, id), queryFn: ({ signal }) => api.insight(pid, id, signal) });

export const dashboardsQuery = (pid: string) =>
  queryOptions({ queryKey: qk.dashboards(pid), queryFn: ({ signal }) => api.dashboards(pid, signal) });

export const dashboardQuery = (pid: string, id: string) =>
  queryOptions({ queryKey: qk.dashboard(pid, id), queryFn: ({ signal }) => api.dashboard(pid, id, signal) });

export const sharesQuery = (pid: string) =>
  queryOptions({ queryKey: qk.shares(pid), queryFn: ({ signal }) => api.shares(pid, signal) });

export const flagsQuery = (pid: string) =>
  queryOptions({ queryKey: qk.flags(pid), queryFn: ({ signal }) => api.flags(pid, signal) });

export const flagQuery = (pid: string, id: number) =>
  queryOptions({ queryKey: qk.flag(pid, id), queryFn: ({ signal }) => api.flag(pid, id, signal) });

export const keysQuery = queryOptions({ queryKey: qk.keys, queryFn: ({ signal }) => api.listKeys(signal) });

/** Public share links are capability tokens, resolved without a session. */
export const publicShareQuery = (token: string) =>
  queryOptions({ queryKey: qk.share(token), queryFn: ({ signal }) => api.publicShare(token, signal), retry: false });

// ── Persons & events ─────────────────────────────────────────────────────

/** Hard page cap: pages beyond this are dropped from memory (and refetches), never unbounded. */
const MAX_PAGES = 20;

export const personsQuery = (pid: string, search: string, limit = 50) =>
  infiniteQueryOptions({
    queryKey: [...qk.persons(pid), search, limit] as const,
    queryFn: ({ signal, pageParam }) => api.persons(pid, { search, cursor: pageParam, limit }, signal),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
    maxPages: MAX_PAGES,
    placeholderData: keepPreviousData,
  });

export const personQuery = (pid: string, id: string) =>
  queryOptions({ queryKey: qk.person(pid, id), queryFn: ({ signal }) => api.person(pid, id, signal) });

export const personEventsQuery = (pid: string, personId: string, pollMs: number | false = 15_000, limit = 100) =>
  infiniteQueryOptions({
    queryKey: [...qk.person(pid, personId), "events", limit] as const,
    queryFn: ({ signal, pageParam }) => api.personEvents(pid, personId, { before: pageParam, limit }, signal),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_before,
    maxPages: MAX_PAGES,
    staleTime: 5_000,
    refetchInterval: pollMs,
  });

/** The activity feed. `pollMs` is false when the user pauses live updates. */
export const eventsQuery = (pid: string, f: { event: string | null; personId: string | null }, pollMs: number | false, limit = 100) =>
  infiniteQueryOptions({
    queryKey: [...qk.events(pid), f.event ?? "", f.personId ?? "", limit] as const,
    queryFn: ({ signal, pageParam }) => api.events(pid, { event: f.event, person_id: f.personId, before: pageParam, limit }, signal),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_before,
    maxPages: MAX_PAGES,
    staleTime: 2_000,
    refetchInterval: pollMs,
    placeholderData: keepPreviousData,
  });

// ── Analytics ────────────────────────────────────────────────────────────

/** An insight result. The server caches too; `refresh` (a mutation-like bypass) is passed per call, not keyed. */
export const insightResultQuery = (pid: string, request: QueryRequest) =>
  queryOptions({
    queryKey: [...qk.query(pid), JSON.stringify(request.query)] as const,
    queryFn: ({ signal }) => api.query(pid, request, signal),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  });

export const actorsQuery = (pid: string, request: ActorsRequest) =>
  queryOptions({
    queryKey: [...qk.query(pid), "actors", JSON.stringify(request)] as const,
    queryFn: ({ signal }) => api.actors(pid, request, signal),
    staleTime: 60_000,
  });

export const webOverviewQuery = (pid: string, q: WebQuery) =>
  queryOptions({
    queryKey: [...qk.web(pid), "overview", JSON.stringify(q)] as const,
    queryFn: ({ signal }) => api.webOverview(pid, q, signal),
    staleTime: 15_000,
    refetchInterval: 30_000,
    placeholderData: keepPreviousData,
  });

export const webBreakdownQuery = (pid: string, q: WebQuery, dimension: WebDimension, limit: number) =>
  queryOptions({
    queryKey: [...qk.web(pid), "breakdown", dimension, limit, JSON.stringify(q)] as const,
    queryFn: ({ signal }) => api.webBreakdown(pid, q, dimension, limit, signal),
    staleTime: 15_000,
  });

// ── Catalog (event / property pickers) ───────────────────────────────────

const CATALOG_STALE = 5 * 60_000;

export const catalogEventsQuery = (pid: string, search: string) =>
  queryOptions({
    queryKey: [...qk.catalog(pid), "events", search] as const,
    queryFn: ({ signal }) => api.catalogEvents(pid, search, signal),
    staleTime: CATALOG_STALE,
    placeholderData: keepPreviousData,
  });

export const catalogPropertiesQuery = (pid: string, type: "event" | "person", search = "") =>
  queryOptions({
    queryKey: [...qk.catalog(pid), "properties", type, search] as const,
    queryFn: ({ signal }) => api.catalogProperties(pid, type, search, signal),
    staleTime: CATALOG_STALE,
  });

export const catalogValuesQuery = (pid: string, key: string, type: "event" | "person", search: string) =>
  queryOptions({
    queryKey: [...qk.catalog(pid), "values", type, key, search] as const,
    queryFn: ({ signal }) => api.catalogValues(pid, key, type, search, signal),
    staleTime: CATALOG_STALE,
    placeholderData: keepPreviousData,
  });
