//! Port of `packages/bench/src/evaluate.test.ts`.

use std::collections::BTreeMap;

use ratchet_bench::testkit::{ConfigOptions, make_config};
use ratchet_bench::{EvaluateCandidateInput, bench_stats, evaluate_candidate, metrics_from_stats};
use ratchet_core::config::Objective;
use ratchet_core::ports::{BenchOutcome, BenchRun};
use ratchet_core::{
    BenchPhase, CandidateVerdict, Direction, GuardrailResult, ProtectedViolation, Verdict,
};

fn run(value: f64, extra: &[(&str, f64)]) -> BenchRun {
    let mut metrics: BTreeMap<String, f64> = BTreeMap::new();
    metrics.insert("http_req_duration.p95".into(), value);
    for (k, v) in extra {
        metrics.insert((*k).to_owned(), *v);
    }
    BenchRun {
        value,
        metrics,
        duration_ms: 10,
    }
}

fn outcome(
    values: &[f64],
    guardrails: Vec<GuardrailResult>,
    extra: &[(&str, f64)],
) -> BenchOutcome {
    BenchOutcome {
        runs: values.iter().map(|v| run(*v, extra)).collect(),
        samples: values.to_vec(),
        guardrails,
        logs_key: None,
        started_at: 1000,
        finished_at: 2000,
    }
}

fn failed(name: &str, detail: Option<&str>) -> GuardrailResult {
    GuardrailResult {
        name: name.into(),
        passed: false,
        detail: detail.map(str::to_owned),
    }
}

fn objective() -> Objective {
    make_config(&ConfigOptions::default()).objective
}

struct Case {
    objective: Objective,
    bench: BenchOutcome,
    confirm: Option<BenchOutcome>,
    confirm_best: Option<f64>,
    violations: Vec<ProtectedViolation>,
}

impl Case {
    fn new(bench: BenchOutcome) -> Self {
        Case {
            objective: objective(),
            bench,
            confirm: None,
            confirm_best: None,
            violations: vec![],
        }
    }

    fn eval(&self) -> ratchet_bench::CandidateEvaluation {
        evaluate_candidate(&EvaluateCandidateInput {
            objective: &self.objective,
            current_best: 100.0,
            min_gain: None,
            noise_floor: 1.0,
            confirm_best: self.confirm_best,
            bench: &self.bench,
            confirm: self.confirm.as_ref(),
            protected_violations: &self.violations,
        })
        .expect("evaluates")
    }
}

#[test]
fn rejects_a_noise_level_change() {
    let r = Case::new(outcome(&[99.5, 99.8, 99.6], vec![], &[])).eval();
    assert_eq!(r.verdict, Verdict::Rejected);
    assert_eq!(r.ledger_verdict, CandidateVerdict::Rejected);
    assert!(!r.needs_confirm && !r.verified);
    assert_eq!(
        r.results.iter().map(|x| x.phase).collect::<Vec<_>>(),
        [BenchPhase::Bench]
    );
}

#[test]
fn asks_for_a_confirm_run_on_a_bench_win_then_verifies_it() {
    let win = outcome(&[80.0, 81.0, 80.0], vec![], &[]);
    let pending = Case::new(win.clone()).eval();
    assert_eq!(pending.verdict, Verdict::PendingConfirm);
    assert_eq!(pending.ledger_verdict, CandidateVerdict::Pending);
    assert!(pending.needs_confirm && !pending.verified);

    let mut c = Case::new(win);
    c.confirm = Some(outcome(&[85.0, 86.0], vec![], &[]));
    c.confirm_best = Some(120.0);
    let done = c.eval();
    assert_eq!(done.verdict, Verdict::Verified);
    assert_eq!(done.ledger_verdict, CandidateVerdict::Verified);
    assert!(done.verified && !done.needs_confirm);
    assert!(
        (done
            .outcome
            .confirm_improvement
            .expect("confirm improvement")
            - 34.5)
            .abs()
            < 1e-9
    );
    assert_eq!(
        done.results.iter().map(|x| x.phase).collect::<Vec<_>>(),
        [BenchPhase::Bench, BenchPhase::Confirm]
    );
}

#[test]
fn fails_a_bench_win_that_does_not_hold_on_the_hidden_trace() {
    let mut c = Case::new(outcome(&[80.0, 80.0, 80.0], vec![], &[]));
    c.confirm = Some(outcome(&[130.0, 131.0], vec![], &[]));
    c.confirm_best = Some(120.0);
    let r = c.eval();
    assert_eq!(r.verdict, Verdict::FailedConfirm);
    assert_eq!(r.ledger_verdict, CandidateVerdict::FailedConfirm);
}

