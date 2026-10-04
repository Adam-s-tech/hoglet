// The one typed client for Hoglet's dashboard API.
//
// Contract shapes come from src/types (generated from src/contract by ts-rs).
// Workspace/resource shapes that are not in the contract yet are declared
// here, once. Cookie auth (same-origin), uniform ApiError, 401 → login,
// every call abortable.

import type { ActorsRequest } from "../types/ActorsRequest";
import type { ActorsResponse } from "../types/ActorsResponse";
import type { CatalogEvent } from "../types/CatalogEvent";
import type { CatalogProperty } from "../types/CatalogProperty";
import type { CatalogValue } from "../types/CatalogValue";
import type { EventListResponse } from "../types/EventListResponse";
import type { FeatureFlag } from "../types/FeatureFlag";
import type { FeatureFlagInput } from "../types/FeatureFlagInput";
import type { FlagEvaluation } from "../types/FlagEvaluation";
import type { InsightQuery } from "../types/InsightQuery";
import type { PersonDetail } from "../types/PersonDetail";
import type { PersonListResponse } from "../types/PersonListResponse";
import type { ProjectStatus } from "../types/ProjectStatus";
import type { QueryRequest } from "../types/QueryRequest";
import type { QueryResponse } from "../types/QueryResponse";
import type { WebBreakdown } from "../types/WebBreakdown";
import type { WebDimension } from "../types/WebDimension";
import type { WebOverview } from "../types/WebOverview";
import type { WebQuery } from "../types/WebQuery";

// ── Workspace shapes (src/control.rs, src/control_resources.rs) ───────────

export type Role = "owner" | "admin" | "member";
export interface User {
  id: string;
  email: string;
  name: string;
}
export interface Project {
  id: string;
  name: string;
  token: string;
}
export interface Organization {
  id: string;
  name: string;
  role: Role;
  projects: Project[];
}
export interface Workspace {
  user: User;
  organizations: Organization[];
}
export type KeyScope = "read" | "write";
export interface PersonalApiKey {
  id: string;
  name: string;
  /** Older servers omit it; treat as read. */
  scope?: KeyScope;
  key_prefix: string;
  last_used: number | null;
  created_at: number;
}
export interface CreatedKey {
  key: PersonalApiKey;
  secret: string;
}
export interface SetupInput {
  email: string;
  password: string;
  organization_name: string;
  project_name: string;
}

export interface SavedInsight {
  id: string;
  project_id: string;
  name: string;
  description: string;
  /** `null` when the stored query predates the InsightQuery contract. */
  query: InsightQuery | null;
  created_by: string;
  created_at: number;
  updated_at: number;
}
export interface DashboardTile {
  insight_id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  insight?: SavedInsight;
}
export interface Dashboard {
  id: string;
  project_id: string;
  name: string;
  tiles: DashboardTile[];
  created_by: string;
  created_at: number;
}
export interface TileInput {
  insight_id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}
export interface ShareLink {
  id: string;
  project_id: string;
  object_type: "insight" | "dashboard";
  object_id: string;
  token: string;
  created_at: number;
  expires_at: number | null;
}
export interface PublicShare {
  share: ShareLink;
  insight?: SavedInsight;
  dashboard?: Dashboard;
}

// ── Errors ─────────────────────────────────────────────────────────────────

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly requestId: string | null;
  readonly field: string | null;

  constructor(status: number, code: string, message: string, requestId: string | null = null, field: string | null = null) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.requestId = requestId;
    this.field = field;
  }

  /** The server does not implement this endpoint (yet). */
  get notAvailable(): boolean {
    return this.code === "not_available";
  }
  get unauthorized(): boolean {
    return this.status === 401;
  }
}

export function isAbort(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof Error) return error.message;
  return "Something went wrong.";
}

// ── Transport ──────────────────────────────────────────────────────────────

export interface RawRequest {
  method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  url: string;
  body?: unknown;
  signal?: AbortSignal;
}
export interface RawResponse {
  status: number;
  body: unknown;
}
export type Transport = (request: RawRequest) => Promise<RawResponse>;

