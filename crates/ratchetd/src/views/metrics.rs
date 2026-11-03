//! Evaluator metrics as stored in the ledger's `BenchStats`: `value` is the headline (the objective), every
//! flattened metric of the evaluator's JSON rides along as `metric:<name>` (engine) or `metrics.<name>` (bench
//! CLI). Port of `packages/desktop/src/main/metrics.ts`.

use std::collections::BTreeMap;

use ratchet_core::config::Guardrail;
use ratchet_core::enums::Direction;
use ratchet_core::parse_config;
use ratchet_core::rows::BenchStats;

use super::format::unit_for_metric;
use crate::api::{Better, CompareRow, MetricOption, SpanRow};

const PREFIXES: [&str; 2] = ["metric:", "metrics."];
pub const SPAN_PREFIX: &str = "span.";

/// `BenchStats` as the flat record the view models carry (`value` plus the extras).
pub fn stats_map(s: &BenchStats) -> BTreeMap<String, f64> {
    let mut m: BTreeMap<String, f64> = s.extra.iter().map(|(k, v)| (k.clone(), *v)).collect();
    m.insert("value".into(), s.value);
    m
}

/// Flattened metric name -> median value, from a stats record.
pub fn metrics_of(stats: &BTreeMap<String, f64>) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (k, v) in stats {
        if !v.is_finite() {
            continue;
        }
        if let Some(p) = PREFIXES.iter().find(|p| k.starts_with(**p)) {
            out.insert(k[p.len()..].to_owned(), *v);
        }
    }
    out
}

/// Value of metric `key` in a stats record; the objective reads the headline `value`.
pub fn stat_of(stats: Option<&BTreeMap<String, f64>>, key: &str, objective: &str) -> Option<f64> {
    let stats = stats?;
    if key == objective
        && let Some(v) = stats.get("value").filter(|v| v.is_finite())
    {
        return Some(*v);
    }
    metrics_of(stats).get(key).copied()
}

const UNIT_SUFFIXES: [&str; 8] = [
    "_mb", "_kb", "_gb", "_bytes", "_ms", "_s", "_pct", "_percent",
];

/// Threshold guardrails of a config: metric -> which way is better (`< x` = lower).
pub fn guardrail_metrics(config_yaml: &str) -> Vec<(String, Better)> {
    let Ok(cfg) = parse_config(config_yaml) else {
        return Vec::new();
    };
    cfg.guardrails
        .iter()
        .filter_map(|g| match g {
            Guardrail::Threshold(t) => Some((
                t.metric.clone(),
                if t.op.as_str().starts_with('>') {
                    Better::Higher
                } else {
                    Better::Lower
                },
            )),
            Guardrail::Command(_) => None,
        })
        .collect()
}

/// A guardrail names `peak_rss`, the evaluator prints `peak_rss_mb`: same lookup as the bench runner's.
fn resolve_name(metrics: &BTreeMap<String, f64>, name: &str) -> Option<String> {
    if metrics.contains_key(name) {
        return Some(name.to_owned());
    }
    UNIT_SUFFIXES
        .iter()
        .map(|s| format!("{name}{s}"))
        .find(|n| metrics.contains_key(n))
}

fn percentile_of(key: &str) -> Option<f64> {
    let i = key.rfind(".p")?;
    key[i + 2..].parse::<f64>().ok()
}

