//! The brief a setup agent (the user's Claude Code / Codex, run locally in a checkout of the repo) is given
//! (`brief.ts`). The agent is asked to size an environment to the task and write ONLY what is needed; Ratchet
//! validates the result (`validate_environment`), so the brief states the rules the validator enforces to save round
//! trips, but the brief is not what makes the draft safe.

use ratchet_core::config::DEFAULT_ENVIRONMENT_INFRA_DIR;
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::numfmt::to_fixed;

use crate::cost::INSTANCE_USD_PER_HOUR;
use crate::hcl::js_trim;
use crate::types::EnvironmentRole;
use crate::validate::{
    ENVIRONMENT_OBJECT_PREFIX, MACHINE_KEYS, MAX_ENVIRONMENT_MACHINES, SANDBOX_BASE_MODULE_NAME,
    SANDBOX_BASE_MODULE_SOURCE,
};

/// The marker the agent ends its final message with, followed by one JSON object ([`AgentAnswer`]).
pub const ANSWER_MARKER: &str = "RATCHET_ENVIRONMENT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAnswer {
    pub scope: EnvironmentScope,
    /// One or two plain sentences: why this much infrastructure. Shown to the user.
    pub reason: String,
}

/// The text the agent must keep to when it describes its choice. Exposed so the mock and the tests produce the same
/// shape.
pub fn format_agent_answer(a: &AgentAnswer) -> String {
    let json = serde_json::json!({ "scope": a.scope.as_str(), "reason": a.reason });
    format!("{ANSWER_MARKER} {json}")
}

/// Reads the agent's final message. Tolerant: the marker may be anywhere and the JSON may be fenced; a bad answer is
/// `None`.
pub fn parse_agent_answer(text: &str) -> Option<AgentAnswer> {
    let at = text.rfind(ANSWER_MARKER)?;
    let rest: Vec<char> = text[at + ANSWER_MARKER.len()..].chars().collect();
    let start = rest.iter().position(|&c| c == '{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = start;
    while i < rest.len() {
        let c = rest[i];
        if in_str {
            if c == '\\' {
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                let json: String = rest[start..=i].iter().collect();
                let v: serde_json::Value = serde_json::from_str(&json).ok()?;
                let scope = v.get("scope")?.as_str()?.parse::<EnvironmentScope>().ok()?;
                let reason = v
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .map(|r| js_trim(r).to_owned())
                    .unwrap_or_default();
                return Some(AgentAnswer { scope, reason });
            }
        }
        i += 1;
    }
    None
}

/// The canonical shape of `main.tf`, shown to the agent (and used by the mock drafter).
pub const MAIN_TF_SKELETON: &str = r#"module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "the service in the repo's container"
      instance_type = "c7i.2xlarge"
      user_data     = file("${path.module}/scripts/api.sh")
    }
    load = {
      role          = "load"
      label         = "Load generator"
      detail        = "k6, on its own host"
      instance_type = "c7i.large"
      user_data     = file("${path.module}/scripts/load.sh")
    }
  }
}
"#;

#[derive(Debug, Clone, Default)]
pub struct SetupBriefInput {
    /// What the user wants optimized, in their words.
    pub goal: String,
    /// The bench machine the repo's ratchet.yaml / template pins, when known (default for `single`).
    pub hardware: Option<String>,
    /// The run's time limit in seconds, so the agent can weigh cost.
    pub time_limit_seconds: Option<f64>,
}

const SETUP_TEMPLATE: &str = r#"You are the setup agent for Ratchet, a tool that lets coding agents optimize a metric of this repository and keeps only changes a trusted evaluator confirms. Your job is to design the ENVIRONMENT the optimization is measured in, before any agent starts. You do not optimize anything and you do not change the code under test.

## The goal
@GOAL@

## Read first
Read the repository (README, build and test files, the service entry points, ratchet.yaml, RATCHET.md, the evaluator under bench/ or eval/) so you know what has to run for the goal's metric to be measured honestly.

## Decide the scope, as small as the task allows
- `none`: pure code or tests. Nothing needs to be deployed; the evaluator's container is enough. Write ONLY the `environment.scope: none` line.
- `single`: one machine is enough (a CLI, a build, one service that needs nothing else). Pin the hardware with `environment.machine`. You may add a Dockerfile under @DIR@/ and point `environment.image` at it.
- `full`: the metric depends on other components (a database with seeded data, a cache, a separate load generator, other services). Write OpenTofu under @DIR@/.
Choose `full` only when a single machine would not measure the metric; every machine costs money while it runs. The run is limited to @HOURS@.