const fetchTransport: Transport = async ({ method, url, body, signal }) => {
  let response: Response;
  try {
    response = await fetch(url, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? { accept: "application/json" } : { accept: "application/json", "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    });
  } catch (error) {
    if (isAbort(error)) throw error;
    throw new ApiError(0, "network", "Can't reach the Hoglet server. Check that it is running.");
  }
  const text = await response.text();
  let parsed: unknown = null;
  if (text.length > 0) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = text;
    }
  }
  return { status: response.status, body: parsed };
};

const MOCK = import.meta.env.MODE === "mock";
let transportPromise: Promise<Transport> | null = null;
function transport(): Promise<Transport> {
  if (!transportPromise) {
    transportPromise = MOCK ? import("../mock").then((m) => m.mockTransport) : Promise.resolve(fetchTransport);
  }
  return transportPromise;
}
export const isMock = MOCK;

let unauthorizedListener: (() => void) | null = null;
/** The app installs one listener: any 401 outside the auth flow → login. */
export function onUnauthorized(listener: (() => void) | null): void {
  unauthorizedListener = listener;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function toApiError(status: number, body: unknown): ApiError {
  if (isRecord(body) && isRecord(body.error)) {
    const e = body.error;
    return new ApiError(
      status,
      typeof e.code === "string" ? e.code : "error",
      typeof e.message === "string" ? e.message : `Request failed (${status}).`,
      typeof e.request_id === "string" ? e.request_id : null,
      typeof e.field === "string" ? e.field : null,
    );
  }
  // A 404/405 without the uniform envelope comes from the router itself: the
  // endpoint does not exist on this server build.
  if (status === 404 || status === 405 || status === 501) {
    return new ApiError(status, "not_available", "This server doesn't provide this feature yet.");
  }
  if (status >= 500) return new ApiError(status, "server_error", `The server failed (${status}). Try again.`);
  return new ApiError(status, "error", `Request failed (${status}).`);
}

type QueryParams = Record<string, string | number | boolean | null | undefined>;

function withQuery(path: string, params?: QueryParams): string {
  if (!params) return path;
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "") continue;
    search.set(key, String(value));
  }
  const qs = search.toString();
  return qs ? `${path}?${qs}` : path;
}

async function request<T>(
  method: RawRequest["method"],
  path: string,
  options: { body?: unknown; query?: QueryParams; signal?: AbortSignal; authFlow?: boolean } = {},
): Promise<T> {
  const send = await transport();
  const { status, body } = await send({ method, url: withQuery(path, options.query), body: options.body, signal: options.signal });
  if (status >= 200 && status < 300) return body as T;
  const error = toApiError(status, body);
  if (status === 401 && !options.authFlow) unauthorizedListener?.();
  throw error;
}

// ── Normalizers (tolerate the pre-contract backend shapes) ─────────────────

const QUERY_KINDS = new Set(["TrendsQuery", "FunnelsQuery", "RetentionQuery", "LifecycleQuery", "StickinessQuery", "PathsQuery", "SqlQuery"]);

function normalizeInsight(raw: unknown): SavedInsight {
  const r = isRecord(raw) ? raw : {};
  const q = r.query ?? r.query_ir;
  return {
    id: String(r.id ?? ""),
    project_id: String(r.project_id ?? ""),
    name: String(r.name ?? "Untitled"),
    description: String(r.description ?? ""),
    query: isRecord(q) && typeof q.kind === "string" && QUERY_KINDS.has(q.kind) ? (q as unknown as InsightQuery) : null,
    created_by: String(r.created_by ?? ""),
    created_at: Number(r.created_at ?? 0),
    updated_at: Number(r.updated_at ?? r.created_at ?? 0),
  };
}

