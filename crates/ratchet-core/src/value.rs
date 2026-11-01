//! The value model: what a metric improvement is worth in dollars.
//!
//! Port of `packages/core/src/value.ts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::Objective;
use crate::enums::{Direction, ValueBasis, ValuePeriod};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ValueEstimate {
    /// Metric improvement in the objective's direction (positive = better), in metric units.
    pub improvement: f64,
    /// USD per `period` implied by the improvement.
    pub usd_per_period: f64,
    pub period: ValuePeriod,
    /// Normalized to a year, for ROI.
    pub usd_per_year: f64,
    pub basis: ValueBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Dollar value of moving the objective from `baseline` to `best`, or `None` when the objective has no
/// `value` (or a value is not finite).
pub fn estimate_value(objective: &Objective, baseline: f64, best: f64) -> Option<ValueEstimate> {
    let v = objective.value.as_ref()?;
    if !baseline.is_finite() || !best.is_finite() {
        return None;
    }
    let improvement = match objective.direction {
        Direction::Minimize => baseline - best,
        Direction::Maximize => best - baseline,
    };
    let usd_per_period = improvement.max(0.0) * v.usd_per_unit;
    Some(ValueEstimate {
        improvement,
        usd_per_period,
        period: v.period,
        usd_per_year: if v.period == ValuePeriod::Month {
            usd_per_period * 12.0
        } else {
            usd_per_period
        },
        basis: v.basis,
        unit: v.unit.clone().filter(|u| !u.is_empty()),
    })
}

/// Return on spend: yearly value created per dollar spent (`None` when nothing was spent).
pub fn return_on_spend(usd_per_year: f64, spend_usd: f64) -> Option<f64> {
    (spend_usd > 0.0).then(|| usd_per_year / spend_usd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ValueModel;

    fn cfg(direction: Direction, value: Option<ValueModel>) -> Objective {
        Objective {
            name: None,
            metric: "m".into(),
            direction,
            min_gain: Some(0.03),
            value,
        }
    }

    fn model(usd: f64, period: ValuePeriod, basis: ValueBasis, unit: Option<&str>) -> ValueModel {
        ValueModel {
            usd_per_unit: usd,
            period,
            basis,
            unit: unit.map(str::to_owned),
            note: None,
        }
    }

    // value.test.ts: "returns null without objective.value"
    #[test]
    fn returns_none_without_objective_value() {
        assert_eq!(
            estimate_value(&cfg(Direction::Minimize, None), 182.0, 124.0),
            None
        );
    }

    // "values a minimize improvement per year"
    #[test]
    fn values_a_minimize_improvement_per_year() {
        let o = cfg(
            Direction::Minimize,
            Some(model(
                1000.0,
                ValuePeriod::Year,
                ValueBasis::Estimated,
                Some("ms"),
            )),
        );
        let v = estimate_value(&o, 182.0, 124.0).unwrap();
        assert_eq!(v.improvement, 58.0);
        assert_eq!(v.usd_per_year, 58_000.0);
        assert_eq!(v.basis, ValueBasis::Estimated);
        assert_eq!(v.unit.as_deref(), Some("ms"));
    }

    // "annualizes monthly values and handles maximize"
    #[test]
    fn annualizes_monthly_values_and_handles_maximize() {
        let o = cfg(
            Direction::Maximize,
            Some(model(10.0, ValuePeriod::Month, ValueBasis::Measured, None)),
        );
        let v = estimate_value(&o, 0.91, 0.93).unwrap();
        assert!((v.usd_per_period - 0.2).abs() < 1e-9);
        assert!((v.usd_per_year - 2.4).abs() < 1e-9);
    }

    // "never reports negative value for a regression"
    #[test]
    fn never_reports_negative_value_for_a_regression() {
        let o = cfg(
            Direction::Minimize,
            Some(model(5.0, ValuePeriod::Year, ValueBasis::Estimated, None)),
        );
        assert_eq!(estimate_value(&o, 100.0, 120.0).unwrap().usd_per_year, 0.0);
    }

    // returnOnSpend "divides yearly value by spend"
    #[test]
    fn return_on_spend_divides_yearly_value_by_spend() {
        assert!((return_on_spend(58_000.0, 268.0).unwrap() - 216.4).abs() < 0.05);
        assert_eq!(return_on_spend(1.0, 0.0), None);
    }

    #[test]
    fn non_finite_values_give_none() {
        let o = cfg(
            Direction::Minimize,
            Some(model(5.0, ValuePeriod::Year, ValueBasis::Estimated, None)),
        );
        assert_eq!(estimate_value(&o, f64::NAN, 1.0), None);
    }
}