#[test]
fn blocks_on_a_failed_guardrail_from_either_run_or_a_protected_violation_whatever_the_speed() {
    let fast = [10.0, 10.0, 10.0];
    let g1 = Case::new(outcome(
        &fast,
        vec![failed("error_rate < 0.001", Some("x"))],
        &[],
    ))
    .eval();
    assert_eq!(
        (g1.verdict, g1.ledger_verdict),
        (Verdict::Blocked, CandidateVerdict::Blocked)
    );
    assert!(g1.reasons[0].contains("error_rate < 0.001"));

    let mut c = Case::new(outcome(&fast, vec![], &[]));
    c.confirm = Some(outcome(&fast, vec![failed("peak_rss < 1.2x", None)], &[]));
    let g2 = c.eval();
    assert_eq!(g2.verdict, Verdict::Blocked);
    assert_eq!(
        g2.guardrails
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["peak_rss < 1.2x"]
    );

    let mut c = Case::new(outcome(&fast, vec![], &[]));
    c.violations = vec![ProtectedViolation {
        path: "bench/k6/search.js".into(),
        pattern: "bench/**".into(),
    }];
    let v = c.eval();
    assert_eq!(v.verdict, Verdict::Blocked);
    assert!(v.reasons.join(",").contains("bench/k6/search.js"));
}

#[test]
fn blocks_without_calling_the_rule_when_the_bench_produced_no_samples() {
    let bench = BenchOutcome {
        runs: vec![],
        samples: vec![],
        guardrails: vec![failed("pnpm test", Some("exited with status 1"))],
        logs_key: None,
        started_at: 1,
        finished_at: 2,
    };
    let r = Case::new(bench).eval();
    assert_eq!(r.verdict, Verdict::Blocked);
    assert_eq!(
        r.reasons,
        ["guardrail failed: pnpm test (exited with status 1)"]
    );
    assert!(r.outcome.candidate_value.is_nan());
    assert_eq!(r.outcome.required_gain, 3.0); // max(noise 1, 3% of 100)
    assert!(r.results.is_empty(), "a ledger row needs a finite value");
}

#[test]
fn maximize_works_through_core() {
    let mut c = Case::new(outcome(&[120.0, 121.0, 120.0], vec![], &[]));
    c.objective.direction = Direction::Maximize;
    c.confirm = Some(outcome(&[125.0], vec![], &[]));
    assert_eq!(c.eval().verdict, Verdict::Verified);
}

#[test]
fn bench_stats_has_the_headline_value_summary_and_per_metric_medians() {
    let s = bench_stats(&outcome(
        &[10.0, 12.0, 11.0],
        vec![],
        &[("peak_rss_mb", 200.0)],
    ))
    .expect("stats");
    assert_eq!(s.value, 11.0);
    for (k, v) in [
        ("n", 3.0),
        ("median", 11.0),
        ("min", 10.0),
        ("max", 12.0),
        ("metrics.peak_rss_mb", 200.0),
        ("metrics.http_req_duration.p95", 11.0),
    ] {
        assert_eq!(s.get(k), Some(v), "{k}");
    }
    let flat: BTreeMap<String, f64> = s.extra.clone();
    let m = metrics_from_stats(&flat);
    assert_eq!(
        m,
        BTreeMap::from([
            ("peak_rss_mb".to_owned(), 200.0),
            ("http_req_duration.p95".to_owned(), 11.0)
        ])
    );
}

#[test]
fn bench_stats_is_none_without_samples_and_drops_non_finite_entries() {
    assert!(bench_stats(&outcome(&[], vec![], &[])).is_none());
    let s = bench_stats(&outcome(&[0.0, 0.0], vec![], &[])).expect("stats");
    assert!(s.value.is_finite() && s.extra.values().all(|v| v.is_finite()));
    let s = bench_stats(&outcome(&[-1.0, 1.0], vec![], &[])).expect("stats"); // mean 0 -> cv Infinity
    assert!(s.get("cv").is_none());
}

#[test]
fn produces_ledger_shaped_records() {
    let r = Case::new(outcome(&[80.0, 81.0, 80.0], vec![], &[])).eval();
    let rec = &r.results[0];
    assert_eq!(rec.phase, BenchPhase::Bench);
    assert_eq!(rec.samples, [80.0, 81.0, 80.0]);
    assert_eq!((rec.started_at, rec.finished_at), (1000, 2000));
    assert_eq!(rec.stats.value, 80.0);
}
