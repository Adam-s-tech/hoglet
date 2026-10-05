//! Hoglet — PostHog-compatible product analytics. One binary.
//!
//! The wire contract lives in `spec/wire-compat.md` at the repo root; every handler
//! cites the section it implements. The supported PostHog wire subset stays
//! stable at the edge; Hoglet-specific behavior remains behind it.

// Handlers return `Result<T, Response>` so `?` short-circuits to the HTTP error
// directly; the large `Err` is on the cold path.
#![allow(clippy::result_large_err)]

pub mod application;
pub mod cache;
pub mod capture;
pub mod contract;
pub mod control;
pub mod control_members;
pub mod control_resources;
pub mod demo;
pub mod enrichment;
pub mod explore;
pub mod fault;
pub mod flags;
pub mod forward;
pub mod import;
pub mod lake;
pub mod metrics;
pub mod persons;
pub mod pipeline;
pub mod projection_catalog;
pub mod projections;
pub mod query;
pub mod reconcile;
pub mod ratelimit;
pub mod routes;
pub mod security;
pub mod server;
pub mod sink;
pub mod source;
pub mod storage_bootstrap;
pub mod token;
pub mod user_admin;

