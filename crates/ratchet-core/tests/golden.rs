//! Differential tests against the TypeScript implementation.
//!
//! The fixtures in `tests/golden/*.json` were produced by running the (since deleted) TypeScript code on seeded
//! random and hand-picked inputs. Every test feeds the same inputs to the Rust code and requires the same
//! answer: identical strings, identical errors, identical floating-point numbers. This is what "behaviour
//! identical" means for statistics, auto-calibration, the ratchet rule, protected-path matching, number
//! formatting, config validation and template rendering.

use std::path::PathBuf;

use ratchet_core::bench::calibrate::{CalibrationInput, calibrate, format_percent, nice_gain};
use ratchet_core::bench::stats::{NoiseFloorOptions, noise_floor, summarize};
use ratchet_core::config::{
    GuardrailResult, ThresholdGuardrail, evaluate_threshold, parse_config, parse_threshold,
};
use ratchet_core::enums::Direction;
use ratchet_core::numfmt::{js_number_to_string, to_fixed, to_precision};
use ratchet_core::protected::{find_protected_violations, matches_glob, normalize_repo_path};
use ratchet_core::protected_set::{
    ProtectedConfigInput, effective_protected, evaluator_dirs_from_run,
};
use ratchet_core::ratchet::{RatchetInput, evaluate_ratchet};
use ratchet_core::templates::{
    RenderTemplateOptions, TemplateFilter, build_template_proposal, list_templates, render_template,
};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.json"));
    serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Collects mismatches so a failure shows several of them.
#[derive(Default)]
struct Diffs(Vec<String>);

impl Diffs {
    fn check<T: PartialEq + std::fmt::Debug>(
        &mut self,
        what: impl FnOnce() -> String,
        got: T,
        want: T,
    ) {
        if got != want {
            self.0
                .push(format!("{}\n    rust: {got:?}\n      ts: {want:?}", what()));
        }
    }

    fn finish(self, total: usize) {
        assert!(
            self.0.is_empty(),
            "{} of {total} cases differ from the TypeScript output; first ones:\n{}",
            self.0.len(),
            self.0
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// Numbers compare as `f64` (JSON integers and floats are the same JavaScript number).
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
    }
}

/// `"NaN"` / `"Infinity"` markers of the generator back to numbers.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap(),
        Value::String(s) => match s.as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            other => panic!("not a number marker: {other}"),
        },
        other => panic!("not a number: {other}"),
    }
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(num).collect()
}

fn opt_num(v: Option<&Value>) -> Option<f64> {
    v.filter(|v| !v.is_null()).map(num)
}

#[test]
fn number_formatting_matches_v8() {
    let f = load("numfmt");
    let mut d = Diffs::default();
    let mut total = 0;
    for r in f["toFixed"].as_array().unwrap() {
        let (x, digits, want) = (
            num(&r[0]),
            r[1].as_u64().unwrap() as usize,
            r[2].as_str().unwrap(),
        );
        d.check(
            || format!("({x:e}).toFixed({digits})"),
            to_fixed(x, digits),
            want.to_owned(),
        );
        total += 1;
    }
    for r in f["toPrecision"].as_array().unwrap() {
        let (x, p, want) = (
            num(&r[0]),
            r[1].as_u64().unwrap() as usize,
            r[2].as_str().unwrap(),
        );
        d.check(
            || format!("({x:e}).toPrecision({p})"),
            to_precision(x, p),
            want.to_owned(),
        );
        total += 1;
    }
    for r in f["toString"].as_array().unwrap() {
        let (x, want) = (num(&r[0]), r[1].as_str().unwrap());
        d.check(
            || format!("String({x:e})"),
            js_number_to_string(x),
            want.to_owned(),
        );
        total += 1;
    }
    d.finish(total);
}

