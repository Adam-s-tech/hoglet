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
//! Errors everywhere: `ApiError` with an HTTP status that matches.

pub mod common;
pub mod flags;
pub mod insight;
pub mod persons;
pub mod web;
