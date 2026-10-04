//! The dashboard/API wire contract.
//!
//! Every JSON shape the dashboard reads or writes is defined here, once, and
//! exported to TypeScript by ts-rs (`cargo test` regenerates
//! `web/src/types/`). Route handlers serialize these types; they do not
//! invent parallel ones. Changes are additive only.
//!
//! Endpoint map (all under `/api/projects/{project_id}`, session cookie or
//! personal API key):
//!
//! | Method & path | Body → response |
//! |---|---|
//! | `POST /query` | `QueryRequest` → `QueryResponse` |
//! | `POST /query/actors` | `ActorsRequest` → `ActorsResponse` |
//! | `GET /persons` | → `PersonListResponse` |
//! | `GET /persons/{id}` | → `PersonDetail` |
//! | `GET /persons/{id}/events` | → `EventListResponse` |
//! | `POST /persons/{id}/erase` | → `{distinct_ids, events}` (owner/admin; physical) |
//! | `POST /demo` | → `{events}` — fill the project with demo data |
//! | `GET/PUT /forwarding` | `{enabled, host, posthog_token}` → forwarding status (shadow mode) |
//! | `GET /events` | → `EventListResponse` |
//! | `GET /web/overview` | `WebQuery` (query string) → `WebOverview` |
//! | `GET /web/breakdown` | `WebQuery` + `dimension`, `limit` → `WebBreakdown` |
//! | `GET /catalog/events` | → `Vec<CatalogEvent>` |
//! | `GET /catalog/properties` | → `Vec<CatalogProperty>` |
//! | `GET /catalog/values` | → `Vec<CatalogValue>` |
//! | `GET /status` | → `ProjectStatus` |
//! | `GET/POST /feature_flags` | `FeatureFlagInput` → `FeatureFlag` |
//! | `GET/PATCH/DELETE /feature_flags/{id}` | → `FeatureFlag` |
//! | `GET /feature_flags/{id}/evaluate` | `distinct_id` → `FlagEvaluation` |
//!
//! Organization scope (`/api/organizations/{org_id}`; session cookie, reads
//! also accept a personal key; every change needs an owner/admin session):
//!
//! | Method & path | Body → response |
//! |---|---|
//! | `GET /members` | → `Vec<Member>` (any member) |
//! | `PATCH /members/{user_id}` | `UpdateMemberRequest` → `Member` (admins cannot touch owners) |
//! | `DELETE /members/{user_id}` | → 204 (also leaving; never the last owner) |
//! | `GET /invites` | → `Vec<Invite>` (owner/admin) |
//! | `POST /invites` | `CreateInviteRequest` → `CreatedInvite` (token shown once) |
//! | `DELETE /invites/{invite_id}` | → 204 (revoke) |
//! | `POST /api/invites/preview` | `InviteTokenRequest` → `InvitePreview` (no session) |
//! | `POST /api/invites/accept` | `AcceptInviteRequest` → `Workspace` + session cookie (no session) |
//!
//! Errors everywhere: `ApiError` with an HTTP status that matches.

pub mod common;
pub mod flags;
pub mod insight;
pub mod members;
pub mod persons;
pub mod web;
