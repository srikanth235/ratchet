//! Bench outcome -> ledger statistics. Port of `packages/engine/src/bench-stats.ts`.
//!
//! The engine stores each metric's median under `metric:<name>` (the bench CLI uses `metrics.<name>`; the views read
//! both).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use ratchet_core::ports::BenchOutcome;
use ratchet_core::rows::BenchStats;
use ratchet_core::{median, summarize};

/// Metric medians are stored in `BenchStats` under this prefix so they cannot clash with summary keys.
pub const METRIC_PREFIX: &str = "metric:";

pub fn samples_of(outcome: &BenchOutcome) -> Result<Vec<f64>, String> {
    let s: Vec<f64> = if outcome.samples.is_empty() {
        outcome.runs.iter().map(|r| r.value).collect()
    } else {
        outcome.samples.clone()
    };
    if s.is_empty() {
        return Err("bench outcome has no samples".into());
    }
    Ok(s)
}

/// Median of every flattened metric across runs (first-seen order).
pub fn median_metrics(outcome: &BenchOutcome) -> IndexMap<String, f64> {
    let mut by_key: IndexMap<String, Vec<f64>> = IndexMap::new();
    for run in &outcome.runs {
        for (k, v) in &run.metrics {
            if !v.is_finite() {
                continue;
            }
            by_key.entry(k.clone()).or_default().push(*v);
        }
    }
    by_key
        .into_iter()
        .filter_map(|(k, list)| median(&list).ok().map(|m| (k, m)))
        .collect()
}

/// Ledger stats row: `value` is the headline (median); summary numbers and per-metric medians ride along.
pub fn stats_of(outcome: &BenchOutcome) -> Result<BenchStats, String> {
    let samples = samples_of(outcome)?;
    let sum = summarize(&samples).map_err(|e| e.to_string())?;
    let mut stats = BenchStats::new(sum.median)
        .with("n", sum.n as f64)
        .with("mean", sum.mean)
        .with("stddev", sum.stddev)
        .with("cv", if sum.cv.is_finite() { sum.cv } else { 0.0 })
        .with("min", sum.min)
        .with("max", sum.max);
    for (k, v) in median_metrics(outcome) {
        stats.extra.insert(format!("{METRIC_PREFIX}{k}"), v);
    }
    Ok(stats)
}

/// Inverse of the prefixing in [`stats_of`]: the baseline metrics the bench runner needs for relative guardrails.
pub fn metrics_from_stats(stats: &BenchStats) -> BTreeMap<String, f64> {
    stats
        .extra
        .iter()
        .filter_map(|(k, v)| k.strip_prefix(METRIC_PREFIX).map(|m| (m.to_owned(), *v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratchet_core::ports::BenchRun;

    fn outcome(values: &[f64]) -> BenchOutcome {
        BenchOutcome {
            runs: values
                .iter()
                .map(|v| BenchRun {
                    value: *v,
                    metrics: [("peak_rss".to_owned(), 500.0 + v)].into(),
                    duration_ms: 1,
                })
                .collect(),
            samples: values.to_vec(),
            guardrails: vec![],
            logs_key: None,
            started_at: 0,
            finished_at: 1,
        }
    }

    #[test]
    fn stats_carry_the_summary_and_prefixed_metric_medians() {
        let s = stats_of(&outcome(&[100.0, 102.0, 98.0])).unwrap();
        assert_eq!(s.value, 100.0);
        assert_eq!(s.get("n"), Some(3.0));
        assert_eq!(s.get("min"), Some(98.0));
        assert_eq!(s.get("metric:peak_rss"), Some(600.0));
        assert_eq!(metrics_from_stats(&s).get("peak_rss"), Some(&600.0));
    }

    #[test]
    fn an_outcome_without_samples_is_an_error() {
        let mut o = outcome(&[]);
        o.samples.clear();
        assert!(stats_of(&o).is_err());
    }
}
