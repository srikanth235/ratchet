//! The ledger as the traits `ratchet-core` programs against.

use std::collections::BTreeMap;

use async_trait::async_trait;
use ratchet_core::enums::{SandboxStatus, SpendCategory};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ledger::{
    LedgerResult, RecordSandboxInput, SandboxLedger, SandboxStatusPatch, SpendLedger,
};
use ratchet_core::ports::CostReconcileLine;
use ratchet_core::rows::{Optimization, Sandbox, SpendBreakdown};

use crate::store::Ledger;

#[async_trait]
impl SandboxLedger for Ledger {
    async fn record_sandbox(&self, input: RecordSandboxInput) -> LedgerResult<Sandbox> {
        Ledger::record_sandbox(self, input).await
    }

    async fn get_sandbox(&self, id: &str) -> LedgerResult<Option<Sandbox>> {
        Ledger::get_sandbox(self, id).await
    }

    async fn list_sandboxes(&self) -> LedgerResult<Vec<Sandbox>> {
        Ledger::list_sandboxes(self).await
    }

    async fn set_sandbox_status(
        &self,
        id: &str,
        status: SandboxStatus,
        patch: SandboxStatusPatch,
    ) -> LedgerResult<Sandbox> {
        Ledger::set_sandbox_status(self, id, status, patch).await
    }

    async fn optimizations_in_sandbox(&self, sandbox_id: &str) -> LedgerResult<Vec<Optimization>> {
        Ledger::optimizations_in_sandbox(self, sandbox_id).await
    }
}

#[async_trait]
impl SpendLedger for Ledger {
    async fn get_optimization(&self, id: &OptimizationId) -> LedgerResult<Option<Optimization>> {
        Ledger::get_optimization(self, id).await
    }

    async fn spend_breakdown(&self, id: &OptimizationId) -> LedgerResult<SpendBreakdown> {
        Ledger::spend_breakdown(self, id).await
    }

    async fn apply_spend_corrections(
        &self,
        id: &OptimizationId,
        actuals: &BTreeMap<SpendCategory, f64>,
        source: &str,
        epsilon_usd: f64,
        at: i64,
    ) -> LedgerResult<Vec<CostReconcileLine>> {
        Ledger::apply_spend_corrections(self, id, actuals, source, epsilon_usd, at).await
    }
}
