//! Compose core's `evaluate_ratchet` with bench outcomes (port of `evaluate.ts`).

use std::collections::BTreeMap;

use ratchet_core::bench::calibrate::calibration::FALLBACK_GAIN;
use ratchet_core::config::Objective;
use ratchet_core::ports::BenchOutcome;
use ratchet_core::ratchet::RatchetError;
use ratchet_core::{
    BenchPhase, BenchStats, CandidateVerdict, GuardrailResult, ProtectedViolation, RatchetInput,
    RatchetOutcome, Verdict, evaluate_ratchet, summarize,
};

use crate::flatten::median_metrics;

/// A bench result in the shape of `LedgerStore.record_bench_result` (minus `candidate_id`).
#[derive(Debug, Clone, PartialEq)]
pub struct BenchResultRecord {
    pub phase: BenchPhase,
    pub samples: Vec<f64>,
    pub stats: BenchStats,
    pub guardrails: Vec<GuardrailResult>,
    pub started_at: i64,
    pub finished_at: i64,
}

/// Stats stored with a bench result: `value` (the median, the number the ratchet compares), the usual summary (`n`, `mean`,
/// `median`, `stddev`, `cv`, `min`, `max`) and every other metric's median under `metrics.<key>` (e.g.
/// `metrics.peak_rss_mb`). Returns `None` without samples.
pub fn bench_stats(outcome: &BenchOutcome) -> Option<BenchStats> {
    if outcome.samples.is_empty() {
        return None;
    }
    let s = summarize(&outcome.samples).ok()?;
    let mut stats = BenchStats::new(s.median)
        .with("n", s.n as f64)
        .with("mean", s.mean)
        .with("median", s.median)
        .with("stddev", s.stddev)
        .with("cv", s.cv)
        .with("min", s.min)
        .with("max", s.max);
    for (k, v) in median_metrics(outcome.runs.iter().map(|r| &r.metrics)) {
        stats.extra.insert(format!("metrics.{k}"), v);
    }
    // JSON cannot carry Infinity (cv with mean 0)
    stats.extra.retain(|_, v| v.is_finite());
    Some(stats)
}

/// Inverse of the `metrics.<key>` entries of `bench_stats`: the baseline metrics to hand to `BenchRunner::run`.
pub fn metrics_from_stats(stats: &BTreeMap<String, f64>) -> BTreeMap<String, f64> {
    stats
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("metrics.").map(|m| (m.to_owned(), *v)))
        .collect()
}

pub fn bench_result_record(phase: BenchPhase, outcome: &BenchOutcome) -> Option<BenchResultRecord> {
    let stats = bench_stats(outcome)?;
    Some(BenchResultRecord {
        phase,
        samples: outcome.samples.clone(),
        stats,
        guardrails: outcome.guardrails.clone(),
        started_at: outcome.started_at,
        finished_at: outcome.finished_at,
    })
}

#[derive(Debug, Clone)]
pub struct EvaluateCandidateInput<'a> {
    pub objective: &'a Objective,
    /// Current best verified value on the bench.
    pub current_best: f64,
    /// The win threshold (a fraction) when it is not fixed in ratchet.yaml: the calibrated one (core's `calibrate`). Wins
    /// over `objective.min_gain`; with neither, the calibration fallback.
    pub min_gain: Option<f64>,
    /// Baseline noise floor in metric units (core's `noise_floor`).
    pub noise_floor: f64,
    /// The current best's value on the hidden trace; defaults to `current_best` (see core's `RatchetInput`).
    pub confirm_best: Option<f64>,
    pub bench: &'a BenchOutcome,
    /// Present once the confirm run has completed.
    pub confirm: Option<&'a BenchOutcome>,
    pub protected_violations: &'a [ProtectedViolation],
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateEvaluation {
    /// The rule's verdict, including `pending_confirm`.
    pub verdict: Verdict,
    /// The value to store on the candidate row (`pending_confirm` is stored as `pending`).
    pub ledger_verdict: CandidateVerdict,
    /// True when the bench win must now be confirmed on the hidden trace on a fresh instance.
    pub needs_confirm: bool,
    /// True iff the ratchet may advance.
    pub verified: bool,
    pub reasons: Vec<String>,
    /// Raw rule outcome. `candidate_value`/`improvement` are NaN when the bench produced no samples.
    pub outcome: RatchetOutcome,
    /// Guardrails of the bench run followed by those of the confirm run.
    pub guardrails: Vec<GuardrailResult>,
    /// Rows to store with `LedgerStore.record_bench_result` (candidate id added by the caller); absent runs are omitted.
    pub results: Vec<BenchResultRecord>,
}

/// Compose core's `evaluate_ratchet` with bench outcomes. Any failed guardrail (from either run) or protected-path violation
/// blocks. A bench that produced no samples (it never ran because a command guardrail failed, or a run crashed) is blocked
/// by its failed guardrail without calling the rule (which needs at least one sample).
pub fn evaluate_candidate(
    input: &EvaluateCandidateInput<'_>,
) -> Result<CandidateEvaluation, RatchetError> {
    let (bench, confirm) = (input.bench, input.confirm);
    let min_gain = input
        .min_gain
        .or(input.objective.min_gain)
        .unwrap_or(FALLBACK_GAIN);
    let mut guardrails = bench.guardrails.clone();
    if let Some(c) = confirm {
        guardrails.extend(c.guardrails.iter().cloned());
    }
    let mut results: Vec<BenchResultRecord> = Vec::new();
    results.extend(bench_result_record(BenchPhase::Bench, bench));
    if let Some(c) = confirm {
        results.extend(bench_result_record(BenchPhase::Confirm, c));
    }

    let outcome = if bench.samples.is_empty() {
        let mut reasons: Vec<String> = Vec::new();
        for g in &guardrails {
            if !g.passed {
                reasons.push(format!(
                    "guardrail failed: {}{}",
                    g.name,
                    g.detail
                        .as_ref()
                        .filter(|d| !d.is_empty())
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default()
                ));
            }
        }
        for v in input.protected_violations {
            reasons.push(format!(
                "protected path changed: {} (matches {})",
                v.path, v.pattern
            ));
        }
        if reasons.is_empty() {
            reasons.push("the benchmark produced no samples".into());
        }
        RatchetOutcome {
            verdict: Verdict::Blocked,
            reasons,
            candidate_value: f64::NAN,
            improvement: f64::NAN,
            required_gain: input.noise_floor.max(min_gain * input.current_best.abs()),
            confirm_improvement: None,
        }
    } else {
        evaluate_ratchet(&RatchetInput {
            direction: input.objective.direction,
            min_gain,
            noise_floor: input.noise_floor,
            current_best: input.current_best,
            candidate: bench.samples.clone(),
            confirm: confirm
                .filter(|c| !c.samples.is_empty())
                .map(|c| c.samples.clone()),
            confirm_best: input.confirm_best,
            guardrails: guardrails.clone(),
            protected_violations: input.protected_violations.to_vec(),
        })?
    };
    let verdict = outcome.verdict;
    Ok(CandidateEvaluation {
        verdict,
        ledger_verdict: verdict.to_candidate_verdict(),
        needs_confirm: verdict == Verdict::PendingConfirm,
        verified: verdict == Verdict::Verified,
        reasons: outcome.reasons.clone(),
        outcome,
        guardrails,
        results,
    })
}
