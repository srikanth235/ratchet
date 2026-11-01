//! Port of `packages/core/src/config/config-simplified.test.ts`.

use std::path::PathBuf;

use ratchet_core::config::{
    CommandGuardrail, Comparator, ConfigError, Guardrail, RatchetConfig, ThresholdGuardrail,
    parse_config,
};
use ratchet_core::enums::{DepsMode, EvaluatorKind};

const AGENT: &str = "agent:\n  instructions: RATCHET.md\n  allowed_instance_types: [c7i.large]\n";

fn minimal() -> String {
    format!(
        "version: 0\nobjective: {{ metric: http_req_duration.p95, direction: minimize }}\nevaluator:\n  template: http-latency\n{AGENT}"
    )
}

fn ok(text: &str) -> RatchetConfig {
    parse_config(text).unwrap_or_else(|e| {
        panic!(
            "{}",
            e.iter()
                .map(|e| format!("{}: {}", e.path, e.message))
                .collect::<Vec<_>>()
                .join("; ")
        )
    })
}

fn errors(text: &str) -> Vec<ConfigError> {
    parse_config(text).expect_err("expected failure")
}

fn threshold(metric: &str, op: Comparator, value: f64, raw: &str) -> Guardrail {
    Guardrail::Threshold(ThresholdGuardrail {
        metric: metric.into(),
        op,
        value,
        relative: false,
        raw: raw.into(),
    })
}

// "a template, an objective and an agent brief are enough"
#[test]
fn a_template_an_objective_and_an_agent_brief_are_enough() {
    let c = ok(&minimal());
    assert_eq!(c.objective.min_gain, None);
    assert_eq!(c.evaluator.repeats, None);
    assert_eq!(c.evaluator.template.as_deref(), Some("http-latency"));
    assert_eq!(c.evaluator.run, "node bench/http-latency.mjs");
    assert_eq!(c.evaluator.kind, EvaluatorKind::Benchmark);
    assert_eq!(c.evaluator.git_ref, "ratchet/bench");
    assert_eq!(c.evaluator.hardware, "c7i.2xlarge");
    assert_eq!(c.evaluator.deps, DepsMode::Auto);
    assert_eq!(
        c.guardrails,
        vec![threshold("error_rate", Comparator::Lt, 0.001, "< 0.001")]
    );
    assert!(std::ptr::eq(c.bench(), &c.evaluator));
}

// "explicit values beat the template and the environment"
#[test]
fn explicit_values_beat_the_template_and_the_environment() {
    let text = format!("{}guardrails:\n  - run: npm test\nenvironment: {{ machine: m7i.large }}\n", minimal())
        .replace("template: http-latency", "template: http-latency\n  run: node my/bench.js\n  hardware: c7i.4xlarge\n  kind: simulation");
    let c = ok(&text);
    assert_eq!(c.evaluator.run, "node my/bench.js");
    assert_eq!(c.evaluator.hardware, "c7i.4xlarge");
    assert_eq!(c.evaluator.kind, EvaluatorKind::Simulation);
    assert_eq!(
        c.guardrails,
        vec![Guardrail::Command(CommandGuardrail {
            run: "npm test".into()
        })]
    );
}

// "hardware falls back to environment.machine, then the template, then c7i.large"
#[test]
fn hardware_falls_back_to_environment_machine_then_the_template_then_c7i_large() {
    assert_eq!(
        ok(&format!(
            "{}environment: {{ machine: m7i.xlarge }}\n",
            minimal()
        ))
        .evaluator
        .hardware,
        "m7i.xlarge"
    );
    assert_eq!(
        ok(&minimal().replace("template: http-latency", "run: ./b.sh"))
            .evaluator
            .hardware,
        "c7i.large"
    );
    assert_eq!(
        ok(&minimal().replace("template: http-latency", "template: build-size"))
            .evaluator
            .hardware,
        "c7i.xlarge"
    );
}

// "repeats, min_gain and noise_factor are optional but validated when present"
#[test]
fn repeats_min_gain_and_noise_factor_are_optional_but_validated_when_present() {
    let c = ok(&minimal()
        .replace("direction: minimize", "direction: minimize, min_gain: 0.05")
        .replace(
            "template: http-latency",
            "template: http-latency\n  repeats: 7\n  noise_factor: 3",
        ));
    assert_eq!(c.objective.min_gain, Some(0.05));
    assert_eq!(
        (c.evaluator.repeats, c.evaluator.noise_factor),
        (Some(7), Some(3.0))
    );
    assert_eq!(
        errors(&minimal().replace(
            "template: http-latency",
            "template: http-latency\n  repeats: 0"
        ))[0]
            .path,
        "evaluator.repeats"
    );
    assert_eq!(
        errors(&minimal().replace(
            "template: http-latency",
            "template: http-latency\n  noise_factor: -1"
        ))[0]
            .path,
        "evaluator.noise_factor"
    );
    assert_eq!(
        errors(&minimal().replace("direction: minimize", "direction: minimize, min_gain: 3"))[0]
            .path,
        "objective.min_gain"
    );
}

// "needs either run or template"
#[test]
fn needs_either_run_or_template() {
    let e = errors(&minimal().replace("  template: http-latency\n", "  hardware: c7i.large\n"));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].path, "evaluator.run");
    assert!(
        e[0].message.contains("run") && e[0].message.contains("template"),
        "{}",
        e[0].message
    );
}

