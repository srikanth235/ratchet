//! Port of `packages/core/src/config/config.test.ts` (the parts that are not guardrail unit tests, which live
//! next to the code in `config/guardrails.rs`).

use ratchet_core::config::{
    CommandGuardrail, Comparator, ConfigError, Guardrail, ThresholdGuardrail, format_config_error,
    load_config, parse_config,
};
use ratchet_core::enums::{DepsMode, Direction, EvaluatorKind, ValueBasis, ValuePeriod};

const VALID: &str = r#"
version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: k6 run bench/k6/search.js
  repeats: 5
  hardware: c7i.2xlarge
  confirm_trace: s3://sealed/search-trace.jsonl
guardrails:
  - run: pytest -q
  - run: openapi-diff --base main
  - error_rate: "< 0.001"
  - peak_rss: "< 1.2x"
protected:
  - bench/**
  - ratchet.yaml
  - tests/contract/**
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge, c7i.xlarge, m7i.large]
"#;

fn errors_of(text: &str) -> Vec<ConfigError> {
    match parse_config(text) {
        Ok(_) => panic!("expected failure"),
        Err(e) => e,
    }
}

fn err(path: &str, message: &str) -> ConfigError {
    ConfigError {
        path: path.into(),
        message: message.into(),
    }
}

/// Replace a line in VALID.
fn mutate(from: &str, to: &str) -> String {
    assert!(VALID.contains(from), "{from:?} not in VALID");
    VALID.replacen(from, to, 1)
}

// parseConfig: "parses the §4 example"
#[test]
fn parses_the_section_4_example() {
    let c = parse_config(VALID).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(c.objective.metric, "http_req_duration.p95");
    assert_eq!(c.objective.direction, Direction::Minimize);
    assert_eq!(c.objective.min_gain, Some(0.03));
    assert_eq!(c.objective.name, None);
    assert_eq!(c.bench().repeats, Some(5));
    assert_eq!(
        c.bench().confirm_trace.as_deref(),
        Some("s3://sealed/search-trace.jsonl")
    );
    assert_eq!(
        c.protected,
        ["bench/**", "ratchet.yaml", "tests/contract/**"]
    );
    assert_eq!(c.agent.allowed_instance_types.len(), 3);
    assert_eq!(
        c.guardrails,
        vec![
            Guardrail::Command(CommandGuardrail {
                run: "pytest -q".into()
            }),
            Guardrail::Command(CommandGuardrail {
                run: "openapi-diff --base main".into()
            }),
            Guardrail::Threshold(ThresholdGuardrail {
                metric: "error_rate".into(),
                op: Comparator::Lt,
                value: 0.001,
                relative: false,
                raw: "< 0.001".into()
            }),
            Guardrail::Threshold(ThresholdGuardrail {
                metric: "peak_rss".into(),
                op: Comparator::Lt,
                value: 1.2,
                relative: true,
                raw: "< 1.2x".into()
            }),
        ]
    );
}

// "accepts maximize and omits optional confirm_trace, guardrails, protected"
#[test]
fn accepts_maximize_and_omits_optional_fields() {
    let text = r#"
version: 0
objective: { metric: rps, direction: maximize, min_gain: 0.05 }
bench: { ref: r, run: ./bench.sh, repeats: 3, hardware: c7i.xlarge }
agent: { instructions: RATCHET.md, allowed_instance_types: [c7i.xlarge] }
"#;
    let c = parse_config(text).unwrap();
    assert_eq!(c.objective.direction, Direction::Maximize);
    assert_eq!(c.bench().confirm_trace, None);
    assert_eq!(c.guardrails, vec![]);
    assert_eq!(c.protected, Vec::<String>::new());
}

// "reports invalid YAML syntax with a line number"
#[test]
fn reports_invalid_yaml_syntax_with_a_line_number() {
    let errs = errors_of("version: 0\nobjective: [unclosed\n");
    let m = &errs[0].message;
    assert!(m.starts_with("invalid YAML (line "), "{m}");
    assert!(m.contains(", column "), "{m}");
    assert_eq!(errs[0].path, "");
}

// "reports empty and non-mapping documents"
#[test]
fn reports_empty_and_non_mapping_documents() {
    assert!(errors_of("")[0].message.contains("empty"));
    assert!(errors_of("# only a comment\n")[0].message.contains("empty"));
    assert!(errors_of("- a\n- b")[0].message.contains("mapping"));
    assert!(errors_of("just a string")[0].message.contains("mapping"));
}

// "reports missing required sections with their paths"
#[test]
fn reports_missing_required_sections_with_their_paths() {
    let errs = errors_of("version: 0\n");
    let mut paths: Vec<&str> = errs.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["evaluator", "objective"]);
    let first = format_config_error(&errs[0]);
    assert!(
        first.starts_with("evaluator: ") || first.starts_with("objective: "),
        "{first}"
    );
    assert!(errs.iter().all(|e| e.message == "is required"));
}

// "reports a wrong version"
#[test]
fn reports_a_wrong_version() {
    let errs = errors_of(&mutate("version: 0", "version: 1"));
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].path, "version");
    assert!(errs[0].message.contains("must be 0"));
}

// "reports a bad direction at objective.direction"
#[test]
fn reports_a_bad_direction() {
    let errs = errors_of(&mutate("direction: minimize", "direction: lower"));
    assert_eq!(
        errs,
        vec![err(
            "objective.direction",
            "direction must be \"minimize\" or \"maximize\""
        )]
    );
}

// "validates min_gain range and type"
#[test]
fn validates_min_gain_range_and_type() {
    assert_eq!(
        errors_of(&mutate("min_gain: 0.03", "min_gain: 3"))[0].path,
        "objective.min_gain"
    );
    assert!(
        errors_of(&mutate("min_gain: 0.03", "min_gain: -0.1"))[0]
            .message
            .contains(">= 0")
    );
    assert!(
        errors_of(&mutate("min_gain: 0.03", "min_gain: 3%"))[0]
            .message
            .contains("must be a number")
    );
    assert!(parse_config(&mutate("min_gain: 0.03", "min_gain: 0")).is_ok());
}

// "validates repeats"
#[test]
fn validates_repeats() {
    assert_eq!(
        errors_of(&mutate("repeats: 5", "repeats: 0"))[0].path,
        "evaluator.repeats"
    );
    assert!(
        errors_of(&mutate("repeats: 5", "repeats: 2.5"))[0]
            .message
            .contains("integer")
    );
}

// "rejects unknown keys (typos) with their location"
#[test]
fn rejects_unknown_keys_with_their_location() {
    let errs = errors_of(&mutate(
        "  hardware: c7i.2xlarge",
        "  hardware: c7i.2xlarge\n  hardwre: x",
    ));
    assert_eq!(errs, vec![err("evaluator", "unknown key \"hardwre\"")]);
    assert_eq!(
        errors_of(&format!("{VALID}extra: 1\n"))[0],
        err("", "unknown key \"extra\"")
    );
}

// "reports bad guardrail thresholds with the index and metric in the path"
#[test]
fn reports_bad_guardrail_thresholds_with_the_index_and_metric_in_the_path() {
    let errs = errors_of(&mutate(
        "error_rate: \"< 0.001\"",
        "error_rate: \"about 0.001\"",
    ));
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].path, "guardrails[2].error_rate");
    assert!(
        errs[0]
            .message
            .contains("invalid threshold \"about 0.001\"")
    );
}

// "rejects guardrails with several keys, no keys, or non-string values"
#[test]
fn rejects_guardrails_with_several_keys_no_keys_or_non_string_values() {
    let several = errors_of(&mutate(
        "- run: pytest -q",
        "- run: pytest -q\n    error_rate: \"< 1\"",
    ));
    assert!(several[0].message.contains("exactly one key"));
    let none = errors_of(&mutate("- run: pytest -q", "- {}"));
    assert!(none[0].message.contains("exactly one key") && none[0].message.contains("found 0"));
    let numeric = errors_of(&mutate("error_rate: \"< 0.001\"", "error_rate: 0.001"));
    assert_eq!(numeric[0].path, "guardrails[2].error_rate");
    assert!(numeric[0].message.contains("must be strings"));
    assert_eq!(
        errors_of(&mutate("- run: pytest -q", "- pytest"))[0].path,
        "guardrails[0]"
    );
}

// "rejects empty run guardrail"
#[test]
fn rejects_an_empty_run_guardrail() {
    let errs = errors_of(&mutate("- run: pytest -q", "- run: \"\""));
    assert_eq!(
        errs[0],
        err("guardrails[0].run", "`run` must be a non-empty command")
    );
}

// "requires at least one instance type and string globs"
#[test]
fn requires_at_least_one_instance_type_and_string_globs() {
    assert_eq!(
        errors_of(&mutate("[c7i.2xlarge, c7i.xlarge, m7i.large]", "[]"))[0].path,
        "agent.allowed_instance_types"
    );
    assert_eq!(
        errors_of(&mutate("  - ratchet.yaml", "  - 5"))[0].path,
        "protected[1]"
    );
}

// "collects multiple errors at once"
#[test]
fn collects_multiple_errors_at_once() {
    let errs = errors_of(
        &mutate("direction: minimize", "direction: sideways").replace("repeats: 5", "repeats: 0"),
    );
    let mut paths: Vec<&str> = errs.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["evaluator.repeats", "objective.direction"]);
}

// loadConfig "loads from disk"
#[test]
fn loads_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ratchet.yaml");
    std::fs::write(&p, VALID).unwrap();
    assert!(load_config(&p).is_ok());
}

// "reports a missing file as an error"
#[test]
fn reports_a_missing_file_as_an_error() {
    let r = load_config("/nonexistent/dir/ratchet.yaml");
    assert_eq!(
        r,
        Err(vec![err("", "cannot find /nonexistent/dir/ratchet.yaml")])
    );
}

// ---- generic evaluator + value ----

const GENERIC: &str = r#"
version: 0
pack: forecasting
objective:
  name: Weekly sales forecast
  metric: wape
  direction: minimize
  min_gain: 0.02
  value: { usd_per_unit: 250000, period: year, unit: "WAPE point" }
evaluator:
  kind: backtest
  ref: ratchet/eval
  run: python -m eval.backtest
  repeats: 3
  hardware: m7i.xlarge
  holdout: s3://sealed/last-8-weeks.parquet
agent: { instructions: RATCHET.md, allowed_instance_types: [m7i.xlarge] }
"#;

// "parses `evaluator`, holdout and value, and aliases it as `bench`"
#[test]
fn parses_evaluator_holdout_and_value() {
    let c = parse_config(GENERIC).unwrap();
    assert_eq!(c.evaluator.kind, EvaluatorKind::Backtest);
    assert_eq!(
        c.evaluator.holdout.as_deref(),
        Some("s3://sealed/last-8-weeks.parquet")
    );
    assert_eq!(
        c.bench().confirm_trace.as_deref(),
        Some("s3://sealed/last-8-weeks.parquet")
    );
    assert!(std::ptr::eq(c.bench(), &c.evaluator));
    let v = c.objective.value.as_ref().unwrap();
    assert_eq!(
        (v.usd_per_unit, v.period, v.basis),
        (250_000.0, ValuePeriod::Year, ValueBasis::Estimated)
    );
    assert_eq!(v.unit.as_deref(), Some("WAPE point"));
    assert_eq!(v.note, None);
    assert_eq!(c.pack.as_deref(), Some("forecasting"));
}

// "defaults evaluator.kind to benchmark for legacy `bench:` configs"
#[test]
fn defaults_evaluator_kind_to_benchmark_for_legacy_bench_configs() {
    let r = parse_config(
        &GENERIC
            .replace("evaluator:", "bench:")
            .replace("  kind: backtest\n", ""),
    )
    .unwrap();
    assert_eq!(r.evaluator.kind, EvaluatorKind::Benchmark);
}

// "rejects a non-positive value"
#[test]
fn rejects_a_non_positive_value() {
    let errs = errors_of(&GENERIC.replace("usd_per_unit: 250000", "usd_per_unit: 0"));
    assert_eq!(errs[0].path, "objective.value.usd_per_unit");
}

// ---- evaluator.deps ----

const BASE: &str = r#"
version: 0
objective: { metric: p95, direction: minimize, min_gain: 0.03 }
evaluator:
  ref: ratchet/bench
  run: node bench.mjs
  repeats: 3
  hardware: c7i.xlarge
agent: { instructions: RATCHET.md, allowed_instance_types: [c7i.xlarge] }
"#;

fn with_deps(value: &str) -> String {
    BASE.replace(
        "  hardware: c7i.xlarge",
        &format!("  hardware: c7i.xlarge\n  deps: {value}"),
    )
}

// "defaults to auto (detect the package manager from the lockfile at evaluator.ref)"
#[test]
fn deps_default_to_auto() {
    let c = parse_config(BASE).unwrap();
    assert_eq!(c.evaluator.deps, DepsMode::Auto);
    assert_eq!(c.bench().deps, DepsMode::Auto);
}

// it.each(['auto','pnpm','npm','none']) "accepts %s, also under the legacy bench: key"
#[test]
fn deps_accepts_each_mode_also_under_the_legacy_key() {
    for (text, mode) in [
        ("auto", DepsMode::Auto),
        ("pnpm", DepsMode::Pnpm),
        ("npm", DepsMode::Npm),
        ("none", DepsMode::None),
    ] {
        assert_eq!(
            parse_config(&with_deps(text)).unwrap().evaluator.deps,
            mode,
            "{text}"
        );
        let legacy = parse_config(&with_deps(text).replace("evaluator:", "bench:")).unwrap();
        assert_eq!(legacy.bench().deps, mode, "{text} (legacy)");
    }
}

// "rejects other package managers with the list of valid ones"
#[test]
fn deps_rejects_other_package_managers_with_the_list_of_valid_ones() {
    let errs = errors_of(&with_deps("yarn"));
    assert_eq!(errs[0].path, "evaluator.deps");
    assert!(errs[0].message.contains("auto, pnpm, npm, none"));
}

// "rejects a boolean (deps: false is not a way to say none)"
#[test]
fn deps_rejects_a_boolean() {
    assert!(parse_config(&with_deps("false")).is_err());
}

// ---- beyond the TypeScript cases: behaviour the zod schema had that its tests did not pin ----

#[test]
fn a_config_round_trips_through_json() {
    let c = parse_config(VALID).unwrap();
    let json = serde_json::to_string(&c).unwrap();
    let back: ratchet_core::RatchetConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["evaluator"]["ref"], "ratchet/bench");
    assert_eq!(v["evaluator"]["confirm_trace"], v["evaluator"]["holdout"]);
    assert_eq!(v["guardrails"][2]["kind"], "threshold");
}

#[test]
fn both_bench_and_evaluator_make_bench_an_unknown_key() {
    let text = format!("{BASE}bench: {{ run: x }}\n");
    assert_eq!(errors_of(&text), vec![err("", "unknown key \"bench\"")]);
}

#[test]
fn several_unknown_keys_are_listed_together() {
    let text = format!("{BASE}zzz: 1\nyyy: 2\n");
    assert_eq!(
        errors_of(&text),
        vec![err("", "unknown keys \"zzz\", \"yyy\"")]
    );
}

#[test]
fn wrong_types_use_zods_default_wording_where_the_schema_has_no_custom_message() {
    let e = errors_of(&BASE.replace(
        "objective: { metric: p95, direction: minimize, min_gain: 0.03 }",
        "objective: 5",
    ));
    assert_eq!(
        e,
        vec![err(
            "objective",
            "Invalid input: expected object, received number"
        )]
    );
    let e = errors_of(&format!("{BASE}pack: 3\n"));
    assert_eq!(
        e,
        vec![err(
            "pack",
            "Invalid input: expected string, received number"
        )]
    );
    let e = errors_of(&format!("{BASE}pack: \"\"\n"));
    assert_eq!(
        e,
        vec![err(
            "pack",
            "Too small: expected string to have >=1 characters"
        )]
    );
}

#[test]
fn an_integral_float_is_a_valid_repeats() {
    let c = parse_config(&BASE.replace("repeats: 3", "repeats: 4.0")).unwrap();
    assert_eq!(c.evaluator.repeats, Some(4));
}

#[test]
fn a_missing_or_blank_metric_has_the_custom_messages() {
    let e = errors_of(&BASE.replace("metric: p95, ", ""));
    assert_eq!(
        e,
        vec![err("objective.metric", "objective.metric must be a string")]
    );
    let e = errors_of(&BASE.replace("metric: p95", "metric: \"\""));
    assert_eq!(
        e,
        vec![err(
            "objective.metric",
            "objective.metric must not be empty"
        )]
    );
}

#[test]
fn value_defaults_and_validation() {
    let text = BASE.replace(
        "objective: { metric: p95, direction: minimize, min_gain: 0.03 }",
        "objective: { metric: p95, direction: minimize, value: { usd_per_unit: 2, period: month, basis: measured, note: n } }",
    );
    let c = parse_config(&text).unwrap();
    let v = c.objective.value.unwrap();
    assert_eq!(
        (v.period, v.basis, v.note.as_deref()),
        (ValuePeriod::Month, ValueBasis::Measured, Some("n"))
    );
    let bad = text.replace("period: month", "period: week");
    assert_eq!(
        errors_of(&bad),
        vec![err(
            "objective.value.period",
            "value.period must be \"month\" or \"year\""
        )]
    );
}

#[test]
fn evaluator_field_messages() {
    let e = errors_of(&BASE.replace(
        "repeats: 3",
        "kind: vibes\n  noise_factor: 11\n  holdout: \"\"",
    ));
    let msgs: Vec<(&str, &str)> = e
        .iter()
        .map(|e| (e.path.as_str(), e.message.as_str()))
        .collect();
    assert_eq!(
        msgs,
        [
            (
                "evaluator.kind",
                "evaluator.kind must be one of benchmark, backtest, eval, simulation"
            ),
            ("evaluator.noise_factor", "noise_factor must be <= 10"),
            // the `min(1)` check has no message of its own: the schema's custom error is reported (zod 4)
            ("evaluator.holdout", "holdout must be a string"),
        ]
    );
}
