//! The value model applied to ledger rows (port of `packages/desktop/src/main/value.ts`).

use ratchet_core::config::{Objective, ValueModel};
use ratchet_core::enums::{ValueBasis, ValuePeriod};
use ratchet_core::rows::Optimization;
use ratchet_core::{estimate_value, return_on_spend};

use crate::api::ValueVm;

/// Improvement of `value` over `baseline` in percent, positive = better in the objective's direction.
pub fn gain_vs_baseline(direction: ratchet_core::Direction, baseline: f64, value: f64) -> f64 {
    if baseline == 0.0 {
        return 0.0;
    }
    let delta = match direction {
        ratchet_core::Direction::Minimize => baseline - value,
        ratchet_core::Direction::Maximize => value - baseline,
    };
    (delta / baseline.abs()) * 100.0
}

/// The optimization's `objective.value` (ledger columns) as the config shape core's `estimate_value` reads.
fn objective_of(row: &Optimization) -> Option<Objective> {
    let usd = row.value_usd_per_unit?;
    Some(Objective {
        name: None,
        metric: row.metric.clone(),
        direction: row.direction,
        min_gain: Some(row.min_gain),
        value: Some(ValueModel {
            usd_per_unit: usd,
            period: row.value_period.unwrap_or(ValuePeriod::Year),
            basis: row.value_basis.unwrap_or(ValueBasis::Estimated),
            unit: row.value_unit.clone(),
            note: None,
        }),
    })
}

/// Yearly dollar value of moving the objective from `from` to `to`; `None` without a value model.
pub fn value_of_move(row: &Optimization, from: f64, to: f64) -> Option<f64> {
    let obj = objective_of(row)?;
    estimate_value(&obj, from, to).map(|e| e.usd_per_year)
}

/// Value of the improvement baseline -> best and its return on `spend_usd`; `None` without a model or an improvement.
pub fn value_of(
    row: &Optimization,
    baseline: Option<f64>,
    best: Option<f64>,
    spend_usd: f64,
) -> Option<ValueVm> {
    let obj = objective_of(row)?;
    let est = estimate_value(&obj, baseline?, best?)?;
    if est.improvement <= 0.0 {
        return None; // nothing gained, nothing to value
    }
    Some(ValueVm {
        usd_per_unit: obj.value.as_ref()?.usd_per_unit,
        unit: row.value_unit.clone(),
        period: est.period,
        basis: est.basis,
        usd_per_year: est.usd_per_year,
        return_on_spend: return_on_spend(est.usd_per_year, spend_usd),
    })
}