function normalizeDashboard(raw: unknown): Dashboard {
  const r = isRecord(raw) ? raw : {};
  const tiles = Array.isArray(r.tiles) ? r.tiles : [];
  return {
    id: String(r.id ?? ""),
    project_id: String(r.project_id ?? ""),
    name: String(r.name ?? "Untitled"),
    created_by: String(r.created_by ?? ""),
    created_at: Number(r.created_at ?? 0),
    tiles: tiles.filter(isRecord).map((t) => ({
      insight_id: String(t.insight_id ?? ""),
      x: Number(t.x ?? 0),
      y: Number(t.y ?? 0),
      w: Number(t.w ?? 4),
      h: Number(t.h ?? 3),
      insight: t.insight ? normalizeInsight(t.insight) : undefined,
    })),
  };
}

function normalizeCatalogEvents(raw: unknown): CatalogEvent[] {
  const list = Array.isArray(raw) ? raw : isRecord(raw) && Array.isArray(raw.events) ? raw.events : [];
  return list.map((e: unknown) => {
    if (typeof e === "string") return { name: e, count: 0, last_seen: null };
    const r = isRecord(e) ? e : {};
    return {
      name: String(r.name ?? r.event ?? ""),
      count: Number(r.count ?? 0),
      last_seen: typeof r.last_seen === "string" ? r.last_seen : null,
    };
  });
}

function normalizeCatalogProperties(raw: unknown, source: string): CatalogProperty[] {
  const list = Array.isArray(raw) ? raw : [];
  return list.map((p: unknown) => {
    if (typeof p === "string") return { key: p, type: source, property_type: "string", count: 0 };
    const r = isRecord(p) ? p : {};
    return {
      key: String(r.key ?? ""),
      type: String(r.type ?? r.source ?? source),
      property_type: String(r.property_type ?? r.type_guess ?? "string"),
      count: Number(r.count ?? 0),
    };
  });
}

function normalizeCatalogValues(raw: unknown): CatalogValue[] {
  const list = Array.isArray(raw) ? raw : [];
  return list.map((v: unknown) => {
    if (!isRecord(v)) return { value: String(v), count: 0 };
    return { value: String(v.value ?? ""), count: Number(v.count ?? 0) };
  });
}

// ── Endpoints ──────────────────────────────────────────────────────────────

const p = (projectId: string) => `/api/projects/${encodeURIComponent(projectId)}`;

function webParams(q: WebQuery): QueryParams {
  return {
    date_from: q.date_from,
    date_to: q.date_to,
    interval: q.interval,
    properties: q.properties.length ? JSON.stringify(q.properties) : undefined,
  };
}