// "rejects an unknown template and a demo, listing the valid ids"
#[test]
fn rejects_an_unknown_template_and_a_demo_listing_the_valid_ids() {
    let e = errors(&minimal().replace("http-latency", "nope"));
    assert_eq!(e[0].path, "evaluator.template");
    let m = &e[0].message;
    assert!(m.starts_with("unknown evaluator template \"nope\""), "{m}");
    assert!(
        m.find("http-latency")
            .is_some_and(|a| m.rfind("llm-eval").is_some_and(|b| a < b)),
        "{m}"
    );
    assert!(
        errors(&minimal().replace("http-latency", "demo-search"))[0]
            .message
            .contains("unknown evaluator template")
    );
    assert_eq!(
        errors(&minimal().replace("http-latency", "Bad_Id"))[0].path,
        "evaluator.template"
    );
}

// "the bench: alias works with a template too"
#[test]
fn the_bench_alias_works_with_a_template_too() {
    assert_eq!(
        ok(&minimal().replace("evaluator:", "bench:")).evaluator.run,
        "node bench/http-latency.mjs"
    );
}

// environment: "accepts image, machine, region and network"
#[test]
fn environment_accepts_image_machine_region_and_network() {
    let c = ok(&format!(
        "{}environment:
  image: .devcontainer/Dockerfile
  machine: c7i.2xlarge
  region: eu-west-1
  network: [registry.npmjs.org, \"*.pythonhosted.org\", files.pythonhosted.org]
",
        minimal()
    ));
    let e = c.environment.unwrap();
    assert_eq!(e.image.as_deref(), Some(".devcontainer/Dockerfile"));
    assert_eq!(e.machine.as_deref(), Some("c7i.2xlarge"));
    assert_eq!(e.region.as_deref(), Some("eu-west-1"));
    assert_eq!(
        e.network.unwrap(),
        [
            "registry.npmjs.org",
            "*.pythonhosted.org",
            "files.pythonhosted.org"
        ]
    );
}

// "all four are optional, an empty block is fine, unknown keys are not"
#[test]
fn environment_keys_are_optional_and_unknown_keys_are_not() {
    let c = ok(&format!("{}environment: {{}}\n", minimal()));
    assert_eq!(c.environment, Some(Default::default()));
    assert_eq!(
        errors(&format!("{}environment: {{ gpu: true }}\n", minimal()))[0],
        ConfigError {
            path: "environment".into(),
            message: "unknown key \"gpu\"".into()
        }
    );
}

// it.each: "rejects %s"
#[test]
fn environment_rejects_bad_values() {
    let cases: [(&str, &str, &str); 12] = [
        ("image: /etc/passwd", "environment.image", "relative"),
        (
            "image: ../outside/Dockerfile",
            "environment.image",
            "no \"..\"",
        ),
        ("image: \"\"", "environment.image", "must not be empty"),
        ("machine: big", "environment.machine", "instance type"),
        ("machine: C7I.Large", "environment.machine", "instance type"),
        ("region: useast1", "environment.region", "AWS region"),
        ("region: us-east", "environment.region", "AWS region"),
        (
            "network: [\"https://registry.npmjs.org\"]",
            "environment.network[0]",
            "host names",
        ),
        (
            "network: [\"registry.npmjs.org:443\"]",
            "environment.network[0]",
            "host names",
        ),
        ("network: [\"*\"]", "environment.network[0]", "host names"),
        (
            "network: [\"localhost\"]",
            "environment.network[0]",
            "host names",
        ),
        (
            "network: registry.npmjs.org",
            "environment.network",
            "list of host names",
        ),
    ];
    for (line, path, message) in cases {
        let e = errors(&format!("{}environment: {{ {line} }}\n", minimal()));
        assert_eq!(e[0].path, path, "{line}");
        assert!(e[0].message.contains(message), "{line}: {}", e[0].message);
    }
}

// infra: "is a repo path, off by default"
#[test]
fn infra_is_a_repo_path_off_by_default() {
    assert_eq!(ok(&minimal()).infra, None);
    assert_eq!(
        ok(&format!("{}infra: ops/sandbox\n", minimal()))
            .infra
            .as_deref(),
        Some("ops/sandbox")
    );
}

// "cannot point outside the repository"
#[test]
fn infra_cannot_point_outside_the_repository() {
    assert_eq!(
        errors(&format!("{}infra: ../sandbox\n", minimal()))[0].path,
        "infra"
    );
    assert_eq!(
        errors(&format!("{}infra: /abs/path\n", minimal()))[0].path,
        "infra"
    );
}

// "needs only objective and evaluator; the agent block defaults (brief RATCHET.md, the bench machine)"
#[test]
fn needs_only_objective_and_evaluator_and_the_agent_block_defaults() {
    let mut paths: Vec<String> = errors("version: 0\n").into_iter().map(|e| e.path).collect();
    paths.sort();
    assert_eq!(paths, ["evaluator", "objective"]);
    let c = ok(
        "version: 0\nobjective: { metric: http_req_duration.p95, direction: minimize }\nevaluator: { template: http-latency }\nenvironment: { machine: c7i.xlarge }\n",
    );
    assert_eq!(c.agent.instructions, "RATCHET.md");
    assert_eq!(c.agent.allowed_instance_types, ["c7i.xlarge"]);
}

// "existing configs stay valid"
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn existing_example_configs_stay_valid() {
    for file in [
        "examples/demo-search/ratchet.yaml",
        "examples/demo-cli/ratchet.yaml",
        "examples/demo-forecast/ratchet.yaml",
    ] {
        let c = ok(&std::fs::read_to_string(repo_root().join(file)).unwrap());
        assert!(!c.evaluator.run.is_empty(), "{file}");
        assert!(c.objective.min_gain.unwrap_or(0.0) > 0.0, "{file}");
    }
}

// "the example directories are the ones listed"
#[test]
fn the_example_directories_are_the_ones_listed() {
    let mut demos: Vec<String> = std::fs::read_dir(repo_root().join("examples"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("demo-"))
        .collect();
    demos.sort();
    assert_eq!(demos, ["demo-cli", "demo-forecast", "demo-search"]);
}