## What you may write (anything else is rejected)
1. `ratchet.yaml`: add or change ONLY the `environment:` block (`scope`, and when needed `machine`, `image`, `region`). Do not touch objective, evaluator, guardrails, protected, agent or any egress hosts.
2. Files under `@DIR@/`: `main.tf`, scripts, a Dockerfile, seed data.
Do not edit source code, tests, the evaluator or lockfiles.

## The base module (the only module you may call)
Ratchet ships `@SOURCE@`. It brings the network isolation, the evaluation-only identity, the sealed results and the budget tags; you do not create networks, security groups, IAM, buckets or public addresses, and you do not declare providers, backends or variables named `ratchet`. You describe machines as a LITERAL map; each entry may only have: @KEYS@.
- `role`: @ROLES@
- `instance_type`: a literal. Fixed-performance types only for app, load and evaluator (no burstable t-types). Known types and prices: @TYPES@.
- `user_data`: a script, from a file in the draft: `file("${path.module}/scripts/x.sh")`.
- At most @MAXM@ machines. Allowed extra resource: `aws_s3_object` for seed data, `bucket = module.sandbox.artifacts_bucket`, key under `@PREFIX@`, `tags = module.sandbox.tags`.

`main.tf` looks like this:

```hcl
@SKELETON@```

## Hard rules
- The measuring machines have NO internet. Scripts must not curl, wget, pip/npm/apt install, docker pull or git clone: everything they need comes from the repo's container image or the seed data you ship.
- Hardware is pinned: the same instance types for the baseline and every candidate.
- Everything must be destroyable: no prevent_destroy, no deletion protection, no provisioners, no import blocks.
- Never put secrets, keys or real customer data in a file; seed data is synthetic or anonymised.
- Keep it minimal and cheap. Prefer one fewer machine to one more.

## When you are done
End your final message with one line, exactly:
@MARKER@ {"scope":"none|single|full","reason":"one or two plain sentences on why this much infrastructure"}
"#;

