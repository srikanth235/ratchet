//! What the domain needs from the ledger, as traits (the SQLite store lives in `ratchet-ledger`).
//!
//! The sandbox lifecycle and the cost reconciliation are domain logic that reads and writes the ledger. They
//! are written against these small traits so that `ratchet-core` does not depend on the database crate.

use async_trait::async_trait;
use serde_json::Value;

use crate::enums::{SandboxProviderKind, SandboxStatus, SpendCategory};
use crate::ids::{OptimizationId, SandboxId};
use crate::ports::CostReconcileLine;
use crate::rows::{Optimization, Sandbox, SpendBreakdown};

/// Why a ledger operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LedgerErrorCode {
    /// A referenced row does not exist.
    NotFound,
    /// The input is not acceptable (a non-finite number, a foreign agent, a baseline change after a move, ...).
    Invalid,
    /// The optimization has no baseline yet.
    NoBaseline,
    /// The ratchet only turns one way: the value is not strictly better than the current best.
    RatchetNotImproved,
    /// The database refused (a constraint, I/O, ...). The message is the database's.
    Database,
}

impl LedgerErrorCode {
    /// The TypeScript `LedgerError.code` string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Invalid => "invalid",
            Self::NoBaseline => "no_baseline",
            Self::RatchetNotImproved => "ratchet_not_improved",
            Self::Database => "database",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct LedgerError {
    pub code: LedgerErrorCode,
    pub message: String,
}

impl LedgerError {
    pub fn new(code: LedgerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(LedgerErrorCode::NotFound, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(LedgerErrorCode::Invalid, message)
    }

    pub fn database(message: impl Into<String>) -> Self {
        Self::new(LedgerErrorCode::Database, message)
    }
}

pub type LedgerResult<T> = Result<T, LedgerError>;

/// Keep, set or clear an optional column.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Patch<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

/// Input of [`SandboxLedger::record_sandbox`].
#[derive(Debug, Clone, PartialEq)]
pub struct RecordSandboxInput {
    pub id: SandboxId,
    pub provider: SandboxProviderKind,
    pub region: String,
    pub account_id: Option<String>,
    pub status: Option<SandboxStatus>,
    pub created_at: Option<i64>,
}

impl RecordSandboxInput {
    pub fn new(
        id: impl Into<SandboxId>,
        provider: SandboxProviderKind,
        region: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider,
            region: region.into(),
            account_id: None,
            status: None,
            created_at: None,
        }
    }
}

/// Options of [`SandboxLedger::set_sandbox_status`]. `outputs` is stored on apply (`Clear` removes them on
/// destroy); `account_id` is filled in once the account is known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SandboxStatusPatch {
    pub outputs: Patch<Value>,
    pub account_id: Patch<String>,
    pub at: Option<i64>,
}

/// The sandbox rows of the ledger.
#[async_trait]
pub trait SandboxLedger: Send + Sync {
    /// Register a sandbox (idempotent on id: an existing row is returned unchanged).
    async fn record_sandbox(&self, input: RecordSandboxInput) -> LedgerResult<Sandbox>;
    async fn get_sandbox(&self, id: &str) -> LedgerResult<Option<Sandbox>>;
    /// Newest first.
    async fn list_sandboxes(&self) -> LedgerResult<Vec<Sandbox>>;
    /// Move a sandbox through apply / destroy, stamping `applied_at` / `destroyed_at`.
    async fn set_sandbox_status(
        &self,
        id: &str,
        status: SandboxStatus,
        patch: SandboxStatusPatch,
    ) -> LedgerResult<Sandbox>;
    /// Optimizations that ran in a sandbox; teardown must not destroy one that is still live.
    async fn optimizations_in_sandbox(&self, sandbox_id: &str) -> LedgerResult<Vec<Optimization>>;
}

/// The spend rows of the ledger, for cost reconciliation.
#[async_trait]
pub trait SpendLedger: Send + Sync {
    async fn get_optimization(&self, id: &OptimizationId) -> LedgerResult<Option<Optimization>>;
    async fn spend_breakdown(&self, id: &OptimizationId) -> LedgerResult<SpendBreakdown>;
    /// Write the corrections that bring the ledger's infrastructure categories to `actuals`, atomically.
    ///
    /// Inside one write transaction (so two concurrent reconciles cannot both correct the same gap) the
    /// implementation re-reads the breakdown, computes the lines with
    /// [`plan_corrections`](crate::cost::plan_corrections) and appends one `estimated = false` row
    /// (`source`, no agent, amount = the line's delta, possibly negative) per line with `written`.
    async fn apply_spend_corrections(
        &self,
        id: &OptimizationId,
        actuals: &std::collections::BTreeMap<SpendCategory, f64>,
        source: &str,
        epsilon_usd: f64,
        at: i64,
    ) -> LedgerResult<Vec<CostReconcileLine>>;
}
