//! `ratchetd`: the Ratchet daemon.
//!
//! One long-lived local process that owns the ledger and serves the API on a unix socket: REST resources over
//! HTTP plus one server-sent-event stream (`/v1/events`). The Mac app (Swift client generated from the OpenAPI
//! document) and the `ratchet` CLI are both clients of the same socket. See `docs/NATIVE_ARCHITECTURE.md`.
//!
//! - [`api`]: the contract, request/response types with `serde` + `utoipa::ToSchema` (always compiled);
//! - [`service`]: the `AppService` trait every route calls (feature `server`);
//! - [`views`]: ledger rows -> view models, shared by services (feature `server`);
//! - [`sim`]: the simulation service, seeded demo data and scripted agents (feature `server`);
//! - [`routes`], [`openapi`], [`hub`], [`auth`], [`server`], [`launchagent`]: the HTTP side (feature `server`).

pub mod api;

#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod hub;
#[cfg(feature = "server")]
pub mod launchagent;
#[cfg(feature = "server")]
pub mod openapi;
pub mod paths;
#[cfg(feature = "engine")]
pub mod real;
#[cfg(feature = "server")]
pub mod routes;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod service;
#[cfg(feature = "server")]
pub mod sim;
#[cfg(feature = "server")]
pub mod views;

/// Crate version (`GET /v1/status`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
