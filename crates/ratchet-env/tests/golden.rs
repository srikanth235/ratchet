//! Differential tests against the TypeScript implementation (`tests/golden/generate.mjs` runs it on seeded inputs).
//!
//! Every fixture case is fed to the Rust code and must give the same answer: same checks and messages, same numbers
//! (compared as JavaScript numbers), same strings. `ratchet.yaml` edits are compared by their parsed meaning, not byte
//! for byte (see PARITY.md).

use std::collections::HashMap;
use std::path::PathBuf;

use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::SandboxEnvironmentKind;
use ratchet_env::hcl::{HclBlock, HclValue, parse_hcl, strip_comments};
use ratchet_env::{
    AgentAnswer, CostEstimateOptions, EnvironmentConfigInput, EnvironmentConfigPatch,
    EnvironmentFile, EnvironmentRole, MachineSpec, SampleOptions, SetupBriefInput,
    ValidateEnvironmentInput, apply_environment_to_config_yaml, build_revise_brief,
    build_setup_brief, environment_module_of, environment_sandbox_id, estimate_machines_cost,
    format_agent_answer, hash_environment_files, parse_agent_answer, read_environment_scope,
    sample_effort, sample_environment, sample_reason, validate_environment,
};
use serde_json::{Value, json};

fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.json"));
    serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

#[derive(Default)]
struct Diffs(Vec<String>);

impl Diffs {
    fn check(&mut self, what: impl FnOnce() -> String, got: &Value, want: &Value) {
        if !same_json(got, want) {
            self.0
                .push(format!("{}\n    rust: {got}\n      ts: {want}", what()));
        }
    }
    fn finish(self, total: usize) {
        assert!(
            self.0.is_empty(),
            "{} of {total} cases differ from the TypeScript output; first ones:\n{}",
            self.0.len(),
            self.0
                .iter()
                .take(6)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// The wording of a YAML syntax error is the parser's (`yaml` in TypeScript, `serde_norway` here): everything after
/// `invalid YAML (line L, column C):` is not compared, and neither are the line and column.
fn normalize_yaml_errors(v: &Value) -> Value {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"invalid YAML \(line \d+, column \d+\): .*").unwrap()
    });
    match v {
        Value::String(t) => {
            Value::String(RE.replace(t, "invalid YAML <parser message>").into_owned())
        }
        Value::Array(a) => Value::Array(a.iter().map(normalize_yaml_errors).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), normalize_yaml_errors(x)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap().to_owned()
}

fn scope_of(v: &str) -> EnvironmentScope {
    v.parse().unwrap()
}

// ---- validate -----------------------------------------------------------------------------------------------------------

fn files_of(v: &Value) -> Vec<EnvironmentFile> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|f| EnvironmentFile::new(s(&f["path"]), s(&f["content"])))
        .collect()
}

fn apply_op(files: &mut Vec<EnvironmentFile>, op: &Value) {
    let path = s(&op["path"]);
    match op["t"].as_str().unwrap() {
        "sub" => {
            if let Some(f) = files.iter_mut().find(|f| f.path == path) {
                f.content = f.content.replacen(&s(&op["from"]), &s(&op["to"]), 1);
            }
        }
        "append" => {
            if let Some(f) = files.iter_mut().find(|f| f.path == path) {
                f.content.push_str(&s(&op["text"]));
            }
        }
        "drop" => files.retain(|f| f.path != path),
        "add" => {
            let content = match op.get("repeat").and_then(Value::as_u64) {
                Some(n) => "x".repeat(n as usize),
                None => s(&op["content"]),
            };
            files.push(EnvironmentFile::new(path, content));
        }
        "dup" => {
            if let Some(f) = files.iter().find(|f| f.path == path).cloned() {
                files.push(f);
            }
        }
        other => panic!("unknown op {other}"),
    }
}

fn machine_json(m: &MachineSpec) -> Value {
    let mut o = serde_json::Map::new();
    if let Some(k) = &m.key {
        o.insert("key".into(), json!(k));
    }
    o.insert("name".into(), json!(m.name));
    o.insert("role".into(), json!(m.role.as_str()));
    o.insert("machine".into(), json!(m.machine));
    o.insert("detail".into(), json!(m.detail));
    if let Some(d) = m.data_volume_gb {
        o.insert("dataVolumeGb".into(), json!(d));
    }
    if let Some(d) = m.root_volume_gb {
        o.insert("rootVolumeGb".into(), json!(d));
    }
    Value::Object(o)
}

