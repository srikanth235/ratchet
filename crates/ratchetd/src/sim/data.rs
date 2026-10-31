//! Fixed data of the simulation: the demo ratchet.yaml, scripted hypotheses, simulated evaluator stats, a PRNG.

use ratchet_core::rows::BenchStats;

use crate::views::format::round_to;

/// The demo-search ratchet.yaml (`examples/demo-search`). Used when a request carries no config.
pub const DEFAULT_CONFIG_YAML: &str = r#"version: 0
pack: perf                         # domain pack: code performance
objective:
  name: search-api p95 latency
  metric: http_req_duration.p95   # key in the benchmark's JSON output (ms)
  direction: minimize
  min_gain: 0.03                  # 3%
  value:                          # business case: what 1 ms of p95 is worth
    usd_per_unit: 1200            # USD per ms of p95 improvement per year (infra + conversion)
    period: year
    basis: estimated              # becomes `measured` once confirmed in production
    unit: ms
evaluator:                        # the trusted evaluation (alias: bench)
  kind: benchmark
  ref: ratchet/bench              # protected ref holding the trusted bench
  run: k6 run bench/k6/search.js  # writes bench-result.json (RESULT_FILE); local fallback: node dist/bench/local.js
  repeats: 5                      # also used for the baseline noise floor
  hardware: c7i.2xlarge           # pinned, dedicated host
  holdout: s3://sealed/demo-search-trace.json   # bench/traces/generate.ts --seed <hidden>
guardrails:
  - run: pnpm test:unit
  - run: pnpm test:contract
  - error_rate: "< 0.001"
  - peak_rss: "< 1.2x"
protected:                        # changes here block a candidate
  - bench/**
  - ratchet.yaml
  - tests/contract/**
  - SOLUTIONS.md
agent:
  instructions: RATCHET.md        # brief injected into every session
  allowed_instance_types: [c7i.2xlarge, c7i.xlarge, m7i.large]
"#;

/// The demo config with another objective name / metric / direction, and optionally without the value model.
pub fn demo_config_yaml(name: &str, metric: &str, direction: &str, valued: bool) -> String {
    let mut out = Vec::new();
    let mut skipping_value = false;
    for line in DEFAULT_CONFIG_YAML.lines() {
        if skipping_value {
            if line.starts_with("    ") {
                continue;
            }
            skipping_value = false;
        }
        if line.starts_with("  name:") {
            out.push(format!("  name: {}", yaml_scalar(name)));
        } else if line.starts_with("  metric:") {
            out.push(format!("  metric: {}", yaml_scalar(metric)));
        } else if line.starts_with("  direction:") {
            out.push(format!("  direction: {direction}"));
        } else if line.starts_with("  value:") && !valued {
            skipping_value = true;
        } else {
            out.push(line.to_owned());
        }
    }
    out.join("\n") + "\n"
}

fn yaml_scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ' ' | '/'))
        && !s.starts_with(' ')
        && !s.ends_with(' ');
    if plain {
        s.to_owned()
    } else {
        serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
    }
}

pub const HYPOTHESES: [&str; 8] = [
    "Memoise the tokenizer so repeated queries skip re-parsing",
    "Batch index reads to cut round trips on the hot path",
    "Replace the JSON response encoder with a pre-sized buffer writer",
    "Cache compiled query plans keyed by normalised query",
    "Avoid re-sorting results that the index already returns ordered",
    "Move the facet counting off the request path into a lazy iterator",
    "Switch the scoring loop to typed arrays to reduce GC pressure",
    "Short-circuit empty queries before they reach the index",
];

pub const PROTECTED_TOUCHES: [&str; 2] = ["bench/k6/search.js", "tests/contract/search.test.ts"];

/// Stats row of a simulated evaluator run, in the shape the real engine stores (`value` = headline median, every
/// evaluator metric as `metric:<name>`). For the search demo's objective it mirrors what `examples/demo-search`'s
/// local bench prints: the latency percentiles, rps, error rate, peak RSS and the p95 request's spans.
pub fn simulated_stats(metric: &str, v: f64, extra: &[(&str, f64)]) -> BenchStats {
    let r3 = |n: f64| round_to(n, 3);
    let mut s = BenchStats::new(v).with(&format!("metric:{metric}"), v);
    for (k, x) in extra {
        s = s.with(k, *x);
    }
    if metric == "http_req_duration.p95" {
        let search = (0.55 * (v - 118.0)).max(1.5);
        let serialize = 0.03 * v;
        let parse = 0.9;
        s = s
            .with("metric:http_req_duration.p50", r3(v * 0.41))
            .with("metric:http_req_duration.p99", r3(v * 1.58))
            .with("metric:rps", r3(498.0 - (v - 120.0) * 0.08))
            .with("metric:error_rate", 0.0002)
            .with("metric:peak_rss_mb", (150.0 + (v - 120.0) * 0.4).round())
            .with("metric:span.parse", parse)
            .with("metric:span.search", r3(search))
            .with("metric:span.serialize", r3(serialize))
            .with(
                "metric:span.wait",
                r3((v - parse - search - serialize).max(0.0)),
            );
    }
    s
}

/// mulberry32, the PRNG of the TypeScript dev engine.
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next_f64() * xs.len() as f64) as usize % xs.len()]
    }

    pub fn hex(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| char::from_digit((self.next_f64() * 16.0) as u32 % 16, 16).unwrap_or('0'))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            let x = a.next_f64();
            assert_eq!(x, b.next_f64());
            assert!((0.0..1.0).contains(&x));
        }
        assert_eq!(Rng::new(1).hex(40).len(), 40);
    }

    #[test]
    fn demo_config_variants_parse() {
        for valued in [true, false] {
            let y = demo_config_yaml(
                "Ingest throughput",
                "throughput.events_per_s",
                "maximize",
                valued,
            );
            let c = ratchet_core::parse_config(&y).unwrap_or_else(|e| panic!("{e:?}\n{y}"));
            assert_eq!(c.objective.value.is_some(), valued);
            assert_eq!(c.objective.metric, "throughput.events_per_s");
            assert_eq!(c.objective.name.as_deref(), Some("Ingest throughput"));
        }
        assert!(ratchet_core::parse_config(DEFAULT_CONFIG_YAML).is_ok());
    }

    #[test]
    fn search_demo_stats_have_spans_and_percentiles() {
        let s = simulated_stats("http_req_duration.p95", 150.0, &[]);
        assert_eq!(s.value, 150.0);
        assert!(s.get("metric:span.search").unwrap() > 1.0);
        assert_eq!(s.get("metric:http_req_duration.p50"), Some(61.5));
        assert!(
            simulated_stats("wape", 10.0, &[])
                .get("metric:span.search")
                .is_none()
        );
    }
}