export const api = {
  // Auth & workspace
  bootstrap: (signal?: AbortSignal) => request<{ setup_required: boolean }>("GET", "/api/auth/bootstrap", { signal, authFlow: true }),
  setup: (input: SetupInput) => request<Workspace>("POST", "/api/auth/setup", { body: input, authFlow: true }),
  login: (email: string, password: string) => request<Workspace>("POST", "/api/auth/login", { body: { email, password }, authFlow: true }),
  logout: () => request<{ status: string }>("POST", "/api/auth/logout", { authFlow: true }),
  me: (signal?: AbortSignal) => request<Workspace>("GET", "/api/auth/me", { signal, authFlow: true }),
  listKeys: (signal?: AbortSignal) => request<PersonalApiKey[]>("GET", "/api/auth/keys", { signal }),
  createKey: (name: string, scope: KeyScope) => request<CreatedKey>("POST", "/api/auth/keys", { body: { name, scope } }),
  revokeKey: (id: string) => request<null>("DELETE", `/api/auth/keys/${encodeURIComponent(id)}`),
  createOrganization: (name: string) => request<Organization>("POST", "/api/organizations", { body: { name } }),
  createProject: (organizationId: string, name: string) =>
    request<Project>("POST", `/api/organizations/${encodeURIComponent(organizationId)}/projects`, { body: { name } }),

  // Analytics
  query: (projectId: string, body: QueryRequest, signal?: AbortSignal) =>
    request<QueryResponse>("POST", `${p(projectId)}/query`, { body, signal }),
  actors: (projectId: string, body: ActorsRequest, signal?: AbortSignal) =>
    request<ActorsResponse>("POST", `${p(projectId)}/query/actors`, { body, signal }),
  webOverview: (projectId: string, q: WebQuery, signal?: AbortSignal) =>
    request<WebOverview>("GET", `${p(projectId)}/web/overview`, { query: webParams(q), signal }),
  webBreakdown: (projectId: string, q: WebQuery, dimension: WebDimension, limit: number, signal?: AbortSignal) =>
    request<WebBreakdown>("GET", `${p(projectId)}/web/breakdown`, { query: { ...webParams(q), dimension, limit }, signal }),
  status: (projectId: string, signal?: AbortSignal) => request<ProjectStatus>("GET", `${p(projectId)}/status`, { signal }),

  /** Fill the project with 90 days of realistic demo events. */
  loadDemo: (projectId: string) => request<{ events: number }>("POST", `${p(projectId)}/demo`),
  /** GDPR erase: the person, every distinct id, every event. Owner/admin only. */
  erasePerson: (projectId: string, personId: string) =>
    request<{ distinct_ids: number; events: number }>("POST", `${p(projectId)}/persons/${encodeURIComponent(personId)}/erase`),

  // Persons & events
  persons: (projectId: string, q: { search?: string; cursor?: string | null; limit?: number }, signal?: AbortSignal) =>
    request<PersonListResponse>("GET", `${p(projectId)}/persons`, { query: q, signal }),
  person: (projectId: string, personId: string, signal?: AbortSignal) =>
    request<PersonDetail>("GET", `${p(projectId)}/persons/${encodeURIComponent(personId)}`, { signal }),
  personEvents: (projectId: string, personId: string, q: { before?: string | null; limit?: number }, signal?: AbortSignal) =>
    request<EventListResponse>("GET", `${p(projectId)}/persons/${encodeURIComponent(personId)}/events`, { query: q, signal }),
  events: (projectId: string, q: { event?: string | null; person_id?: string | null; before?: string | null; limit?: number }, signal?: AbortSignal) =>
    request<EventListResponse>("GET", `${p(projectId)}/events`, { query: q, signal }),

  // Catalog (contract params: search, type, key, limit).
  catalogEvents: async (projectId: string, search: string, signal?: AbortSignal) =>
    normalizeCatalogEvents(await request<unknown>("GET", `${p(projectId)}/catalog/events`, { query: { search, limit: 200 }, signal })),
  catalogProperties: async (projectId: string, type: "event" | "person", search: string, signal?: AbortSignal) =>
    normalizeCatalogProperties(await request<unknown>("GET", `${p(projectId)}/catalog/properties`, { query: { type, search }, signal }), type),
  catalogValues: async (projectId: string, key: string, type: "event" | "person", search: string, signal?: AbortSignal) =>
    normalizeCatalogValues(await request<unknown>("GET", `${p(projectId)}/catalog/values`, { query: { key, type, search, limit: 50 }, signal })),

  // Feature flags
  flags: (projectId: string, signal?: AbortSignal) => request<FeatureFlag[]>("GET", `${p(projectId)}/feature_flags`, { signal }),
  flag: (projectId: string, id: number, signal?: AbortSignal) => request<FeatureFlag>("GET", `${p(projectId)}/feature_flags/${id}`, { signal }),
  createFlag: (projectId: string, input: FeatureFlagInput) => request<FeatureFlag>("POST", `${p(projectId)}/feature_flags`, { body: input }),
  updateFlag: (projectId: string, id: number, input: Partial<FeatureFlagInput>) =>
    request<FeatureFlag>("PATCH", `${p(projectId)}/feature_flags/${id}`, { body: input }),
  deleteFlag: (projectId: string, id: number) => request<null>("DELETE", `${p(projectId)}/feature_flags/${id}`),
  evaluateFlag: (projectId: string, id: number, distinctId: string, signal?: AbortSignal) =>
    request<FlagEvaluation>("GET", `${p(projectId)}/feature_flags/${id}/evaluate`, { query: { distinct_id: distinctId }, signal }),

  // Saved insights
  insights: async (projectId: string, signal?: AbortSignal) =>
    (await request<unknown[]>("GET", `${p(projectId)}/insights`, { signal })).map(normalizeInsight),
  insight: async (projectId: string, id: string, signal?: AbortSignal) =>
    normalizeInsight(await request<unknown>("GET", `${p(projectId)}/insights/${encodeURIComponent(id)}`, { signal })),
  createInsight: async (projectId: string, draft: { name: string; description: string; query: InsightQuery }) =>
    normalizeInsight(
      await request<unknown>("POST", `${p(projectId)}/insights`, {
        body: { name: draft.name, description: draft.description, query_ir: draft.query },
      }),
    ),
  updateInsight: async (projectId: string, id: string, draft: { name: string; description: string; query: InsightQuery }) =>
    normalizeInsight(
      await request<unknown>("PUT", `${p(projectId)}/insights/${encodeURIComponent(id)}`, {
        body: { name: draft.name, description: draft.description, query_ir: draft.query },
      }),
    ),
  deleteInsight: (projectId: string, id: string) => request<null>("DELETE", `${p(projectId)}/insights/${encodeURIComponent(id)}`),

  // Dashboards
  dashboards: async (projectId: string, signal?: AbortSignal) =>
    (await request<unknown[]>("GET", `${p(projectId)}/dashboards`, { signal })).map(normalizeDashboard),
  dashboard: async (projectId: string, id: string, signal?: AbortSignal) =>
    normalizeDashboard(await request<unknown>("GET", `${p(projectId)}/dashboards/${encodeURIComponent(id)}`, { signal })),
  createDashboard: async (projectId: string, name: string) =>
    normalizeDashboard(await request<unknown>("POST", `${p(projectId)}/dashboards`, { body: { name } })),
  renameDashboard: async (projectId: string, id: string, name: string) =>
    normalizeDashboard(await request<unknown>("PUT", `${p(projectId)}/dashboards/${encodeURIComponent(id)}`, { body: { name } })),
  deleteDashboard: (projectId: string, id: string) => request<null>("DELETE", `${p(projectId)}/dashboards/${encodeURIComponent(id)}`),
  replaceTiles: async (projectId: string, id: string, tiles: TileInput[]) =>
    normalizeDashboard(await request<unknown>("PUT", `${p(projectId)}/dashboards/${encodeURIComponent(id)}/tiles`, { body: tiles })),

  // Share links
  shares: (projectId: string, signal?: AbortSignal) => request<ShareLink[]>("GET", `${p(projectId)}/shares`, { signal }),
  createShare: (projectId: string, objectType: "insight" | "dashboard", objectId: string) =>
    request<ShareLink>("POST", `${p(projectId)}/shares`, { body: { object_type: objectType, object_id: objectId } }),
  deleteShare: (projectId: string, id: string) => request<null>("DELETE", `${p(projectId)}/shares/${encodeURIComponent(id)}`),
  publicShare: async (token: string, signal?: AbortSignal) => {
    const raw = await request<Record<string, unknown>>("GET", `/api/shares/${encodeURIComponent(token)}`, { signal, authFlow: true });
    return {
      share: raw.share as ShareLink,
      insight: raw.insight ? normalizeInsight(raw.insight) : undefined,
      dashboard: raw.dashboard ? normalizeDashboard(raw.dashboard) : undefined,
    } satisfies PublicShare;
  },

  // Wire edge: a real capture call, exactly what an SDK sends.
  captureTestEvent: (token: string, distinctId: string) =>
    request<unknown>("POST", "/capture/", {
      body: {
        api_key: token,
        event: "hoglet_test_event",
        distinct_id: distinctId,
        properties: { $lib: "hoglet-web", source: "onboarding" },
        timestamp: new Date().toISOString(),
      },
      authFlow: true,
    }),
};