#[test]
fn validate_environment_matches_typescript_on_samples_and_mutations() {
    let f = load("validate");
    let mut d = Diffs::default();
    let cases = f["cases"].as_array().unwrap();
    for (n, c) in cases.iter().enumerate() {
        let base = &f["bases"][c["base"].as_str().unwrap()];
        let mut files = files_of(&base["files"]);
        for op in c["ops"].as_array().unwrap() {
            apply_op(&mut files, op);
        }
        let scope = c["opts"]["scope"]
            .as_str()
            .map_or_else(|| scope_of(base["scope"].as_str().unwrap()), scope_of);
        let mut input = ValidateEnvironmentInput::new(scope, files);
        if c["opts"]["orig"].as_bool().unwrap() {
            input.original_config_yaml = Some(s(&base["original"]));
        }
        let extra = &c["opts"]["extra"];
        input.module_only = extra["moduleOnly"].as_bool().unwrap_or(false);
        input.hardware = extra.get("hardware").map(s);
        input.config_path = extra.get("configPath").map(s);
        input.time_limit_seconds = extra.get("timeLimitSeconds").and_then(Value::as_f64);
        input.prices = extra.get("prices").map(|p| {
            p.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_f64().unwrap()))
                .collect::<HashMap<_, _>>()
        });
        let r = validate_environment(&input);
        let got = json!({
            "ok": r.ok,
            "checks": r.checks.iter().map(|c| json!({"id": c.id, "label": c.label, "ok": c.ok, "detail": c.detail})).collect::<Vec<_>>(),
            "machines": r.machines.iter().map(machine_json).collect::<Vec<_>>(),
            "resources": serde_json::to_value(&r.resources).unwrap(),
            "cost": serde_json::to_value(r.cost).unwrap(),
            "problems": r.problems,
        });
        d.check(
            || {
                format!(
                    "case {n}: base {} ops {} opts {}",
                    c["base"], c["ops"], c["opts"]
                )
            },
            &normalize_yaml_errors(&got),
            &normalize_yaml_errors(&c["out"]),
        );
    }
    d.finish(cases.len());
}

// ---- cost ---------------------------------------------------------------------------------------------------------------

#[test]
fn estimate_machines_cost_matches_typescript() {
    let f = load("cost");
    let mut d = Diffs::default();
    for (n, c) in f.as_array().unwrap().iter().enumerate() {
        let machines: Vec<MachineSpec> = c["machines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| MachineSpec {
                key: None,
                name: s(&m["name"]),
                role: EnvironmentRole::parse(m["role"].as_str().unwrap()).unwrap(),
                machine: s(&m["machine"]),
                detail: s(&m["detail"]),
                data_volume_gb: m.get("dataVolumeGb").and_then(Value::as_f64),
                root_volume_gb: m.get("rootVolumeGb").and_then(Value::as_f64),
            })
            .collect();
        let opts = CostEstimateOptions {
            time_limit_seconds: c["opts"]["timeLimitSeconds"].as_f64().unwrap(),
            prices: c["opts"].get("prices").map(|p| {
                p.as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_f64().unwrap()))
                    .collect()
            }),
        };
        let e = estimate_machines_cost(&machines, &opts);
        let got = json!({"resources": serde_json::to_value(&e.resources).unwrap(), "cost": serde_json::to_value(e.cost).unwrap(), "unpriced": e.unpriced});
        d.check(|| format!("case {n}: {}", c["machines"]), &got, &c["out"]);
    }
    d.finish(400);
}

// ---- brief --------------------------------------------------------------------------------------------------------------

fn answer_json(a: Option<AgentAnswer>) -> Value {
    a.map_or(
        Value::Null,
        |a| json!({"scope": a.scope.as_str(), "reason": a.reason}),
    )
}

#[test]
fn agent_answers_and_briefs_match_typescript() {
    let f = load("brief");
    let mut d = Diffs::default();
    let mut total = 0;
    for c in f["answers"].as_array().unwrap() {
        d.check(
            || format!("parseAgentAnswer({})", c["text"]),
            &answer_json(parse_agent_answer(c["text"].as_str().unwrap())),
            &c["out"],
        );
        total += 1;
    }
    for c in f["formats"].as_array().unwrap() {
        let a = AgentAnswer {
            scope: scope_of(c["answer"]["scope"].as_str().unwrap()),
            reason: s(&c["answer"]["reason"]),
        };
        let text = format_agent_answer(&a);
        d.check(
            || format!("formatAgentAnswer({})", c["answer"]),
            &json!(text),
            &c["text"],
        );
        d.check(
            || format!("round trip {}", c["answer"]),
            &answer_json(parse_agent_answer(&text)),
            &c["back"],
        );
        total += 2;
    }
    for c in f["briefs"].as_array().unwrap() {
        let i = &c["input"];
        let input = SetupBriefInput {
            goal: s(&i["goal"]),
            hardware: i.get("hardware").map(s),
            time_limit_seconds: i.get("timeLimitSeconds").and_then(Value::as_f64),
        };
        d.check(
            || format!("buildSetupBrief({i})"),
            &json!(build_setup_brief(&input)),
            &c["out"],
        );
        total += 1;
    }
    for c in f["revises"].as_array().unwrap() {
        d.check(
            || format!("buildReviseBrief({})", c["message"]),
            &json!(build_revise_brief(&s(&c["message"]))),
            &c["out"],
        );
        total += 1;
    }
    d.finish(total);
}