/// The first message to the setup agent.
pub fn build_setup_brief(input: &SetupBriefInput) -> String {
    let types = INSTANCE_USD_PER_HOUR
        .iter()
        .map(|(t, _)| *t)
        .collect::<Vec<_>>()
        .join(", ");
    let hours = match input.time_limit_seconds {
        Some(s) if s != 0.0 && !s.is_nan() => format!("{} h", to_fixed(s / 3600.0, 1)),
        _ => "2 h".to_owned(),
    };
    let roles = EnvironmentRole::ALL
        .iter()
        .map(|r| r.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    fill(
        SETUP_TEMPLATE,
        &[
            ("GOAL", js_trim(&input.goal)),
            ("DIR", DEFAULT_ENVIRONMENT_INFRA_DIR),
            ("HOURS", &hours),
            ("SOURCE", SANDBOX_BASE_MODULE_SOURCE),
            ("KEYS", &MACHINE_KEYS.join(", ")),
            ("ROLES", &roles),
            ("TYPES", &types),
            ("MAXM", &MAX_ENVIRONMENT_MACHINES.to_string()),
            ("PREFIX", ENVIRONMENT_OBJECT_PREFIX),
            ("SKELETON", MAIN_TF_SKELETON),
            ("MARKER", ANSWER_MARKER),
        ],
    )
}

/// Single-pass `@KEY@` substitution (substituted text is never scanned again).
fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 2048);
    let mut rest = template;
    while let Some(i) = rest.find('@') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('@').and_then(|j| {
            vars.iter()
                .find(|(k, _)| *k == &after[..j])
                .map(|v| (j, v.1))
        }) {
            Some((j, val)) => {
                out.push_str(val);
                rest = &after[j + 1..];
            }
            None => {
                out.push('@');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The follow-up message for the revise loop.
pub fn build_revise_brief(message: &str) -> String {
    format!(
        "The user asked for a change to the environment you drafted:\n\n\"{}\"\n\nUpdate the files in place, keeping every rule from the first brief (you may change only the environment: block of ratchet.yaml and files under {}/). If the request cannot be done inside the base module (for example a managed database, which the isolated network cannot host), keep the closest thing that works and say so in your reason. End your final message with the {} line again.",
        js_trim(message),
        DEFAULT_ENVIRONMENT_INFRA_DIR,
        ANSWER_MARKER
    )
}

#[allow(dead_code)]
const _BASE_NAME: &str = SANDBOX_BASE_MODULE_NAME;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_yaml::{EnvironmentConfigPatch, apply_environment_to_config_yaml};
    use crate::types::EnvironmentFile;
    use crate::validate::{ValidateEnvironmentInput, validate_environment};

    fn answer(scope: EnvironmentScope, reason: &str) -> AgentAnswer {
        AgentAnswer {
            scope,
            reason: reason.into(),
        }
    }

    // "round-trips and is found anywhere in the final message"
    #[test]
    fn answer_round_trips_and_is_found_anywhere() {
        let a = answer(
            EnvironmentScope::Full,
            "p95 depends on database queries, so a single machine would not measure it.",
        );
        assert_eq!(
            parse_agent_answer(&format_agent_answer(&a)),
            Some(a.clone())
        );
        assert_eq!(
            parse_agent_answer(&format!(
                "Done! I wrote the files.\n\n{}\nBye",
                format_agent_answer(&a)
            )),
            Some(a)
        );
        assert_eq!(
            parse_agent_answer(&format!(
                "{ANSWER_MARKER}\n```json\n{{\"scope\":\"none\",\"reason\":\"x\"}}\n```"
            )),
            Some(answer(EnvironmentScope::None, "x"))
        );
    }

    // "the last marker wins and braces inside the reason do not confuse it"
    #[test]
    fn last_marker_wins_and_braces_in_the_reason_are_fine() {
        let text = format!(
            "{}\n{ANSWER_MARKER} {{\"scope\":\"full\",\"reason\":\"a }} brace and a \\\"quote\\\"\"}}",
            format_agent_answer(&answer(EnvironmentScope::Single, "first"))
        );
        assert_eq!(
            parse_agent_answer(&text),
            Some(answer(EnvironmentScope::Full, "a } brace and a \"quote\""))
        );
    }

    // "is null for a missing marker, bad JSON or an unknown scope"
    #[test]
    fn bad_answers_are_none() {
        assert_eq!(parse_agent_answer("no answer here"), None);
        assert_eq!(
            parse_agent_answer(&format!("{ANSWER_MARKER} {{not json}}")),
            None
        );
        assert_eq!(
            parse_agent_answer(&format!(
                "{ANSWER_MARKER} {{\"scope\":\"huge\",\"reason\":\"x\"}}"
            )),
            None
        );
        assert_eq!(
            parse_agent_answer(&format!("{ANSWER_MARKER} {{\"scope\":\"full\"")),
            None
        );
        assert_eq!(
            parse_agent_answer(&format!("{ANSWER_MARKER} {{\"scope\":\"full\"}}")),
            Some(answer(EnvironmentScope::Full, ""))
        );
    }

    fn brief() -> String {
        build_setup_brief(&SetupBriefInput {
            goal: "Make checkout p95 faster".into(),
            hardware: Some("c7i.2xlarge".into()),
            time_limit_seconds: Some(7200.0),
        })
    }

    // "carries the goal, the three scopes, the rules the validator enforces and the answer line"
    #[test]
    fn brief_carries_goal_scopes_rules_and_answer_line() {
        let b = brief();
        assert!(b.contains("Make checkout p95 faster"));
        for s in ["`none`", "`single`", "`full`"] {
            assert!(b.contains(s), "{s}");
        }
        for s in [
            "infra/ratchet/",
            "./ratchet-sandbox",
            "NO internet",
            "2.0 h",
            ANSWER_MARKER,
            "Do not edit source code",
        ] {
            assert!(b.contains(s), "{s}");
        }
    }

    // "lists the allowed machine keys and the priced instance types"
    #[test]
    fn brief_lists_machine_keys_and_instance_types() {
        let b = brief();
        assert!(b.contains(
            "role, instance_type, label, detail, user_data, root_volume_gb, data_volume_gb"
        ));
        assert!(b.contains("c7i.2xlarge"));
        assert!(b.contains("r7g.large"));
    }

    // "the skeleton it shows is itself a draft that passes the validator"
    #[test]
    fn the_skeleton_passes_the_validator() {
        let cfg = "version: 0\nobjective:\n  metric: m\n  direction: minimize\nevaluator:\n  template: http-latency\n";
        let mut input = ValidateEnvironmentInput::new(
            EnvironmentScope::Full,
            vec![
                EnvironmentFile::new(
                    "ratchet.yaml",
                    apply_environment_to_config_yaml(
                        cfg,
                        &EnvironmentConfigPatch::scope(EnvironmentScope::Full),
                    )
                    .unwrap(),
                ),
                EnvironmentFile::new("infra/ratchet/main.tf", MAIN_TF_SKELETON),
                EnvironmentFile::new("infra/ratchet/scripts/api.sh", "#!/bin/bash\n"),
                EnvironmentFile::new("infra/ratchet/scripts/load.sh", "#!/bin/bash\n"),
            ],
        );
        input.original_config_yaml = Some(cfg.into());
        assert_eq!(validate_environment(&input).problems, Vec::<String>::new());
    }

    // "the revise brief repeats the change and the answer line"
    #[test]
    fn revise_brief_repeats_the_change_and_the_answer_line() {
        let b = build_revise_brief("use RDS instead of a Postgres container");
        assert!(b.contains("use RDS instead of a Postgres container"));
        assert!(b.contains(ANSWER_MARKER));
    }
}