#[test]
fn statistics_match_typescript_bit_for_bit() {
    let f = load("stats");
    let mut d = Diffs::default();
    let cases = f.as_array().unwrap();
    for c in cases {
        let samples = nums(&c["samples"]);
        let s = summarize(&samples).unwrap();
        let want = &c["summary"];
        let what = || format!("summarize({samples:?})");
        d.check(
            what,
            (s.n as f64, s.mean, s.median, s.stddev, s.min, s.max),
            (
                num(&want["n"]),
                num(&want["mean"]),
                num(&want["median"]),
                num(&want["stddev"]),
                num(&want["min"]),
                num(&want["max"]),
            ),
        );
        let cv_want = num(&want["cv"]);
        d.check(
            || format!("cv({samples:?})"),
            s.cv.to_bits(),
            cv_want.to_bits(),
        );
        for n in c["noise"].as_array().unwrap() {
            let opts = &n[0];
            let o = NoiseFloorOptions {
                k: opt_num(opts.get("k")),
                min_absolute: opt_num(opts.get("minAbsolute")),
            };
            d.check(
                || format!("noiseFloor({samples:?}, {opts})"),
                noise_floor(&samples, o).unwrap(),
                num(&n[1]),
            );
        }
    }
    d.finish(cases.len());
}

#[test]
fn calibration_matches_typescript_bit_for_bit() {
    let f = load("calibrate");
    let mut d = Diffs::default();
    let cases = f.as_array().unwrap();
    for c in cases {
        let i = &c["input"];
        let samples = nums(&i["samples"]);
        let input = CalibrationInput {
            samples: &samples,
            repeats: i.get("repeats").map(|v| v.as_u64().unwrap()),
            min_gain: opt_num(i.get("minGain")),
            noise_factor: opt_num(i.get("noiseFactor")),
        };
        let got = serde_json::to_value(calibrate(&input).unwrap()).unwrap();
        if !same_json(&got, &c["out"]) {
            d.0.push(format!(
                "calibrate({i})\n    rust: {got}\n      ts: {}",
                c["out"]
            ));
        }
    }
    d.finish(cases.len());
}

#[test]
fn nice_gain_and_percent_formatting_match_typescript() {
    let f = load("nice_gain");
    let mut d = Diffs::default();
    for r in f.as_array().unwrap() {
        let x = num(&r[0]);
        d.check(|| format!("niceGain({x})"), nice_gain(x), num(&r[1]));
        d.check(
            || format!("formatPercent({x})"),
            format_percent(x),
            r[2].as_str().unwrap().to_owned(),
        );
    }
    d.finish(f.as_array().unwrap().len());
}

#[test]
fn the_ratchet_rule_matches_typescript() {
    let f = load("rule");
    let mut d = Diffs::default();
    let cases = f.as_array().unwrap();
    for c in cases {
        let input: RatchetInput = serde_json::from_value(c["input"].clone()).unwrap();
        let got = serde_json::to_value(evaluate_ratchet(&input).unwrap()).unwrap();
        if !same_json(&got, &c["out"]) {
            d.0.push(format!(
                "evaluateRatchet({})\n    rust: {got}\n      ts: {}",
                c["input"], c["out"]
            ));
        }
    }
    d.finish(cases.len());
}

