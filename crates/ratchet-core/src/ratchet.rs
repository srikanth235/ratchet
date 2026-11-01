//! The Ratchet rule (docs/V0_ARCHITECTURE.md section 3), as a pure function.
//!
//! Port of `packages/core/src/ratchet/rule.ts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::bench::stats::{StatsError, median};
use crate::config::GuardrailResult;
use crate::enums::{Direction, Verdict};
use crate::numfmt::precision_trimmed;
use crate::protected::ProtectedViolation;

/// Input of [`evaluate_ratchet`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RatchetInput {
    pub direction: Direction,
    /// Fraction of the current best, e.g. 0.03 for 3%.
    pub min_gain: f64,
    /// Absolute noise floor in metric units (see `noise_floor` in `bench::stats`).
    pub noise_floor: f64,
    /// Current best verified value on the bench (e.g. median of the best's bench runs).
    pub current_best: f64,
    /// The candidate's N bench samples.
    pub candidate: Vec<f64>,
    /// Samples from the confirm run on a fresh instance against the hidden trace. Absent or empty: not yet run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Vec<f64>>,
    /// The current best's value on the hidden trace. Defaults to `current_best`, which is only right when the
    /// bench and the hidden trace share a scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_best: Option<f64>,
    pub guardrails: Vec<GuardrailResult>,
    pub protected_violations: Vec<ProtectedViolation>,
}

/// Output of [`evaluate_ratchet`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RatchetOutcome {
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    /// Candidate's central value (median of its bench samples).
    pub candidate_value: f64,
    /// Signed improvement over `current_best` in the objective's direction (positive = better).
    pub improvement: f64,
    /// `max(noise_floor, min_gain * |current_best|)`. Improvement must be strictly greater.
    pub required_gain: f64,
    /// Signed improvement of the confirm run over `confirm_best`, when a confirm run exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_improvement: Option<f64>,
}

