//! Ratchet's ledger: SQLite through sqlx, the single source of truth for an optimization.
//!
//! `ratchet_moves` and `spend_events` are append-only (triggers in `migrations/0001_init.sql`); money is USD as
//! `REAL`; timestamps are epoch milliseconds. Open one with [`Ledger::open`] (a file, WAL) or
//! [`Ledger::open_memory`]; both apply the migration.
//!
//! [`Ledger`] implements `ratchet_core::ledger::{SandboxLedger, SpendLedger}`, so the sandbox lifecycle and the
//! cost reconciliation of `ratchet-core` run against it.
//!
//! Port of `packages/core/src/ledger`. See `PARITY.md`.

mod rowmap;
mod store;
mod traits;

pub use ratchet_core::ledger::{
    LedgerError, LedgerErrorCode, LedgerResult, Patch, RecordSandboxInput, SandboxStatusPatch,
};
pub use ratchet_core::rows::{gain_percent, is_improvement};
pub use store::{
    AddAgentInput, AppendAgentEventInput, BoxFuture, CreateOptimizationInput, Ledger,
    LedgerOptions, RecordBaselineInput, RecordBenchResultInput, RecordSpendInput,
    SubmitCandidateInput, ValueInput,
};
