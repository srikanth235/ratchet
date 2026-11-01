//! The API contract: every request and response type of `ratchetd`.
//!
//! Plain Rust structs with `serde` (camelCase JSON) and `utoipa::ToSchema`. The OpenAPI document is generated from
//! them and the route annotations (`crate::openapi`), and the Swift client is generated from that document.
//! This module compiles without the `server` feature, so a client (the `ratchet` CLI) can share the types.
//!
//! Groups: [`vm`] (optimizations, candidates, agents, spend, activity, config), [`settings`] (settings, GitHub,
//! AWS, agent CLIs), [`repos`] (repos, environments), [`commands`] (bodies and queries), [`events`] (the SSE
//! union), and the provisional [`connectors`], [`overview`] and [`env_drafts`].

pub mod commands;
pub mod common;
pub mod connectors;
pub mod env_drafts;
pub mod events;
pub mod overview;
pub mod repos;
pub mod settings;
pub mod summary;
pub mod validate;
pub mod vm;

pub use commands::*;
pub use common::*;
pub use connectors::*;
pub use env_drafts::*;
pub use events::*;
pub use overview::*;
pub use repos::*;
pub use settings::*;
pub use summary::validate_config_text;
pub use vm::*;
