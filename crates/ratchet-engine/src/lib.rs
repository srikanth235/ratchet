//! The Ratchet engine and its runtimes: the Rust port of `packages/engine` and `packages/runtime`.
//!
//! - [`Engine`]: creates an optimization, runs the baseline, launches agents and drives the control loop (the ratchet
//!   rule, the verdict flow incl. the confirm run, budget / time / plateau stops, the kill switch, crash reconciliation)
//!   against the ports of `ratchet-core`. All durable state is in the ledger plus the `engine_kv` table.
//! - the automatic sandbox lifecycle (created at the first run, destroyed after the idle window, receipt with orphan
//!   check): `sandbox_auto`;
//! - helpers: [`CostMeter`], [`reap`], [`default_report`], [`KvStore`], bench statistics, settings, errors;
//! - the runtimes: `runtime::simulation`, `runtime::live`, `runtime::hybrid` (the engine wired to mock, real or mixed
//!   ports);
//! - `desktop`: the desktop main-process logic the daemon needs (secret store, live configuration, AWS profiles,
//!   harness detection);
//! - `testing` (feature): in-memory fakes of every port with one shared call log.
//!
//! See `PARITY.md` for the mapping from the TypeScript tests.

// Callback settings (`Arc<dyn Fn(..) -> ..>`) mirror the TypeScript option bags; aliases would hide the shapes.
#![allow(clippy::type_complexity)]

pub mod bench_stats;
pub mod cost;
pub mod deps;
pub mod desktop;
pub mod engine;
pub mod errors;
pub mod kv;
pub mod misc;
pub mod ports;
pub mod reaper;
pub mod report;
pub mod runtime;
pub mod sandbox_auto;
pub mod settings;
pub mod tick;

pub use cost::CostMeter;
pub use engine::{
    CreateOptimizationInput, Engine, EngineOptions, NOT_CONFIGURED, ResumeReport, is_not_configured,
};
pub use errors::{EngineError, EngineErrorCode, EngineResult, assert_valid_id};
pub use kv::{KvStore, k};
pub use misc::{TeardownCostReconcile, TeardownReport};
pub use ports::{EnginePorts, GitSource};
pub use reaper::reap;
pub use report::default_report;
pub use settings::{
    DEFAULT_TOKEN_PRICING, EngineSettings, ReaperSettings, ReportBuilder, SandboxInputContext,
    SandboxSettings, SecretSource, TokenPricing, default_secret_names,
};

#[cfg(feature = "testing")]
pub mod testing;
