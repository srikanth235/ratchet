//! The value model applied to ledger rows (port of `packages/desktop/src/main/value.ts`).

use ratchet_core::config::{Objective, ValueModel};
use ratchet_core::enums::{ValueBasis, ValuePeriod};
use ratchet_core::rows::Optimization;
use ratchet_core::{estimate_value, return_on_spend};

use ratchet_ledger::ValueInput;

use super::format::usd_plain;
use crate::api::{SetValueModelInput, ValueModelVm, ValueVm};

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

/// The Value row of an optimization: its model in numbers and in plain words.
pub fn value_model_vm(row: &Optimization) -> ValueModelVm {
    let unit = row.value_unit.clone().filter(|u| !u.is_empty());
    let note = row.value_note.clone().filter(|n| !n.is_empty());
    let summary = match row.value_usd_per_unit {
        None => "Not set".to_owned(),
        Some(usd) => {
            let per = unit.as_deref().unwrap_or("unit");
            let period = match row.value_period.unwrap_or(ValuePeriod::Year) {
                ValuePeriod::Month => "a month",
                ValuePeriod::Year => "a year",
            };
            let basis = row.value_basis.unwrap_or(ValueBasis::Estimated);
            let mut line = format!("${} per {per} {period} \u{b7} {basis}", usd_plain(usd));
            if let Some(n) = &note {
                line.push_str(" \u{b7} ");
                line.push_str(n);
            }
            line
        }
    };
    ValueModelVm {
        set: row.value_usd_per_unit.is_some(),
        metric: row.metric.clone(),
        direction: row.direction,
        usd_per_unit: row.value_usd_per_unit,
        period: row
            .value_usd_per_unit
            .map(|_| row.value_period.unwrap_or(ValuePeriod::Year)),
        unit: row.value_usd_per_unit.and(unit),
        basis: row
            .value_usd_per_unit
            .map(|_| row.value_basis.unwrap_or(ValueBasis::Estimated)),
        note: row.value_usd_per_unit.and(note),
        summary,
    }
}

/// A `PUT .../value` body as the ledger takes it; the message is for the 400.
pub fn value_input(input: SetValueModelInput) -> Result<ValueInput, (&'static str, String)> {
    if !input.usd_per_unit.is_finite() || input.usd_per_unit <= 0.0 {
        return Err((
            "usdPerUnit",
            "usdPerUnit must be a number greater than 0 (dollars per one unit of improvement)"
                .into(),
        ));
    }
    let text = |v: Option<String>, field: &'static str, max: usize| {
        let t = v.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        match t {
            Some(t) if t.chars().count() > max => {
                Err((field, format!("{field} is at most {max} characters")))
            }
            other => Ok(other),
        }
    };
    Ok(ValueInput {
        usd_per_unit: input.usd_per_unit,
        period: input.period.unwrap_or(ValuePeriod::Year),
        basis: input.basis.unwrap_or(ValueBasis::Estimated),
        unit: text(input.unit, "unit", 24)?,
        note: text(input.note, "note", 200)?,
    })
}

use ratchet_core::ids::OptimizationId;

use super::ledger_views::LedgerViews;
use crate::api::{FieldError, ScheduleVm, SetScheduleInput};
use crate::service::{ServiceError, ServiceResult};

fn invalid_field(path: &str, message: String) -> ServiceError {
    ServiceError::InvalidFields {
        message: message.clone(),
        fields: vec![FieldError::new(path, message)],
    }
}

impl LedgerViews {
    /// The Value row of an optimization.
    pub async fn value_model(&self, id: &str) -> ServiceResult<ValueModelVm> {
        Ok(value_model_vm(&self.require(id).await?))
    }

    /// Replace the value model. The wins already in the ledger are valued with it from now on.
    pub async fn set_value_model(
        &self,
        id: &str,
        input: SetValueModelInput,
    ) -> ServiceResult<ValueModelVm> {
        self.require(id).await?;
        let value = value_input(input).map_err(|(field, message)| invalid_field(field, message))?;
        let row = self
            .ledger
            .set_value_model(&OptimizationId::new(id), Some(&value))
            .await?;
        Ok(value_model_vm(&row))
    }

    /// Remove the value model: the optimization's wins have no dollar value afterwards.
    pub async fn clear_value_model(&self, id: &str) -> ServiceResult<ValueModelVm> {
        self.require(id).await?;
        let row = self
            .ledger
            .set_value_model(&OptimizationId::new(id), None)
            .await?;
        Ok(value_model_vm(&row))
    }

    pub async fn schedule(&self, id: &str) -> ServiceResult<ScheduleVm> {
        Ok(ScheduleVm::of(self.require(id).await?.schedule))
    }

    pub async fn set_schedule(
        &self,
        id: &str,
        input: SetScheduleInput,
    ) -> ServiceResult<ScheduleVm> {
        self.require(id).await?;
        let row = self
            .ledger
            .set_schedule(&OptimizationId::new(id), input.schedule)
            .await?;
        Ok(ScheduleVm::of(row.schedule))
    }
}
