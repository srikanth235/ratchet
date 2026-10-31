//! Cost reconcile: compare the ledger's estimated infrastructure spend with billing data and append correction
//! rows (docs/V0_ARCHITECTURE.md section 7). `spend_events` is append-only, so a correction is a new row
//! `estimated = false, source = 'cost-explorer'` whose amount may be negative.
//!
//! Idempotence: the correction for a category is `billed - (everything the ledger already holds for that
//! category, estimates and earlier corrections)`, so a second run with the same billing data computes 0 and
//! writes nothing. When billing catches up later, only the remainder is written.
//!
//! Port of `packages/core/src/cost/reconcile.ts`; the transaction is the ledger's
//! ([`SpendLedger::apply_spend_corrections`]).

use std::collections::BTreeMap;

use crate::enums::{OptimizationStatus, SpendCategory};
use crate::ids::OptimizationId;
use crate::ledger::{LedgerError, SpendLedger};
use crate::ports::{
    CostReconcileLine, CostReconcileReport, CostReconcileStatus, CostReconciler, PortError,
    TimeWindow,
};
use crate::rows::CategoryTotals;

pub const COST_RECONCILE_SOURCE: &str = "cost-explorer";

/// Cost Explorer data can lag up to ~24 h behind usage.
pub const COST_EXPLORER_LAG_MS: i64 = 24 * 3_600_000;

/// Categories that come from cloud billing. `tokens` are model API spend, which AWS does not bill.
pub const RECONCILED_CATEGORIES: [SpendCategory; 4] = [
    SpendCategory::AgentRunner,
    SpendCategory::Experiment,
    SpendCategory::BenchRunner,
    SpendCategory::StorageEgress,
];

/// Options of [`reconcile_spend`].
#[derive(Debug, Clone, Copy)]
pub struct ReconcileSpendOptions {
    /// "Now" (epoch ms), from the engine's clock.
    pub now: i64,
    /// How long after the optimization ended billing is assumed incomplete. Default 24 h.
    pub lag_ms: Option<i64>,
    /// Ignore corrections smaller than this (USD). Default 1e-6.
    pub epsilon_usd: Option<f64>,
}