// ---- environment ids --------------------------------------------------------------------------------------------------

#[test]
fn hashes_sandbox_ids_and_module_refs_match_typescript() {
    let f = load("environment");
    let mut d = Diffs::default();
    let mut total = 0;
    for c in f["sets"].as_array().unwrap() {
        let files = files_of(&c["files"]);
        let h = hash_environment_files(files.iter().map(|f| (f.path.as_str(), f.content.as_str())));
        d.check(
            || format!("hashEnvironmentFiles({})", c["files"]),
            &json!(h),
            &c["hash"],
        );
        total += 1;
    }
    for c in f["ids"].as_array().unwrap() {
        d.check(
            || format!("environmentSandboxId({}, {})", c["region"], c["hash"]),
            &json!(environment_sandbox_id(
                c["region"].as_str().unwrap(),
                c["hash"].as_str().unwrap()
            )),
            &c["id"],
        );
        total += 1;
    }
    for c in f["modules"].as_array().unwrap() {
        let input = EnvironmentConfigInput {
            infra: c["infra"].as_str().map(str::to_owned),
            scope: c["scope"].as_str().map(scope_of),
        };
        let got = environment_module_of(&input).map_or(Value::Null, |m| {
            json!({
                "kind": if m.kind == SandboxEnvironmentKind::Custom { "custom" } else { "drafted" },
                "path": m.path,
                "required": m.required
            })
        });
        d.check(
            || format!("environmentModuleOf({}, {})", c["infra"], c["scope"]),
            &got,
            &c["out"],
        );
        total += 1;
    }
    d.finish(total);
}

// ---- hcl ------------------------------------------------------------------------------------------------------------------

fn value_json(v: &HclValue) -> Value {
    match v {
        HclValue::String { value, dynamic } => {
            json!({"kind": "string", "value": value, "dynamic": dynamic})
        }
        HclValue::Number(n) => json!({"kind": "number", "value": n}),
        HclValue::Bool(b) => json!({"kind": "bool", "value": b}),
        HclValue::Null => json!({"kind": "null"}),
        HclValue::Object(e) => {
            json!({"kind": "object", "entries": e.iter().map(|(k, x)| json!([k, value_json(x)])).collect::<Vec<_>>()})
        }
        HclValue::List(i) => {
            json!({"kind": "list", "items": i.iter().map(value_json).collect::<Vec<_>>()})
        }
        HclValue::Expr(t) => json!({"kind": "expr", "text": t}),
    }
}

fn block_json(b: &HclBlock) -> Value {
    json!({
        "type": b.block_type,
        "labels": b.labels,
        "attrs": b.attrs.iter().map(|(k, v)| json!([k, value_json(v)])).collect::<Vec<_>>(),
        "blocks": b.blocks.iter().map(block_json).collect::<Vec<_>>(),
        "raw": b.raw,
        "line": b.line,
    })
}

#[test]
fn hcl_parsing_matches_typescript() {
    let f = load("hcl");
    let mut d = Diffs::default();
    let cases = f.as_array().unwrap();
    for c in cases {
        let text = c["text"].as_str().unwrap();
        let file = parse_hcl(text);
        let got = json!({
            "errors": file.errors,
            "blocks": file.blocks.iter().map(block_json).collect::<Vec<_>>(),
            "stripped": strip_comments(text),
        });
        let want = json!({"errors": c["errors"], "blocks": c["blocks"], "stripped": c["stripped"]});
        d.check(|| format!("parseHcl({:?})", text), &got, &want);
    }
    d.finish(cases.len());
}

// ---- ratchet.yaml edits and samples -----------------------------------------------------------------------------------------

fn yaml_value(text: &str) -> Value {
    let y: serde_norway::Value =
        serde_norway::from_str(text).unwrap_or_else(|e| panic!("{e}: {text:?}"));
    serde_json::to_value(y).unwrap()
}