/// Metrics that belong with the objective:
/// - nested objective (`http_req_duration.p95`): its siblings (`.p50`, `.p95`, `.p99`);
/// - top-level objective (`wape`): the other top-level metrics that are not guardrails (`mape`, `bias`).
///
/// The objective is always included. Spans are never options.
pub fn metric_options(
    objective: &str,
    config_yaml: &str,
    stats: &BTreeMap<String, f64>,
) -> Vec<MetricOption> {
    let metrics = metrics_of(stats);
    let dot = objective.rfind('.').filter(|i| *i > 0);
    let mut names: Vec<String> = match dot {
        Some(i) => {
            let parent = &objective[..=i];
            metrics
                .keys()
                .filter(|k| k.starts_with(parent) && !k[parent.len()..].contains('.'))
                .cloned()
                .collect()
        }
        None => {
            let guards: Vec<String> = guardrail_metrics(config_yaml)
                .iter()
                .filter_map(|(m, _)| resolve_name(&metrics, m))
                .collect();
            metrics
                .keys()
                .filter(|k| !k.contains('.') && !guards.contains(k))
                .cloned()
                .collect()
        }
    };
    if !names.iter().any(|n| n == objective) {
        names.insert(0, objective.to_owned());
    }
    // Percentile siblings read best in order: p50, p95, p99.
    if names.iter().all(|k| percentile_of(k).is_some()) {
        names.sort_by(|a, b| {
            percentile_of(a)
                .partial_cmp(&percentile_of(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    names
        .into_iter()
        .map(|key| {
            let label = match dot {
                Some(i) if key.starts_with(&objective[..=i]) => key[i + 1..].to_owned(),
                _ => key.clone(),
            };
            MetricOption {
                unit: unit_for_metric(&key).to_owned(),
                objective: key == objective,
                label,
                key,
            }
        })
        .collect()
}

/// Baseline vs best for the objective family, then every threshold-guardrail metric the evaluator reported.
pub fn compare_rows(
    objective: &str,
    direction: Direction,
    config_yaml: &str,
    baseline: &BTreeMap<String, f64>,
    best: Option<&BTreeMap<String, f64>>,
) -> Vec<CompareRow> {
    let Some(best) = best else {
        return Vec::new();
    };
    let objective_better = Better::from(direction);
    let row = |key: &str, label: &str, better: Better| {
        let a = stat_of(Some(baseline), key, objective);
        let z = stat_of(Some(best), key, objective);
        CompareRow {
            key: key.to_owned(),
            label: label.to_owned(),
            unit: unit_for_metric(key).to_owned(),
            baseline: a,
            best: z,
            delta_pct: match (a, z) {
                (Some(a), Some(z)) if a != 0.0 => Some(((z - a) / a.abs()) * 100.0),
                _ => None,
            },
            better,
        }
    };
    let mut rows: Vec<CompareRow> = metric_options(objective, config_yaml, baseline)
        .iter()
        .map(|o| row(&o.key, &o.label, objective_better))
        .collect();
    let metrics = metrics_of(baseline);
    for (metric, better) in guardrail_metrics(config_yaml) {
        if let Some(key) = resolve_name(&metrics, &metric)
            && !rows.iter().any(|r| r.key == key)
        {
            rows.push(row(&key, &key, better));
        }
    }
    rows
}

/// "Where the time went": `span.<name>` metrics of the baseline vs the best verified run; `None` when either lacks them.
pub fn span_rows(
    baseline: &BTreeMap<String, f64>,
    best: Option<&BTreeMap<String, f64>>,
) -> Option<Vec<SpanRow>> {
    let best = metrics_of(best?);
    let rows: Vec<SpanRow> = metrics_of(baseline)
        .into_iter()
        .filter_map(|(k, v)| {
            let name = k.strip_prefix(SPAN_PREFIX)?;
            let b = best.get(&format!("{SPAN_PREFIX}{name}"))?;
            Some(SpanRow {
                name: name.to_owned(),
                baseline: v,
                best: *b,
            })
        })
        .collect();
    (!rows.is_empty()).then_some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(v: f64, extras: &[(&str, f64)]) -> BTreeMap<String, f64> {
        let mut m: BTreeMap<String, f64> =
            extras.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect();
        m.insert("value".into(), v);
        m
    }

    #[test]
    fn options_are_the_objectives_siblings_in_percentile_order() {
        let s = stats(
            100.0,
            &[
                ("metric:http_req_duration.p99", 158.0),
                ("metric:http_req_duration.p50", 41.0),
                ("metric:http_req_duration.p95", 100.0),
                ("metric:rps", 400.0),
            ],
        );
        let o = metric_options("http_req_duration.p95", "", &s);
        let keys: Vec<&str> = o.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "http_req_duration.p50",
                "http_req_duration.p95",
                "http_req_duration.p99"
            ]
        );
        assert_eq!(o[0].label, "p50");
        assert!(o[1].objective);
        assert_eq!(o[1].unit, "ms");
    }

    #[test]
    fn top_level_objective_lists_other_top_level_metrics() {
        let s = stats(
            18.2,
            &[
                ("metric:wape", 18.2),
                ("metric:mape", 20.0),
                ("metric:bias", 1.0),
                ("metric:span.x", 1.0),
            ],
        );
        let o = metric_options("wape", "", &s);
        let keys: Vec<&str> = o.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(keys, ["bias", "mape", "wape"]);
    }

    #[test]
    fn spans_pair_baseline_and_best() {
        let a = stats(
            1.0,
            &[("metric:span.parse", 2.0), ("metric:span.search", 90.0)],
        );
        let b = stats(
            1.0,
            &[("metric:span.parse", 1.0), ("metric:span.search", 40.0)],
        );
        let rows = span_rows(&a, Some(&b)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].name, "search");
        assert_eq!(rows[1].best, 40.0);
        assert!(span_rows(&a, None).is_none());
        assert!(span_rows(&stats(1.0, &[]), Some(&b)).is_none());
    }

    #[test]
    fn compare_rows_cover_the_family_and_the_guardrail_metrics() {
        let yaml = "version: 0\nobjective: { metric: lat.p95, direction: minimize }\nevaluator: { run: x }\nguardrails:\n  - peak_rss: \"< 1.2x\"\n";
        let a = stats(
            100.0,
            &[("metric:lat.p95", 100.0), ("metric:peak_rss_mb", 150.0)],
        );
        let b = stats(
            80.0,
            &[("metric:lat.p95", 80.0), ("metric:peak_rss_mb", 160.0)],
        );
        let rows = compare_rows("lat.p95", Direction::Minimize, yaml, &a, Some(&b));
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["lat.p95", "peak_rss_mb"]);
        assert_eq!(rows[0].delta_pct, Some(-20.0));
        assert_eq!(rows[1].better, Better::Lower);
    }
}