#[test]
fn glob_matching_and_path_normalisation_match_typescript() {
    let f = load("glob");
    let mut d = Diffs::default();
    let mut total = 0;
    let mut skipped = 0;
    for r in f["pairs"].as_array().unwrap() {
        let (p, g) = (r[0].as_str().unwrap(), r[1].as_str().unwrap());
        match r[2].as_bool() {
            Some(want) => {
                d.check(
                    || format!("matchesGlob({p:?}, {g:?})"),
                    matches_glob(p, g),
                    want,
                );
                total += 1;
            }
            None => skipped += 1, // the TypeScript threw (invalid RegExp); Rust never matches instead
        }
    }
    for r in f["normalize"].as_array().unwrap() {
        let p = r[0].as_str().unwrap();
        d.check(
            || format!("normalizeRepoPath({p:?})"),
            normalize_repo_path(p),
            r[1].as_str().unwrap().to_owned(),
        );
        total += 1;
    }
    for c in f["violations"].as_array().unwrap() {
        let paths: Vec<&str> = c["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let globs: Vec<&str> = c["globs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let got = serde_json::to_value(find_protected_violations(&paths, &globs)).unwrap();
        if got != c["out"] {
            d.0.push(format!(
                "findProtectedViolations({paths:?}, {globs:?})\n    rust: {got}\n      ts: {}",
                c["out"]
            ));
        }
        total += 1;
    }
    assert!(
        skipped < 200,
        "too many invalid globs in the fixture: {skipped}"
    );
    d.finish(total);
}

#[test]
fn the_effective_protected_set_matches_typescript() {
    let f = load("protected_set");
    let mut d = Diffs::default();
    let mut total = 0;
    for r in f["runs"].as_array().unwrap() {
        let cmd = r[0].as_str().unwrap();
        let want: Vec<String> = r[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        d.check(
            || format!("evaluatorDirsFromRun({cmd:?})"),
            evaluator_dirs_from_run(cmd),
            want,
        );
        total += 1;
    }
    for c in f["configs"].as_array().unwrap() {
        let i = &c["input"];
        let s = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
        let input = ProtectedConfigInput {
            protected: i
                .get("protected")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
                .unwrap_or_default(),
            evaluator_run: s(i["evaluator"].get("run")),
            evaluator_template: s(i["evaluator"].get("template")),
            environment_image: s(i.get("environment").and_then(|e| e.get("image"))),
            environment_scope: s(i.get("environment").and_then(|e| e.get("scope"))),
            infra: s(i.get("infra")),
        };
        let path = c["configPath"].as_str();
        let got = serde_json::to_value(effective_protected(&input, path)).unwrap();
        if got != c["out"] {
            d.0.push(format!(
                "effectiveProtected({i}, {path:?})\n    rust: {got}\n      ts: {}",
                c["out"]
            ));
        }
        total += 1;
    }
    d.finish(total);
}

#[test]
fn guardrail_thresholds_match_typescript() {
    let f = load("guardrails");
    let mut d = Diffs::default();
    let mut total = 0;
    for r in f["parsed"].as_array().unwrap() {
        let raw = r[0].as_str().unwrap();
        match (&r[1], parse_threshold("m", raw)) {
            (Value::String(want), Err(got)) => {
                d.check(|| format!("parseThreshold({raw:?})"), got, want.clone())
            }
            (want @ Value::Object(_), Ok(got)) => {
                let mut v = serde_json::to_value(&got).unwrap();
                v.as_object_mut()
                    .unwrap()
                    .insert("kind".into(), "threshold".into());
                if !same_json(&v, want) {
                    d.0.push(format!(
                        "parseThreshold({raw:?})\n    rust: {v}\n      ts: {want}"
                    ));
                }
            }
            (want, got) => {
                d.0.push(format!("parseThreshold({raw:?}): ts {want}, rust {got:?}"))
            }
        }
        total += 1;
    }
    for c in f["evals"].as_array().unwrap() {
        let g: ThresholdGuardrail = serde_json::from_value(c["g"].clone()).unwrap();
        let (observed, baseline) = (num(&c["observed"]), opt_num(Some(&c["baseline"])));
        let got = serde_json::to_value(evaluate_threshold(&g, observed, baseline)).unwrap();
        if !same_json(&got, &c["out"]) {
            d.0.push(format!(
                "evaluateThreshold({}, {observed}, {baseline:?})\n    rust: {got}\n      ts: {}",
                c["g"], c["out"]
            ));
        }
        total += 1;
    }
    d.finish(total);
}

#[test]
fn templates_render_and_propose_exactly_like_typescript() {
    let f = load("templates");
    let mut d = Diffs::default();
    let mut total = 0;

    let got = serde_json::to_value(list_templates(TemplateFilter::default())).unwrap();
    if !same_json(&got, &f["list"]) {
        d.0.push(format!(
            "listTemplates()\n    rust: {got}\n      ts: {}",
            f["list"]
        ));
    }
    total += 1;

    for (id, sets) in f["rendered"].as_object().unwrap() {
        for case in sets.as_array().unwrap() {
            let o = &case["options"];
            let opts = RenderTemplateOptions {
                metric: o.get("metric").and_then(Value::as_str).map(str::to_owned),
                direction: o
                    .get("direction")
                    .and_then(Value::as_str)
                    .map(|s| s.parse::<Direction>().unwrap()),
                value_usd: o.get("valueUsd").map(num),
            };
            let r = render_template(id, &opts).unwrap();
            let want_files: Vec<(String, String)> = case["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    (
                        f["path"].as_str().unwrap().to_owned(),
                        f["content"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            let got_files: Vec<(String, String)> = r
                .files
                .iter()
                .map(|f| (f.path.clone(), f.content.clone()))
                .collect();
            d.check(
                || format!("renderTemplate({id}, {o}).files"),
                got_files,
                want_files,
            );
            let want = &case["resolved"];
            d.check(
                || format!("renderTemplate({id}, {o}).options"),
                (
                    r.options.metric.clone(),
                    r.options.direction.as_str().to_owned(),
                    r.options.value_usd,
                ),
                (
                    want["metric"].as_str().unwrap().to_owned(),
                    want["direction"].as_str().unwrap().to_owned(),
                    num(&want["valueUsd"]),
                ),
            );
            let p = build_template_proposal(id, &opts).unwrap();
            let w = &case["proposal"];
            d.check(
                || format!("buildTemplateProposal({id}, {o})"),
                (
                    p.branch.clone(),
                    p.commit_message.clone(),
                    p.title.clone(),
                    p.body.clone(),
                ),
                (
                    w["branch"].as_str().unwrap().to_owned(),
                    w["commitMessage"].as_str().unwrap().to_owned(),
                    w["title"].as_str().unwrap().to_owned(),
                    w["body"].as_str().unwrap().to_owned(),
                ),
            );
            total += 3;
        }
    }
    d.finish(total);
}

#[test]
fn config_validation_matches_typescript_on_a_mutation_corpus() {
    let f = load("config");
    let mut d = Diffs::default();
    let cases = f.as_array().unwrap();
    let (mut ok_cases, mut err_cases) = (0, 0);
    for c in cases {
        let yaml = c["yaml"].as_str().unwrap();
        let want = &c["result"];
        match parse_config(yaml) {
            Ok(cfg) => {
                ok_cases += 1;
                let got = serde_json::to_value(&cfg).unwrap();
                let mut ts = want["config"].clone();
                if want["ok"] != Value::Bool(true) {
                    d.0.push(format!(
                        "parse_config succeeded but TypeScript failed:\n{yaml}\n  ts: {want}"
                    ));
                    continue;
                }
                ts.as_object_mut().unwrap().remove("bench"); // alias of `evaluator`
                if !same_json(&got, &ts) {
                    d.0.push(format!(
                        "config differs for:\n{yaml}\n    rust: {got}\n      ts: {ts}"
                    ));
                }
            }
            Err(errors) => {
                err_cases += 1;
                if want["ok"] != Value::Bool(false) {
                    d.0.push(format!(
                        "parse_config failed but TypeScript succeeded:\n{yaml}\n  rust: {errors:?}"
                    ));
                    continue;
                }
                let got = serde_json::to_value(&errors).unwrap();
                // YAML syntax errors: libyaml and the `yaml` package word them (and place the position) differently
                let syntax = |v: &Value| {
                    v[0]["message"]
                        .as_str()
                        .is_some_and(|m| m.starts_with("invalid YAML (line "))
                };
                if syntax(&got) && syntax(&want["errors"]) {
                    continue;
                }
                if got != want["errors"] {
                    d.0.push(format!(
                        "errors differ for:\n{yaml}\n    rust: {got}\n      ts: {}",
                        want["errors"]
                    ));
                }
            }
        }
    }
    assert!(
        ok_cases > 50 && err_cases > 300,
        "corpus too one-sided: {ok_cases} ok, {err_cases} errors"
    );
    d.finish(cases.len());
}

#[test]
fn guardrail_results_round_trip_the_typescript_shape() {
    let r: GuardrailResult =
        serde_json::from_str(r#"{"name":"a","passed":false,"detail":"d"}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        r#"{"name":"a","passed":false,"detail":"d"}"#
    );
    let r: GuardrailResult = serde_json::from_str(r#"{"name":"a","passed":true}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        r#"{"name":"a","passed":true}"#
    );
}
