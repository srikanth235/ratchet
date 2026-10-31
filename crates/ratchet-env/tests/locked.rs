//! Port of the config / protected-path parts of `packages/core/src/environment/environment.test.ts`.

use ratchet_core::config::{RatchetConfig, parse_config};
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::protected::find_protected_violations;
use ratchet_core::protected_set::{ProtectedConfigInput, ProtectedSource, effective_protected};
use ratchet_env::{EnvironmentConfigPatch, apply_environment_to_config_yaml};

const CONFIG: &str = "# my config
version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
evaluator:
  template: http-latency   # trusted evaluation
protected:
  - tests/**
";

fn cfg(yaml: &str) -> RatchetConfig {
    parse_config(yaml).unwrap_or_else(|e| panic!("{e:?}"))
}

fn with_scope(scope: EnvironmentScope) -> RatchetConfig {
    cfg(&apply_environment_to_config_yaml(CONFIG, &EnvironmentConfigPatch::scope(scope)).unwrap())
}

fn protected(c: &RatchetConfig) -> ratchet_core::protected_set::EffectiveProtected {
    effective_protected(&ProtectedConfigInput::from(c), None)
}

// "is optional and validated"
#[test]
fn environment_scope_is_optional_and_validated() {
    assert!(parse_config(CONFIG).is_ok());
    assert_eq!(
        with_scope(EnvironmentScope::Full)
            .environment
            .unwrap()
            .scope,
        Some(EnvironmentScope::Full)
    );
    let bad = parse_config(&format!("{CONFIG}environment:\n  scope: huge\n")).unwrap_err();
    assert!(
        bad[0]
            .message
            .contains("environment.scope must be one of none, single, full")
    );
}

// "protects infra/ratchet/** as soon as environment.scope is set, whatever the scope"
#[test]
fn protects_infra_ratchet_as_soon_as_the_scope_is_set() {
    for scope in EnvironmentScope::ALL {
        let p = protected(&with_scope(*scope));
        assert!(p.globs.contains(&"infra/ratchet/**".to_string()), "{scope}");
        let e = p
            .entries
            .iter()
            .find(|e| e.glob == "infra/ratchet/**")
            .unwrap();
        assert_eq!(
            (e.source, e.reason.as_str()),
            (ProtectedSource::Environment, "the approved environment")
        );
    }
    assert!(
        !protected(&cfg(CONFIG))
            .globs
            .contains(&"infra/ratchet/**".to_string())
    );
}

// "a candidate that touches the drafted environment or the config is blocked; plain code is not"
#[test]
fn a_candidate_touching_the_environment_is_blocked() {
    let p = protected(&with_scope(EnvironmentScope::Full));
    let v = find_protected_violations(
        &[
            "src/app.ts",
            "infra/ratchet/main.tf",
            "infra/ratchet/scripts/api.sh",
            "ratchet.yaml",
            "infra/other/main.tf",
        ],
        &p.globs,
    );
    assert_eq!(
        v.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(),
        [
            "infra/ratchet/main.tf",
            "infra/ratchet/scripts/api.sh",
            "ratchet.yaml"
        ]
    );
}

// "an environment file is not "required" in the evaluator ref, so it never makes the bench refuse to run"
#[test]
fn an_environment_file_is_not_required_in_the_evaluator_ref() {
    let p = protected(&with_scope(EnvironmentScope::Full));
    assert!(!p.required.contains(&"infra/ratchet/**".to_string()));
    assert!(p.evaluator_globs.contains(&"infra/ratchet/**".to_string()));
}

// "applyEnvironmentToConfigYaml ... environment.machine pins the evaluator host"
#[test]
fn environment_machine_pins_the_evaluator_host() {
    let mut p = EnvironmentConfigPatch::scope(EnvironmentScope::Single);
    p.machine = Some("c7i.xlarge".into());
    p.image = Some("infra/ratchet/Dockerfile".into());
    let c = cfg(&apply_environment_to_config_yaml(CONFIG, &p).unwrap());
    let e = c.environment.as_ref().unwrap();
    assert_eq!(
        (e.machine.as_deref(), e.image.as_deref()),
        (Some("c7i.xlarge"), Some("infra/ratchet/Dockerfile"))
    );
    assert_eq!(c.evaluator.hardware, "c7i.xlarge");
}