#[test]
fn config_yaml_edits_mean_the_same_as_typescript() {
    let f = load("config_yaml");
    let mut d = Diffs::default();
    let mut total = 0;
    for c in f["cases"].as_array().unwrap() {
        let p = &c["patch"];
        let patch = EnvironmentConfigPatch {
            scope: scope_of(p["scope"].as_str().unwrap()),
            machine: p.get("machine").map(s),
            image: p.get("image").map(s),
            region: p.get("region").map(s),
        };
        let text = c["text"].as_str().unwrap();
        let got = apply_environment_to_config_yaml(text, &patch);
        total += 1;
        if c["error"].is_null() {
            let out = match got {
                Ok(o) => o,
                Err(e) => {
                    d.0.push(format!(
                        "{text:?} {p}: rust failed ({e}) but TypeScript produced {}",
                        c["out"]
                    ));
                    continue;
                }
            };
            // same meaning ...
            d.check(
                || format!("{text:?} {p}"),
                &yaml_value(&out),
                &yaml_value(c["out"].as_str().unwrap()),
            );
            // ... and every byte outside the environment block kept: the unedited text is a prefix or the environment
            // block is the only difference (the original's other keys are all still there)
            let before = yaml_value(text);
            if let (Value::Object(b), Value::Object(a)) = (&before, yaml_value(&out)) {
                for (k, v) in b.iter().filter(|(k, _)| *k != "environment") {
                    d.check(
                        || format!("{text:?}: key {k} kept"),
                        a.get(k).unwrap_or(&Value::Null),
                        v,
                    );
                }
            }
            d.check(
                || "scope of the result".into(),
                &json!(read_environment_scope(&out).map(|s| s.to_string())),
                &c["scopeOfOut"],
            );
        } else if got.is_ok() {
            // TypeScript refused the text; Rust may only accept what is genuinely valid YAML with a mapping root
            let doc: Result<serde_norway::Value, _> = serde_norway::from_str(text);
            assert!(
                matches!(
                    doc,
                    Ok(serde_norway::Value::Null | serde_norway::Value::Mapping(_))
                ),
                "rust accepted {text:?} which TypeScript refused ({})",
                c["error"]
            );
        }
    }
    for c in f["scopes"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        d.check(
            || format!("readEnvironmentScope({text:?})"),
            &json!(read_environment_scope(text).map(|s| s.to_string())),
            &c["scope"],
        );
        total += 1;
    }
    d.finish(total);
}

#[test]
fn samples_match_typescript() {
    let f = load("samples");
    let mut d = Diffs::default();
    for c in f["cases"].as_array().unwrap() {
        let scope = scope_of(c["scope"].as_str().unwrap());
        let o = &c["opts"];
        let opts = SampleOptions {
            original_config_yaml: o.get("originalConfigYaml").map(s),
            config_path: o.get("configPath").map(s),
            reason: o.get("reason").map(s),
        };
        let got = sample_environment(scope, &opts);
        let want = &c["out"];
        d.check(
            || format!("{scope} {o}: reason"),
            &json!(got.reason),
            &want["reason"],
        );
        let want_files = want["files"].as_array().unwrap();
        d.check(
            || format!("{scope} {o}: paths"),
            &json!(got.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()),
            &json!(
                want_files
                    .iter()
                    .map(|f| f["path"].clone())
                    .collect::<Vec<_>>()
            ),
        );
        let config_path = o.get("configPath").map_or("ratchet.yaml".to_owned(), s);
        for (g, w) in got.files.iter().zip(want_files) {
            if g.path == config_path {
                d.check(
                    || format!("{scope} {o}: config meaning"),
                    &yaml_value(&g.content),
                    &yaml_value(w["content"].as_str().unwrap()),
                );
            } else {
                d.check(
                    || format!("{scope} {o}: {}", g.path),
                    &json!(g.content),
                    &w["content"],
                );
            }
        }
    }
    d.check(
        || "MOCK_CONFIG_YAML".into(),
        &json!(ratchet_env::MOCK_CONFIG_YAML),
        &f["mockConfigYaml"],
    );
    for scope in EnvironmentScope::ALL {
        d.check(
            || format!("reason {scope}"),
            &json!(sample_reason(*scope)),
            &f["reasons"][scope.as_str()],
        );
        let (secs, usd) = sample_effort(*scope);
        d.check(
            || format!("effort {scope}"),
            &json!({"durationS": secs, "usdSpent": usd}),
            &f["effort"][scope.as_str()],
        );
    }
    d.finish(12);
}