impl ReconcileSpendOptions {
    pub fn at(now: i64) -> Self {
        Self {
            now,
            lag_ms: None,
            epsilon_usd: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconcileError {
    #[error("optimization {0} not found")]
    NotFound(OptimizationId),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    /// The billing system failed.
    #[error(transparent)]
    Billing(#[from] PortError),
}

/// The correction lines for one reconcile run: for each billed category, `delta = actual - ledger`, written when
/// it is larger than `epsilon_usd` in magnitude. Pure.
pub fn plan_corrections(
    current: &CategoryTotals,
    actuals: &BTreeMap<SpendCategory, f64>,
    epsilon_usd: f64,
) -> Vec<CostReconcileLine> {
    RECONCILED_CATEGORIES
        .iter()
        .map(|&category| {
            let actual_usd = actuals.get(&category).copied().unwrap_or(0.0);
            let ledger_usd = current.get(category);
            let delta_usd = actual_usd - ledger_usd;
            CostReconcileLine {
                category,
                ledger_usd,
                actual_usd,
                delta_usd,
                written: delta_usd.abs() > epsilon_usd,
            }
        })
        .collect()
}

/// Reconcile one optimization. Writes nothing (status `pending`) while the optimization is still running, within
/// the billing lag of its end, when the billing system flags its data as estimated, or when it returned no data
/// at all while the ledger has estimates (typically: cost-allocation tags not activated yet). Rejecting those
/// cases is what keeps a lagging report from "correcting" real estimates down to zero.
pub async fn reconcile_spend(
    ledger: &dyn SpendLedger,
    reconciler: &dyn CostReconciler,
    optimization_id: &OptimizationId,
    opts: ReconcileSpendOptions,
) -> Result<CostReconcileReport, ReconcileError> {
    let opt = ledger
        .get_optimization(optimization_id)
        .await?
        .ok_or_else(|| ReconcileError::NotFound(optimization_id.clone()))?;
    let lag_ms = opts.lag_ms.unwrap_or(COST_EXPLORER_LAG_MS);
    let eps = opts.epsilon_usd.unwrap_or(1e-6);
    // The window ends when the optimization did (all instances are terminated before that), so the still-open
    // current billing day is never part of the query.
    let start_ms = opt.started_at.unwrap_or(opt.created_at);
    let window = TimeWindow {
        start_ms,
        end_ms: start_ms.max(opt.ended_at.unwrap_or(opts.now)),
    };

    let early_pending = |reason: String| CostReconcileReport {
        optimization_id: optimization_id.clone(),
        at: opts.now,
        status: CostReconcileStatus::Pending,
        reason: Some(reason),
        window,
        lines: Vec::new(),
        written_usd: 0.0,
    };

    // Cheap gates first: no billing query (each Cost Explorer request costs money) while it cannot be final.
    let ended_at = match (opt.status, opt.ended_at) {
        (
            OptimizationStatus::Completed
            | OptimizationStatus::Stopped
            | OptimizationStatus::Failed,
            Some(t),
        ) => t,
        _ => {
            return Ok(early_pending(format!(
                "optimization is {}; reconcile after it has ended",
                opt.status
            )));
        }
    };
    let ready_at = ended_at + lag_ms;
    if opts.now < ready_at {
        let hours = ((ready_at - opts.now) as f64 / 3_600_000.0).ceil() as i64;
        let lag_hours = (lag_ms as f64 / 3_600_000.0 + 0.5).floor() as i64;
        return Ok(early_pending(format!(
            "billing data lags up to {lag_hours} h behind usage; try again in about {hours} h"
        )));
    }

    let actuals = reconciler
        .actuals(&crate::ports::CostActualsQuery {
            optimization_id: optimization_id.clone(),
            start_ms: window.start_ms,
            end_ms: window.end_ms,
        })
        .await?;
    let current = ledger.spend_breakdown(optimization_id).await?.by_category;
    let pending = |reason: String| {
        let mut report = early_pending(reason);
        report.lines = RECONCILED_CATEGORIES
            .iter()
            .map(|&category| {
                let actual_usd = actuals.by_category.get(&category).copied().unwrap_or(0.0);
                let ledger_usd = current.get(category);
                CostReconcileLine {
                    category,
                    ledger_usd,
                    actual_usd,
                    delta_usd: actual_usd - ledger_usd,
                    written: false,
                }
            })
            .collect();
        report
    };

    if !actuals.complete {
        return Ok(pending(actuals.note.clone().unwrap_or_else(|| {
            "billing data for this period is still marked as estimated".into()
        })));
    }
    let estimated_total: f64 = RECONCILED_CATEGORIES.iter().map(|c| current.get(*c)).sum();
    if actuals.groups == 0 && estimated_total.abs() > eps {
        return Ok(pending(actuals.note.clone().unwrap_or_else(|| {
            "billing returned no cost data for this optimization (cost-allocation tags not activated yet, or data not available yet)".into()
        })));
    }

    // The ledger re-reads inside its write transaction so two concurrent reconciles cannot both correct the same gap.
    let lines = ledger
        .apply_spend_corrections(
            optimization_id,
            &actuals.by_category,
            COST_RECONCILE_SOURCE,
            eps,
            opts.now,
        )
        .await?;
    let written_usd: f64 = lines
        .iter()
        .filter(|l| l.written)
        .map(|l| l.delta_usd)
        .sum();
    Ok(CostReconcileReport {
        optimization_id: optimization_id.clone(),
        at: opts.now,
        status: if lines.iter().any(|l| l.written) {
            CostReconcileStatus::Reconciled
        } else {
            CostReconcileStatus::Unchanged
        },
        reason: None,
        window,
        lines,
        written_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(a: f64, e: f64, b: f64, s: f64) -> CategoryTotals {
        CategoryTotals {
            agent_runner: a,
            experiment: e,
            bench_runner: b,
            storage_egress: s,
            tokens: 5.0,
        }
    }

    #[test]
    fn plans_positive_and_negative_corrections_and_ignores_tokens() {
        let actuals: BTreeMap<_, _> = [
            (SpendCategory::AgentRunner, 1.5),
            (SpendCategory::Experiment, 1.25),
            (SpendCategory::BenchRunner, 0.3),
            (SpendCategory::Tokens, 99.0),
        ]
        .into();
        let lines = plan_corrections(&totals(1.0, 2.0, 0.0, 0.0), &actuals, 1e-6);
        let summary: Vec<(SpendCategory, f64, bool)> = lines
            .iter()
            .map(|l| (l.category, (l.delta_usd * 1e9).round() / 1e9, l.written))
            .collect();
        assert_eq!(
            summary,
            [
                (SpendCategory::AgentRunner, 0.5, true),
                (SpendCategory::Experiment, -0.75, true),
                (SpendCategory::BenchRunner, 0.3, true),
                (SpendCategory::StorageEgress, 0.0, false),
            ]
        );
    }

    #[test]
    fn a_second_plan_against_the_corrected_ledger_writes_nothing() {
        let actuals: BTreeMap<_, _> = [
            (SpendCategory::AgentRunner, 1.5),
            (SpendCategory::Experiment, 2.0),
        ]
        .into();
        let corrected = totals(1.5, 2.0, 0.0, 0.0);
        assert!(
            plan_corrections(&corrected, &actuals, 1e-6)
                .iter()
                .all(|l| !l.written)
        );
    }
}
