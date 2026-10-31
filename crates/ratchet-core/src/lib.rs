//! Ratchet's domain crate: everything the engine, the ledger and the API share.
//!
//! A ratchet only turns one way. Ratchet lets coding agents try to improve a business metric and keeps a change
//! only when a trusted evaluator confirms it is better, on data the agents never saw. This crate holds the
//! pieces of that rule that do not touch the outside world:
//!
//! - [`config`]: `ratchet.yaml` parsing and validation, guardrails;
//! - [`bench`](mod@bench): statistics, auto-calibration (`n = ceil(18 CV^2 / g^2)` clamped to 3..15, `g = max(2 CV, 1%)`),
//!   benchmark-output parsing;
//! - [`ratchet`]: the ratchet rule (`verified | rejected | failed_confirm | blocked`, plus `pending_confirm`);
//! - [`protected`] / [`protected_set`]: protected-path matching and the automatic protected set;
//! - [`value`]: the value model; [`cost`]: cost reconciliation; [`sandbox`]: the sandbox lifecycle;
//! - [`ports`]: every external system as a trait, with in-process mocks in [`ports::mock`];
//! - [`templates`]: the evaluator template gallery, rendering, and the "Use template" proposal;
//! - [`ids`], [`enums`], [`rows`]: the shared data types (serde camelCase, `schemars::JsonSchema`, and
//!   `utoipa::ToSchema` behind the `utoipa` feature).
//!
//! This is the Rust port of `packages/core`; the TypeScript stays as the executable spec. See `PARITY.md`.

pub mod bench;
pub mod config;
pub mod cost;
pub mod enums;
pub mod ids;
pub mod ledger;
pub mod numfmt;
pub mod ports;
pub mod protected;
pub mod protected_set;
pub mod ratchet;
pub mod rows;
pub mod sandbox;
pub mod templates;
pub mod value;

pub use bench::{
    Calibration, CalibrationInput, CalibrationSource, NoiseFloorOptions, StatsError, Summary,
    calibrate_baseline, coefficient_of_variation, explain_calibration, extract_last_json_object,
    extract_metric, format_percent, lookup_path, mean, median, nice_gain, noise_floor, stddev,
    summarize,
};
pub use config::{
    ConfigError, ConfigResult, Guardrail, GuardrailResult, RatchetConfig, load_config,
    parse_config, parse_threshold,
};
pub use cost::{ReconcileSpendOptions, plan_corrections, reconcile_spend};
pub use enums::*;
pub use ids::{AgentId, CandidateId, OptimizationId, SandboxId};
pub use ledger::{LedgerError, LedgerErrorCode, LedgerResult, SandboxLedger, SpendLedger};
pub use protected::{
    ProtectedViolation, find_protected_violations, matches_glob, normalize_repo_path,
};
pub use protected_set::{EffectiveProtected, ProtectedConfigInput, effective_protected};
pub use ratchet::{RatchetInput, RatchetOutcome, evaluate_ratchet};
pub use rows::*;
pub use sandbox::{SandboxLifecycle, SandboxLifecycleError};
pub use templates::{
    TemplateInfo, build_template_proposal, get_template, list_templates, render_template,
    resolve_template_id,
};
pub use value::{ValueEstimate, estimate_value, return_on_spend};

/// Crate version (the TypeScript `VERSION`).
pub const VERSION: &str = "0.0.0";
