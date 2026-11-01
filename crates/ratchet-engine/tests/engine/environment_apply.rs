//! `packages/engine/src/environment/apply.test.ts`: the approved environment is applied as the sandbox of the run. The engine
//! reads it from the PROTECTED ref (never a candidate's tree), validates it against the sandbox contract, hands it to the
//! sandbox provider with the sandbox input, and the same sandbox is destroyed (with the receipt) after the run.

use ratchet_core::config::DEFAULT_ENVIRONMENT_INFRA_DIR as D;
use ratchet_core::enums::{EnvironmentScope, Mode};
use ratchet_core::ports::{GitHost as _, SandboxEnvironmentKind, SandboxInput, SandboxStateStatus};
use ratchet_engine::{EngineErrorCode as C, EngineSettings, SandboxSettings};
use ratchet_env::{
    EnvironmentConfigPatch, SampleOptions, apply_environment_to_config_yaml,
    environment_sandbox_id, sample_environment,
};

use crate::common::*;

const MIN: i64 = 60_000;

/// A harness whose protected ref already holds an approved environment of `scope` (what approving a draft commits).
async fn with_environment(
    scope: EnvironmentScope,
    skip_files: bool,
    tweak: Option<&dyn Fn(&str, &str) -> String>,
) -> Harness {
    let h = Harness::new(HarnessOptions {
        mode: Some(Mode::Live),
        settings: EngineSettings {
            sandbox: SandboxSettings {
                auto: Some(true),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let patch = if scope == EnvironmentScope::Single {
        EnvironmentConfigPatch {
            scope,
            machine: Some("c7i.xlarge".into()),
            image: Some(format!("{D}/Dockerfile")),
            region: None,
        }
    } else {
        EnvironmentConfigPatch::scope(scope)
    };
    {
        let mut files = h.git.files.lock().unwrap();
        files.insert(
            "ratchet.yaml".into(),
            apply_environment_to_config_yaml(CONFIG_YAML, &patch).unwrap(),
        );
        if !skip_files {
            for f in sample_environment(scope, &SampleOptions::default()).files {
                if f.path == "ratchet.yaml" {
                    continue;
                }
                let content = match tweak {
                    Some(t) => t(&f.path, &f.content),
                    None => f.content,
                };
                files.insert(f.path, content);
            }
        }
    }
    h.bench.set(
        "main-sha",
        "baseline",
        ScriptedBench::samples(&[100.0, 102.0, 98.0]).with_metric("peak_rss", 500.0),
    );
    h.bench.set(
        "main-sha",
        "confirm",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    h
}

async fn create(h: &Harness, id: &str) {
    h.engine
        .create_optimization(CreateOptimizationInput {
            id: Some(id.into()),
            repo_url: h.git.remote_url(),
            base_ref: "main".into(),
            agents: vec![Codex],
            budget_usd: 100.0,
            time_limit_seconds: 8 * 3600,
            plateau_evals: 50,
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn start(h: &Harness, id: &str) -> SandboxInput {
    create(h, id).await;
    h.engine.start(id).await.unwrap();
    h.sandbox
        .applies
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("an apply")
}

fn applies(h: &Harness) -> Vec<String> {
    h.log.starting_with("sandbox.apply")
}

// "the engine applies the approved environment as the sandbox: full: the module files from the protected ref go into the
// sandbox input, hashed, tagged, in a sandbox of their own"
#[tokio::test]
async fn full_environment_files_go_into_the_sandbox_input_hashed_and_tagged() {
    let h = with_environment(EnvironmentScope::Full, false, None).await;
    let input = start(&h, "run-a").await;
    let env = input.environment.clone().expect("environment");
    assert_eq!(env.kind, SandboxEnvironmentKind::Drafted);
    assert_eq!(env.scope, EnvironmentScope::Full);
    assert_eq!(env.path, D);
    assert_eq!(env.commit, "main-sha");
    let paths: Vec<_> = env.files.iter().map(|f| f.path.as_str()).collect();
    for want in [
        "main.tf",
        "seed.sh",
        "seed/orders.sql",
        "scripts/api.sh",
        "scripts/load.sh",
    ] {
        assert!(paths.contains(&want), "{want} in {paths:?}");
    }
    assert!(
        env.hash.len() == 10 && env.hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "{}",
        env.hash
    );
    assert_eq!(
        input.sandbox_id,
        environment_sandbox_id("us-east-1", &env.hash)
    );
    let tags = input.tags.clone().unwrap();
    assert_eq!(
        tags.get("ratchet:managed").map(String::as_str),
        Some("true")
    );
    assert_eq!(tags.get("ratchet:env"), Some(&env.hash));
    let opt = h
        .ledger
        .get_optimization(&oid("run-a"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        opt.sandbox_id.as_ref().map(|s| s.to_string()),
        Some(input.sandbox_id.clone())
    );
    assert_eq!(opt.status, OptimizationStatus::Running);
}

// "none: the standard sandbox, nothing extra"
#[tokio::test]
async fn none_uses_the_standard_sandbox() {
    let h = with_environment(EnvironmentScope::None, false, None).await;
    let input = start(&h, "run-a").await;
    assert!(input.environment.is_none());
    assert_eq!(input.sandbox_id, "ratchet-sandbox");
}

// "single without a drafted module: the standard sandbox (the pinned machine comes from ratchet.yaml)"
#[tokio::test]
async fn single_without_a_drafted_module_uses_the_standard_sandbox() {
    let h = with_environment(EnvironmentScope::Single, true, None).await;
    assert!(start(&h, "run-a").await.environment.is_none());
}

// "is deterministic and shared: the same environment in the same region reuses one sandbox; a changed one gets its own"
#[tokio::test]
async fn the_same_environment_shares_a_sandbox_and_a_changed_one_gets_its_own() {
    let h = with_environment(EnvironmentScope::Full, false, None).await;
    let a = start(&h, "run-a").await;
    create(&h, "run-b").await;
    h.engine.start("run-b").await.unwrap();
    let b = h
        .ledger
        .get_optimization(&oid("run-b"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        b.sandbox_id.as_ref().map(|s| s.to_string()),
        Some(a.sandbox_id.clone())
    );
    assert_eq!(applies(&h).len(), 1);

    let tweak = |p: &str, c: &str| {
        if p.ends_with("scripts/redis.sh") {
            format!("{c}# tuned\n")
        } else {
            c.to_owned()
        }
    };
    let h2 = with_environment(EnvironmentScope::Full, false, Some(&tweak)).await;
    let changed = start(&h2, "run-a").await;
    assert_ne!(
        changed.environment.unwrap().hash,
        a.environment.unwrap().hash
    );
    assert_ne!(changed.sandbox_id, a.sandbox_id);
}

// "reads the files at the commit the optimization was created on, not wherever the branch is later"
#[tokio::test]
async fn reads_the_files_at_the_commit_the_optimization_was_created_on() {
    let h = with_environment(EnvironmentScope::Full, false, None).await;
    h.git
        .resolved
        .lock()
        .unwrap()
        .insert("main".into(), "sha-at-create".into());
    h.bench.set(
        "sha-at-create",
        "baseline",
        ScriptedBench::samples(&[100.0, 102.0, 98.0]).with_metric("peak_rss", 500.0),
    );
    h.bench.set(
        "sha-at-create",
        "confirm",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    let input = start(&h, "run-a").await;
    assert_eq!(input.environment.unwrap().commit, "sha-at-create");
}

// "refuses to start when the config asks for an environment the protected ref does not hold"
#[tokio::test]
async fn refuses_to_start_without_the_environment_files() {
    let h = with_environment(EnvironmentScope::Full, true, None).await;
    create(&h, "run-a").await;
    let err = h.engine.start("run-a").await.unwrap_err();
    assert_eq!(err.code, C::InvalidConfig);
    assert!(
        err.message.contains("infra/ratchet") && err.message.contains("does not exist")
            || err.message.contains("Approve the environment"),
        "{}",
        err.message
    );
    assert!(applies(&h).is_empty());
}

// "refuses %s before anything is created" (3 cases)
#[tokio::test]
async fn refuses_an_environment_that_breaks_the_sandbox_contract_before_anything_is_created() {
    type Tweak = fn(&str, &str) -> String;
    let cases: [(&str, Tweak); 3] = [
        ("a security group opening the internet", |p, c| {
            if p == "main.tf" {
                format!(
                    "{c}\nresource \"aws_security_group\" \"open\" {{\n  egress {{\n    cidr_blocks = [\"0.0.0.0/0\"]\n  }}\n}}\n"
                )
            } else {
                c.to_owned()
            }
        }),
        ("an unpinned provider block of its own", |p, c| {
            if p == "main.tf" {
                format!("{c}\nprovider \"aws\" {{\n  region = \"eu-west-1\"\n}}\n")
            } else {
                c.to_owned()
            }
        }),
        ("a module that does not use the base module", |p, c| {
            if p == "main.tf" {
                c.replace("./ratchet-sandbox", "github.com/evil/module")
            } else {
                c.to_owned()
            }
        }),
    ];
    for (name, tweak) in cases {
        let rel = |p: &str, c: &str| tweak(&p[D.len() + 1..], c);
        let h = with_environment(EnvironmentScope::Full, false, Some(&rel)).await;
        create(&h, "run-a").await;
        let err = h.engine.start("run-a").await.unwrap_err();
        assert_eq!(err.code, C::InvalidConfig, "{name}");
        assert!(
            err.message.contains("sandbox contract"),
            "{name}: {}",
            err.message
        );
        assert!(applies(&h).is_empty(), "{name}");
    }
}

// "the environment is torn down after the run: the sandbox that held it is destroyed after the idle window and the
// receipt says so"
#[tokio::test]
async fn the_sandbox_that_held_the_environment_is_destroyed_after_the_idle_window() {
    let h = with_environment(EnvironmentScope::Full, false, None).await;
    let input = start(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    h.clock.advance(31 * MIN);
    h.engine.tick().await.unwrap();
    h.engine.settle().await;
    assert_eq!(
        h.log.starting_with("sandbox.destroy"),
        [format!("sandbox.destroy:{}", input.sandbox_id)]
    );
    assert!(h.sandbox.applied.lock().unwrap().is_empty());
    assert_eq!(
        h.engine
            .sandbox_state(&input.sandbox_id)
            .await
            .unwrap()
            .status,
        SandboxStateStatus::Destroyed
    );
    let receipt = h
        .engine
        .teardown_receipt("run-a")
        .await
        .unwrap()
        .expect("a receipt");
    assert_eq!(receipt.sandbox_id, input.sandbox_id);
}

// "the approved environment is locked: candidates cannot change it" (4 cases)
#[tokio::test]
async fn a_candidate_that_touches_the_locked_environment_is_blocked_without_a_bench_run() {
    for path in [
        "infra/ratchet/main.tf",
        "infra/ratchet/scripts/load.sh",
        "infra/ratchet/seed/orders.sql",
        "ratchet.yaml",
    ] {
        let h = with_environment(EnvironmentScope::Full, false, None).await;
        create(&h, "run-a").await;
        h.engine.start("run-a").await.unwrap();
        let branch = "ratchet/run-a/codex";
        h.candidate(branch, "sha-env", Cand::new().files(&["src/a.ts", path]));
        h.candidate(
            branch,
            "sha-ok",
            Cand::new()
                .files(&["src/a.ts"])
                .bench(&[80.0, 80.0, 80.0])
                .confirm(&[80.0, 80.0, 80.0]),
        );
        h.until(async || all_decided(&h, "run-a", 2).await, 60)
            .await;
        let rows = candidates(&h, "run-a").await;
        let by = |sha: &str| {
            rows.iter()
                .find(|c| c.candidate.sha == sha)
                .unwrap()
                .clone()
        };
        let blocked = by("sha-env");
        assert_eq!(blocked.candidate.verdict.to_string(), "blocked", "{path}");
        assert!(
            blocked.candidate.reasons[0].contains(&format!("protected path changed: {path}")),
            "{path}: {:?}",
            blocked.candidate.reasons
        );
        assert!(blocked.results.is_empty(), "{path}");
        assert_eq!(
            by("sha-ok").candidate.verdict.to_string(),
            "verified",
            "{path}"
        ); // a code-only change next to it is untouched
    }
}

// "the protected set of the optimization lists the environment directory"
#[tokio::test]
async fn the_protected_set_lists_the_environment_directory() {
    let h = with_environment(EnvironmentScope::Full, false, None).await;
    create(&h, "run-a").await;
    let glob = format!("{D}/**");
    assert!(
        h.engine
            .protected_paths("run-a")
            .await
            .unwrap()
            .globs
            .contains(&glob)
    );
    // the decision is locked whatever the scope: even `none` cannot be turned into a bigger environment by a candidate
    let none = with_environment(EnvironmentScope::None, false, None).await;
    create(&none, "run-a").await;
    assert!(
        none.engine
            .protected_paths("run-a")
            .await
            .unwrap()
            .globs
            .contains(&glob)
    );
    // no environment decided yet (no scope in ratchet.yaml): nothing extra is protected
    let undecided = Harness::new(HarnessOptions {
        mode: Some(Mode::Live),
        settings: EngineSettings {
            sandbox: SandboxSettings {
                auto: Some(true),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    create(&undecided, "run-a").await;
    assert!(
        !undecided
            .engine
            .protected_paths("run-a")
            .await
            .unwrap()
            .globs
            .contains(&glob)
    );
}