/// A caller bug: the rule was given input it cannot evaluate (the TypeScript threw `RangeError`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RatchetError {
    #[error("{name} must be a finite number, got {value}")]
    NotFinite { name: &'static str, value: f64 },
    #[error("{0}")]
    InvalidParameter(&'static str),
    #[error("candidate must have at least one sample")]
    NoCandidateSamples,
    #[error(transparent)]
    Samples(#[from] StatsError),
}

/// Improvement in the direction of the objective: positive means better.
pub fn improvement_of(direction: Direction, best: f64, value: f64) -> f64 {
    match direction {
        Direction::Minimize => best - value,
        Direction::Maximize => value - best,
    }
}

fn check_finite(name: &'static str, v: f64) -> Result<(), RatchetError> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(RatchetError::NotFinite { name, value: v })
    }
}

/// `String(Number(n.toPrecision(6)))`
fn fmt(n: f64) -> String {
    precision_trimmed(n, 6)
}

/// The Ratchet rule. Gates, in precedence order:
///
///  1. `blocked`: any failed guardrail or any protected-path violation. This wins over everything, including a
///     spectacular bench result. All blocking reasons are listed.
///  2. `rejected`: improvement of the candidate's median over `current_best` is not STRICTLY greater than
///     `required_gain = max(noise_floor, min_gain * |current_best|)`. Ties at the threshold are rejected.
///  3. `pending_confirm`: bench win, but no confirm samples yet.
///  4. `failed_confirm`: confirm median does not strictly beat `confirm_best` (default: `current_best`).
///  5. `verified`.
///
/// Central value of a sample set is its median (robust to a single outlier run). Fails on empty candidate
/// samples or non-finite inputs; those are caller bugs.
pub fn evaluate_ratchet(input: &RatchetInput) -> Result<RatchetOutcome, RatchetError> {
    let (direction, min_gain, noise_floor, current_best) = (
        input.direction,
        input.min_gain,
        input.noise_floor,
        input.current_best,
    );
    check_finite("currentBest", current_best)?;
    if !min_gain.is_finite() || min_gain < 0.0 {
        return Err(RatchetError::InvalidParameter(
            "minGain must be a finite number >= 0",
        ));
    }
    if !noise_floor.is_finite() || noise_floor < 0.0 {
        return Err(RatchetError::InvalidParameter(
            "noiseFloor must be a finite number >= 0",
        ));
    }
    if input.candidate.is_empty() {
        return Err(RatchetError::NoCandidateSamples);
    }

    let candidate_value = median(&input.candidate)?;
    let improvement = improvement_of(direction, current_best, candidate_value);
    let required_gain = noise_floor.max(min_gain * current_best.abs());
    let base =
        |verdict: Verdict, reasons: Vec<String>, confirm_improvement: Option<f64>| RatchetOutcome {
            verdict,
            reasons,
            candidate_value,
            improvement,
            required_gain,
            confirm_improvement,
        };

    let mut blockers: Vec<String> = Vec::new();
    for g in &input.guardrails {
        if !g.passed {
            blockers.push(format!(
                "guardrail failed: {}{}",
                g.name,
                g.detail
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ));
        }
    }
    for v in &input.protected_violations {
        blockers.push(format!(
            "protected path changed: {} (matches {})",
            v.path, v.pattern
        ));
    }
    if !blockers.is_empty() {
        return Ok(base(Verdict::Blocked, blockers, None));
    }

    let beats_required_gain = improvement > required_gain;
    if !beats_required_gain {
        let why = if improvement <= 0.0 {
            format!(
                "candidate {} is not better than current best {} ({direction})",
                fmt(candidate_value),
                fmt(current_best)
            )
        } else {
            format!(
                "improvement {} does not exceed required gain {} (max of noise floor {} and min_gain {})",
                fmt(improvement),
                fmt(required_gain),
                fmt(noise_floor),
                fmt(min_gain * current_best.abs())
            )
        };
        return Ok(base(Verdict::Rejected, vec![why], None));
    }

    let win = format!(
        "bench improvement {} exceeds required gain {}",
        fmt(improvement),
        fmt(required_gain)
    );
    let confirm = input.confirm.as_deref().filter(|c| !c.is_empty());
    let Some(confirm) = confirm else {
        return Ok(base(
            Verdict::PendingConfirm,
            vec![
                win,
                "confirm run on the hidden trace has not completed".to_owned(),
            ],
            None,
        ));
    };

    let confirm_best = input.confirm_best.unwrap_or(current_best);
    check_finite("confirmBest", confirm_best)?;
    let confirm_value = median(confirm)?;
    let confirm_improvement = improvement_of(direction, confirm_best, confirm_value);
    let confirmed = confirm_improvement > 0.0;
    if !confirmed {
        return Ok(base(
            Verdict::FailedConfirm,
            vec![
                win,
                format!(
                    "confirm run {} does not beat current best {} on the hidden trace ({direction})",
                    fmt(confirm_value),
                    fmt(confirm_best)
                ),
            ],
            Some(confirm_improvement),
        ));
    }
    Ok(base(
        Verdict::Verified,
        vec![
            win,
            format!(
                "confirm run {} beats current best {} on the hidden trace",
                fmt(confirm_value),
                fmt(confirm_best)
            ),
        ],
        Some(confirm_improvement),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> GuardrailResult {
        GuardrailResult {
            name: "pytest -q".into(),
            passed: true,
            detail: None,
        }
    }

    fn bad() -> GuardrailResult {
        GuardrailResult {
            name: "pytest -q".into(),
            passed: false,
            detail: Some("exited with status 1".into()),
        }
    }

    /// Minimize, best 100, min_gain 3% (=3), noise floor 1  => required gain 3.
    fn base() -> RatchetInput {
        RatchetInput {
            direction: Direction::Minimize,
            min_gain: 0.03,
            noise_floor: 1.0,
            current_best: 100.0,
            candidate: vec![90.0, 90.0, 90.0],
            confirm: None,
            confirm_best: None,
            guardrails: vec![ok()],
            protected_violations: vec![],
        }
    }

    fn run(f: impl FnOnce(&mut RatchetInput)) -> RatchetOutcome {
        let mut i = base();
        f(&mut i);
        evaluate_ratchet(&i).unwrap()
    }

    fn run_err(f: impl FnOnce(&mut RatchetInput)) -> RatchetError {
        let mut i = base();
        f(&mut i);
        evaluate_ratchet(&i).unwrap_err()
    }

    // rule.test.ts improvementOf "is positive when better in the objective direction"
    #[test]
    fn improvement_of_is_positive_when_better() {
        assert_eq!(improvement_of(Direction::Minimize, 100.0, 90.0), 10.0);
        assert_eq!(improvement_of(Direction::Minimize, 100.0, 110.0), -10.0);
        assert_eq!(improvement_of(Direction::Maximize, 100.0, 110.0), 10.0);
        assert_eq!(improvement_of(Direction::Maximize, 100.0, 90.0), -10.0);
    }

    // "pending_confirm when the bench wins and there is no confirm run"
    #[test]
    fn minimize_pending_confirm_without_a_confirm_run() {
        let r = run(|_| {});
        assert_eq!(r.verdict, Verdict::PendingConfirm);
        assert_eq!(r.improvement, 10.0);
        assert_eq!(r.required_gain, 3.0);
        assert_eq!(r.candidate_value, 90.0);
        assert!(r.reasons.join(" ").contains("not completed"));
    }

    // "treats an empty confirm array as missing"
    #[test]
    fn minimize_treats_an_empty_confirm_array_as_missing() {
        assert_eq!(
            run(|i| i.confirm = Some(vec![])).verdict,
            Verdict::PendingConfirm
        );
    }

    // "verified when the confirm run also beats the best"
    #[test]
    fn minimize_verified_when_the_confirm_run_also_beats_the_best() {
        let r = run(|i| i.confirm = Some(vec![92.0, 91.0, 93.0]));
        assert_eq!(r.verdict, Verdict::Verified);
        assert_eq!(r.confirm_improvement, Some(8.0));
    }

    // "failed_confirm when the confirm run does not beat the best"
    #[test]
    fn minimize_failed_confirm_when_the_confirm_run_does_not_beat_the_best() {
        let r = run(|i| i.confirm = Some(vec![105.0, 104.0, 106.0]));
        assert_eq!(r.verdict, Verdict::FailedConfirm);
        assert_eq!(r.confirm_improvement, Some(-5.0));
        assert!(r.reasons.join(" ").contains("hidden trace"));
    }

    // "failed_confirm on a tie with the best on the hidden trace"
    #[test]
    fn minimize_failed_confirm_on_a_tie_with_the_best() {
        assert_eq!(
            run(|i| i.confirm = Some(vec![100.0])).verdict,
            Verdict::FailedConfirm
        );
    }

    // "confirmBest overrides currentBest for the confirm comparison"
    #[test]
    fn minimize_confirm_best_overrides_current_best() {
        // hidden trace is on a different scale: best scored 500 there
        assert_eq!(
            run(|i| {
                i.confirm = Some(vec![480.0]);
                i.confirm_best = Some(500.0);
            })
            .verdict,
            Verdict::Verified
        );
        assert_eq!(
            run(|i| i.confirm = Some(vec![480.0])).verdict,
            Verdict::FailedConfirm
        ); // would wrongly compare to 100
        assert_eq!(
            run(|i| {
                i.confirm = Some(vec![510.0]);
                i.confirm_best = Some(500.0);
            })
            .verdict,
            Verdict::FailedConfirm
        );
    }

    // "rejected when the candidate is worse or equal"
    #[test]
    fn minimize_rejected_when_worse_or_equal() {
        let worse = run(|i| i.candidate = vec![110.0]);
        assert_eq!(worse.verdict, Verdict::Rejected);
        assert_eq!(worse.improvement, -10.0);
        assert!(worse.reasons[0].contains("not better"));
        assert_eq!(
            run(|i| i.candidate = vec![100.0]).verdict,
            Verdict::Rejected
        );
    }

    // "rejected when better but within the required gain; ties at the threshold are rejected"
    #[test]
    fn minimize_rejected_within_the_required_gain_and_on_ties() {
        assert_eq!(run(|i| i.candidate = vec![98.0]).verdict, Verdict::Rejected); // 2 < 3
        assert_eq!(run(|i| i.candidate = vec![97.0]).verdict, Verdict::Rejected); // exactly 3 == required
        assert_eq!(
            run(|i| i.candidate = vec![96.9]).verdict,
            Verdict::PendingConfirm
        ); // 3.1 > 3
        assert!(
            run(|i| i.candidate = vec![98.0]).reasons[0].contains("does not exceed required gain")
        );
    }

    // "uses the median of candidate samples, so one outlier run does not decide"
    #[test]
    fn minimize_uses_the_median_of_candidate_samples() {
        assert_eq!(
            run(|i| i.candidate = vec![90.0, 90.0, 200.0]).verdict,
            Verdict::PendingConfirm
        ); // median 90
        assert_eq!(
            run(|i| i.candidate = vec![90.0, 200.0, 200.0]).verdict,
            Verdict::Rejected
        ); // median 200
    }

    fn max(i: &mut RatchetInput) {
        i.direction = Direction::Maximize;
    }

    // maximize: "higher is better"
    #[test]
    fn maximize_higher_is_better() {
        let r = run(|i| {
            max(i);
            i.candidate = vec![110.0];
        });
        assert_eq!(r.verdict, Verdict::PendingConfirm);
        assert_eq!(r.improvement, 10.0);
    }

    // "lower candidate is rejected"
    #[test]
    fn maximize_lower_candidate_is_rejected() {
        assert_eq!(
            run(|i| {
                max(i);
                i.candidate = vec![90.0];
            })
            .verdict,
            Verdict::Rejected
        );
    }

    // "ties at the threshold"
    #[test]
    fn maximize_ties_at_the_threshold() {
        assert_eq!(
            run(|i| {
                max(i);
                i.candidate = vec![103.0];
            })
            .verdict,
            Verdict::Rejected
        );
        assert_eq!(
            run(|i| {
                max(i);
                i.candidate = vec![103.1];
            })
            .verdict,
            Verdict::PendingConfirm
        );
    }

    // "confirm must be higher than best"
    #[test]
    fn maximize_confirm_must_be_higher_than_best() {
        assert_eq!(
            run(|i| {
                max(i);
                i.candidate = vec![110.0];
                i.confirm = Some(vec![111.0]);
            })
            .verdict,
            Verdict::Verified
        );
        assert_eq!(
            run(|i| {
                max(i);
                i.candidate = vec![110.0];
                i.confirm = Some(vec![99.0]);
            })
            .verdict,
            Verdict::FailedConfirm
        );
    }

    // "noise larger than min_gain dominates"
    #[test]
    fn noise_larger_than_min_gain_dominates() {
        // min_gain*best = 3, noise = 8 => required 8
        assert_eq!(
            run(|i| {
                i.noise_floor = 8.0;
                i.candidate = vec![93.0];
            })
            .verdict,
            Verdict::Rejected
        ); // 7 < 8
        assert_eq!(
            run(|i| {
                i.noise_floor = 8.0;
                i.candidate = vec![92.0];
            })
            .verdict,
            Verdict::Rejected
        ); // tie at 8
        assert_eq!(
            run(|i| {
                i.noise_floor = 8.0;
                i.candidate = vec![91.0];
            })
            .verdict,
            Verdict::PendingConfirm
        );
        assert_eq!(run(|i| i.noise_floor = 8.0).required_gain, 8.0);
    }

    // "min_gain dominates when noise is small"
    #[test]
    fn min_gain_dominates_when_noise_is_small() {
        assert_eq!(run(|i| i.noise_floor = 0.1).required_gain, 3.0);
    }

    // "zero noise and zero min_gain: any strict improvement counts"
    #[test]
    fn zero_noise_and_zero_min_gain_any_strict_improvement_counts() {
        assert_eq!(
            run(|i| {
                i.noise_floor = 0.0;
                i.min_gain = 0.0;
                i.candidate = vec![99.9];
            })
            .verdict,
            Verdict::PendingConfirm
        );
        assert_eq!(
            run(|i| {
                i.noise_floor = 0.0;
                i.min_gain = 0.0;
                i.candidate = vec![100.0];
            })
            .verdict,
            Verdict::Rejected
        );
    }

    // "uses |best| for the relative gain when best is negative"
    #[test]
    fn uses_abs_best_for_the_relative_gain() {
        let r = run(|i| {
            i.current_best = -100.0;
            i.candidate = vec![-110.0];
            i.noise_floor = 0.0;
        });
        assert_eq!(r.required_gain, 3.0);
        assert_eq!(r.verdict, Verdict::PendingConfirm);
    }

    // blocked: "a failed guardrail blocks even a great, confirmed result"
    #[test]
    fn a_failed_guardrail_blocks_even_a_great_confirmed_result() {
        let r = run(|i| {
            i.candidate = vec![50.0];
            i.confirm = Some(vec![50.0]);
            i.guardrails = vec![ok(), bad()];
        });
        assert_eq!(r.verdict, Verdict::Blocked);
        assert_eq!(
            r.reasons,
            ["guardrail failed: pytest -q (exited with status 1)"]
        );
    }

    // "a protected path violation blocks"
    #[test]
    fn a_protected_path_violation_blocks() {
        let r = run(|i| {
            i.protected_violations = vec![ProtectedViolation {
                path: "bench/x.js".into(),
                pattern: "bench/**".into(),
            }]
        });
        assert_eq!(r.verdict, Verdict::Blocked);
        assert!(r.reasons[0].contains("protected path changed: bench/x.js (matches bench/**)"));
    }

    // "blocked even when there is no gain (not rejected)"
    #[test]
    fn blocked_even_when_there_is_no_gain() {
        assert_eq!(
            run(|i| {
                i.candidate = vec![120.0];
                i.guardrails = vec![bad()];
            })
            .verdict,
            Verdict::Blocked
        );
    }

    // "lists all blocking reasons"
    #[test]
    fn lists_all_blocking_reasons() {
        let r = run(|i| {
            i.guardrails = vec![
                bad(),
                GuardrailResult {
                    name: "error_rate < 0.001".into(),
                    passed: false,
                    detail: None,
                },
            ];
            i.protected_violations = vec![ProtectedViolation {
                path: "ratchet.yaml".into(),
                pattern: "ratchet.yaml".into(),
            }];
        });
        assert_eq!(r.reasons.len(), 3);
    }

    // "passing guardrails and no violations do not block"
    #[test]
    fn passing_guardrails_and_no_violations_do_not_block() {
        assert_eq!(
            run(|i| i.guardrails = vec![]).verdict,
            Verdict::PendingConfirm
        );
    }

    // invalid input: "throws on empty candidate samples"
    #[test]
    fn errors_on_empty_candidate_samples() {
        assert_eq!(
            run_err(|i| i.candidate = vec![]),
            RatchetError::NoCandidateSamples
        );
    }

    // "throws on non-finite or negative parameters"
    #[test]
    fn errors_on_non_finite_or_negative_parameters() {
        assert!(matches!(
            run_err(|i| i.current_best = f64::NAN),
            RatchetError::NotFinite { .. }
        ));
        assert!(matches!(
            run_err(|i| i.noise_floor = -1.0),
            RatchetError::InvalidParameter(_)
        ));
        assert!(matches!(
            run_err(|i| i.min_gain = -0.1),
            RatchetError::InvalidParameter(_)
        ));
        assert!(matches!(
            run_err(|i| i.candidate = vec![f64::NAN]),
            RatchetError::Samples(_)
        ));
    }

    #[test]
    fn reasons_format_numbers_like_javascript() {
        let r = run(|i| {
            i.candidate = vec![90.123456789];
            i.noise_floor = 0.0;
        });
        assert_eq!(
            r.reasons[0],
            "bench improvement 9.87654 exceeds required gain 3"
        );
    }
}
