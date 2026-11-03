//! Flattening bench JSON into metrics (port of `flatten.ts`).

use std::collections::BTreeMap;

use ratchet_core::median;
use serde_json::Value;

/// Flatten every finite numeric leaf of a JSON value into dotted keys:
/// `{http_req_duration: {p95: 12}, rps: 3}` -> `{"http_req_duration.p95": 12, "rps": 3}`. Arrays, strings, booleans and
/// nulls are skipped.
pub fn flatten_numeric(value: &Value) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    flatten_into(value, "", &mut out);
    out
}

fn flatten_into(value: &Value, prefix: &str, out: &mut BTreeMap<String, f64>) {
    match value {
        Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && f.is_finite()
                && !prefix.is_empty()
            {
                out.insert(prefix.to_owned(), f);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_into(v, &key, out);
            }
        }
        _ => {}
    }
}

/// Per-metric median over runs (a metric is aggregated over the runs that report it).
pub fn median_metrics<'a>(
    runs: impl IntoIterator<Item = &'a BTreeMap<String, f64>>,
) -> BTreeMap<String, f64> {
    let mut by_key: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for metrics in runs {
        for (k, v) in metrics {
            by_key.entry(k.clone()).or_default().push(*v);
        }
    }
    by_key
        .into_iter()
        .filter_map(|(k, xs)| median(&xs).ok().map(|m| (k, m)))
        .collect()
}

const UNIT_SUFFIXES: [&str; 8] = [
    "_mb", "_kb", "_gb", "_bytes", "_ms", "_s", "_pct", "_percent",
];

/// Look a guardrail metric up in a flat metrics map: the exact key first, then the key with a unit suffix (`peak_rss`
/// finds `peak_rss_mb`). The demo config says `peak_rss: "< 1.2x"` while the bench prints `peak_rss_mb`; relative bounds
/// are unit-free, so the alias is safe. Returns the key found and its value.
pub fn resolve_metric(metrics: &BTreeMap<String, f64>, name: &str) -> Option<(String, f64)> {
    if let Some(v) = metrics.get(name) {
        return Some((name.to_owned(), *v));
    }
    for s in UNIT_SUFFIXES {
        let k = format!("{name}{s}");
        if let Some(v) = metrics.get(&k) {
            return Some((k, *v));
        }
    }
    None
}
