//! `packages/runtime/src/live.test.ts`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{DetectOptions, HarnessAuth, HarnessInfo, HarnessKind, Platform};
use ratchet_core::enums::{AgentKind, AgentsWhere, Mode, SandboxProviderKind, SandboxStatus};
use ratchet_core::ports::{SandboxInput, SandboxOutputs, SandboxStateStatus};
use ratchet_engine::runtime::{
    HybridOptions, LiveOptions, LiveRuntime, create_hybrid_runtime, create_live_runtime,
    create_live_runtime_for, resolve_live_images,
};
use ratchet_engine::{CreateOptimizationInput, EngineErrorCode};
use ratchet_ledger::{Ledger, LedgerOptions};
use ratchet_sandbox::MockImageLookup;
use ratchet_sandbox::images::RawImage;
use ratchet_sandbox::sandbox::{ProcessResult, ProcessRunner, ProcessSpec};

const ACCT: &str = "123456789012";
const TOKEN: &str = "ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE12";

fn identity(n: &str) -> serde_json::Value {
    serde_json::json!({
        "role_arn": format!("arn:aws:iam::{ACCT}:role/ratchet/bench/{n}"),
        "role_name": n,
        "instance_profile_arn": format!("arn:aws:iam::{ACCT}:instance-profile/ratchet/bench/{n}"),
        "instance_profile_name": n,
    })
}

fn outputs_json() -> String {
    serde_json::json!({
        "artifacts_bucket": { "value": "ratchet-sbx-artifacts" },
        "sealed_bucket": { "value": "ratchet-sbx-sealed" },
        "agent_boundary_ref": { "value": format!("arn:aws:iam::{ACCT}:policy/ratchet/boundary") },
        "eval_identity_ref": { "value": identity("bench-runner") },
        "confirm_identity_ref": { "value": identity("confirm-runner") },
        "sealed_confirm_prefix": { "value": "holdout/" },
        "network_ref": { "value": { "vpc_id": "vpc-1", "subnet_id": "subnet-1", "bench_subnet_id": "subnet-bench", "security_group_ids": { "agent_runner": "sg-r", "bench": "sg-b" }, "egress_proxy_url": null } },
        "agent_role_path": { "value": "/ratchet/agent/" },
        "limits": { "value": { "max_instances_per_agent": 2, "max_instance_lifetime_hours": 4 } },
    })
    .to_string()
}

fn plan_json() -> String {
    serde_json::json!({
        "resource_changes": [
            { "address": "aws_s3_bucket.artifacts", "mode": "managed", "type": "aws_s3_bucket", "change": { "actions": ["create"] } },
            { "address": "aws_vpc.sandbox", "mode": "managed", "type": "aws_vpc", "change": { "actions": ["create"] } },
        ]
    })
    .to_string()
}

#[derive(Default)]
struct FakeTofu {
    calls: Mutex<Vec<ProcessSpec>>,
}

#[async_trait]
impl ProcessRunner for FakeTofu {
    async fn run(&self, spec: &ProcessSpec) -> ratchet_sandbox::Result<ProcessResult> {
        self.calls.lock().unwrap().push(spec.clone());
        let (exit_code, stdout) = match spec.args[0].as_str() {
            "plan" => (2, String::new()),
            "show" => (0, plan_json()),
            "output" => (0, outputs_json()),
            _ => (0, String::new()),
        };
        Ok(ProcessResult {
            exit_code,
            stdout,
            stderr: String::new(),
        })
    }
}

impl FakeTofu {
    fn subs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.args[0].clone())
            .collect()
    }
}

fn input() -> SandboxInput {
    SandboxInput {
        sandbox_id: "sbx".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.xlarge".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 4.0,
        tags: None,
        environment: None,
    }
}

async fn ledger() -> Ledger {
    Ledger::open_memory(LedgerOptions::default()).await.unwrap()
}

fn live_opts(ledger: &Ledger, state_root: &std::path::Path, tofu: &Arc<FakeTofu>) -> LiveOptions {
    let mut o = LiveOptions::new(
        ledger.clone(),
        "us-east-1",
        "sbx",
        "/opt/tofu",
        "/repo/infra/aws",
        state_root,
    );
    o.tofu_runner = Some(tofu.clone());
    o.aws_profile = Some("sandbox".into());
    o
}

struct Built {
    rt: LiveRuntime,
    tofu: Arc<FakeTofu>,
    _root: tempfile::TempDir,
}

async fn build(ledger: &Ledger, over: impl FnOnce(&mut LiveOptions)) -> Built {
    let root = tempfile::tempdir().unwrap();
    let tofu = Arc::new(FakeTofu::default());
    let mut o = live_opts(ledger, root.path(), &tofu);
    over(&mut o);
    let rt = create_live_runtime(o).await.unwrap();
    Built {
        rt,
        tofu,
        _root: root,
    }
}

fn draft() -> CreateOptimizationInput {
    CreateOptimizationInput {
        repo_url: "x".into(),
        base_ref: "main".into(),
        agents: vec![AgentKind::Codex],
        budget_usd: 1.0,
        time_limit_seconds: 60,
        plateau_evals: 1,
        ..Default::default()
    }
}

fn joined(missing: &[String]) -> String {
    missing.join(" ")
}

#[tokio::test]
async fn before_the_first_apply_sandbox_plan_apply_destroy_work_everything_else_says_what_is_missing()
 {
    let ledger = ledger().await;
    let b = build(&ledger, |_| {}).await;
    let rt = &b.rt;
    assert_eq!(rt.engine.mode(), Mode::Live);
    assert_eq!(rt.repo_url, None);
    for want in ["apply the sandbox", "GitHub", "runner AMI", "bench AMI"] {
        assert!(
            rt.missing.iter().any(|m| m.contains(want)),
            "{want} in {:?}",
            rt.missing
        );
    }

    let plan = rt.engine.plan_sandbox(&input()).await.unwrap();
    assert_eq!(
        plan.changes
            .iter()
            .map(|c| c.address.as_str())
            .collect::<Vec<_>>(),
        ["aws_s3_bucket.artifacts", "aws_vpc.sandbox"]
    );
    assert_eq!(plan.summary, "Plan: 2 to add, 0 to change, 0 to destroy.");
    let row = ledger.get_sandbox("sbx").await.unwrap().unwrap();
    assert_eq!(
        (row.status, row.provider),
        (SandboxStatus::Planned, SandboxProviderKind::Aws)
    );

    // tofu runs with the chosen profile
    assert!(
        b.tofu
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|c| c.env.get("AWS_PROFILE").map(String::as_str) == Some("sandbox"))
    );

    // no optimization can start without an applied sandbox
    let mut with_sbx = draft();
    with_sbx.sandbox_id = Some("sbx".into());
    assert_eq!(
        rt.engine
            .create_optimization(with_sbx.clone())
            .await
            .unwrap_err()
            .code,
        EngineErrorCode::SandboxNotApplied
    );

    let out = rt.engine.apply_sandbox(&input()).await.unwrap();
    assert_eq!(out.artifacts_bucket, "ratchet-sbx-artifacts");
    assert!(b.tofu.subs().contains(&"apply".to_owned()));
    let row = ledger.get_sandbox("sbx").await.unwrap().unwrap();
    assert_eq!(
        (row.status, row.account_id.as_deref()),
        (SandboxStatus::Applied, Some(ACCT))
    );
    let stored: SandboxOutputs = serde_json::from_value(row.outputs.clone().unwrap()).unwrap();
    assert_eq!(stored.network.bench_subnet_id, "subnet-bench");

    // applied, but GitHub is not configured: a clear error from the unready port, nothing is created
    let err = rt.engine.create_optimization(with_sbx).await.unwrap_err();
    assert_eq!(err.code, EngineErrorCode::NotConfigured);
    assert!(
        err.message
            .contains("GitHub is not ready: set the GitHub repository and token"),
        "{}",
        err.message
    );
    assert!(ledger.list_optimizations(None).await.unwrap().is_empty());

    // destroy: the compute port does not exist yet (this runtime was built before the apply), which means no orphans
    rt.engine.destroy_sandbox("sbx").await.unwrap();
    assert!(b.tofu.subs().contains(&"destroy".to_owned()));
    let row = ledger.get_sandbox("sbx").await.unwrap().unwrap();
    assert_eq!(row.status, SandboxStatus::Destroyed);
    assert!(row.outputs.is_none());
    rt.dispose().await;
}

#[tokio::test]
async fn rebuilt_from_the_ledger_outputs_and_a_full_configuration_nothing_is_missing() {
    let ledger = ledger().await;
    let first = build(&ledger, |_| {}).await;
    first.rt.engine.apply_sandbox(&input()).await.unwrap();
    let outputs: SandboxOutputs = serde_json::from_value(
        ledger
            .get_sandbox("sbx")
            .await
            .unwrap()
            .unwrap()
            .outputs
            .unwrap(),
    )
    .unwrap();
    first.rt.dispose().await;

    let b = build(&ledger, |o| {
        o.sandbox = Some(outputs);
        o.github_repo = Some("acme/api".into());
        o.github_token = Some(TOKEN.into());
        o.runner_image_ref = Some("ami-0123456789abcdef0".into());
        o.bench_image_ref = Some("ami-0fedcba9876543210".into());
        o.secrets = Some(Arc::new(|_, name| Some(format!("value-of-{name}"))));
    })
    .await;
    assert!(b.rt.missing.is_empty(), "{:?}", b.rt.missing);
    assert_eq!(
        b.rt.repo_url.as_deref(),
        Some("https://github.com/acme/api.git")
    );
    assert_eq!(
        b.rt.engine.sandbox_state("sbx").await.unwrap().status,
        SandboxStateStatus::Applied
    );
    b.rt.dispose().await;
}

#[tokio::test]
async fn reports_only_what_is_still_missing() {
    let ledger = ledger().await;
    let b = build(&ledger, |o| {
        o.github_repo = Some("acme/api".into());
        o.github_token = Some(TOKEN.into());
        o.runner_image_ref = Some("ami-0123456789abcdef0".into());
    })
    .await;
    assert_eq!(b.rt.missing.len(), 2);
    assert!(joined(&b.rt.missing).contains("apply the sandbox"));
    assert!(joined(&b.rt.missing).contains("bench AMI"));
    assert_eq!(
        b.rt.repo_url.as_deref(),
        Some("https://github.com/acme/api.git")
    );
    b.rt.dispose().await;
}

// ---------------------------------------------------------------------------------------- createHybridRuntime

fn ok(kind: HarnessKind, auth: HarnessAuth) -> HarnessInfo {
    HarnessInfo {
        kind,
        installed: true,
        version: Some("1.0.0".into()),
        path: Some(format!(
            "/usr/local/bin/{}",
            if kind == HarnessKind::Codex {
                "codex"
            } else {
                "claude"
            }
        )),
        auth,
        hint: String::new(),
    }
}

fn signed_in(kind: HarnessKind) -> HarnessInfo {
    ok(kind, HarnessAuth::SignedIn)
}

async fn hybrid(
    ledger: &Ledger,
    over: impl FnOnce(&mut HybridOptions),
) -> (LiveRuntime, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let tofu = Arc::new(FakeTofu::default());
    let mut o = HybridOptions::new(live_opts(ledger, root.path(), &tofu));
    o.harnesses = Some(vec![
        signed_in(HarnessKind::ClaudeCode),
        signed_in(HarnessKind::Codex),
    ]);
    o.login_path = Some("/usr/local/bin:/usr/bin".into());
    over(&mut o);
    (create_hybrid_runtime(o).await.unwrap(), root)
}

#[tokio::test]
async fn defaults_to_local_agents_and_an_invisible_sandbox_no_apply_step_no_runner_ami_needed() {
    let (rt, _r) = hybrid(&ledger().await, |_| {}).await;
    assert_eq!(rt.agents_where, AgentsWhere::Local);
    assert!(rt.local_runners.is_some());
    assert_eq!(
        rt.harnesses.iter().map(|h| h.kind).collect::<Vec<_>>(),
        [HarnessKind::ClaudeCode, HarnessKind::Codex]
    );
    let m = joined(&rt.missing);
    assert!(
        !m.contains("apply the sandbox") && !m.contains("runner AMI"),
        "{m}"
    );
    assert_eq!(rt.missing.len(), 2);
    assert!(
        rt.missing[0].contains("GitHub") && rt.missing[1].contains("bench AMI"),
        "{:?}",
        rt.missing
    );
    rt.dispose().await;
}

#[tokio::test]
async fn is_ready_once_github_and_the_bench_image_are_set() {
    let (rt, _r) = hybrid(&ledger().await, |o| {
        o.live.github_repo = Some("acme/api".into());
        o.live.github_token = Some(TOKEN.into());
        o.live.bench_image_ref = Some("ami-0fedcba9876543210".into());
    })
    .await;
    assert!(rt.missing.is_empty(), "{:?}", rt.missing);
    assert_eq!(
        rt.repo_url.as_deref(),
        Some("https://github.com/acme/api.git")
    );
    rt.dispose().await;
}

#[tokio::test]
async fn without_an_applied_sandbox_a_draft_can_be_created_the_sandbox_is_made_at_the_first_start_so_only_github_is_missing()
 {
    let (rt, _r) = hybrid(&ledger().await, |_| {}).await;
    let err = rt.engine.create_optimization(draft()).await.unwrap_err();
    assert_eq!(err.code, EngineErrorCode::NotConfigured); // not `sandbox_not_applied`
    assert!(
        err.message.contains("GitHub is not ready"),
        "{}",
        err.message
    );
    rt.dispose().await;
}

#[tokio::test]
async fn runs_the_detection_probes_when_no_harnesses_are_given_and_says_what_is_missing_when_none_is_usable()
 {
    let root = tempfile::tempdir().unwrap();
    let spawner =
        FakeSpawner::with_plan(|_, _| FakeProcessSpec::exit(127).with_stderr("not found"));
    let env = |pairs: &[(&str, String)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    };
    let (rt, _r) = hybrid(&ledger().await, |o| {
        o.harnesses = None;
        o.login_path = None;
        o.detect = DetectOptions {
            spawner: Some(Arc::new(spawner)),
            env: Some(env(&[
                ("PATH", root.path().display().to_string()),
                ("HOME", root.path().display().to_string()),
                ("SHELL", "/bin/sh".into()),
            ])),
            home: Some(root.path().display().to_string()),
            platform: Some(Platform::Linux),
            timeout: Some(Duration::from_millis(200)),
            shell_timeout: Some(Duration::from_millis(200)),
            ..Default::default()
        };
    })
    .await;
    assert_eq!(
        rt.harnesses
            .iter()
            .map(|h| (h.kind, h.installed))
            .collect::<Vec<_>>(),
        [
            (HarnessKind::ClaudeCode, false),
            (HarnessKind::Codex, false),
            (HarnessKind::Grok, false)
        ]
    );
    assert!(
        joined(&rt.missing).contains("install and sign in to Claude Code, Codex or Grok"),
        "{:?}",
        rt.missing
    );
    rt.dispose().await;
}

#[tokio::test]
async fn a_signed_out_harness_does_not_count_as_usable_a_signed_in_one_does() {
    let (out, _r1) = hybrid(&ledger().await, |o| {
        o.harnesses = Some(vec![ok(HarnessKind::ClaudeCode, HarnessAuth::SignedOut)])
    })
    .await;
    assert!(joined(&out.missing).contains("sign in to Claude Code, Codex or Grok"));
    let (usable, _r2) = hybrid(&ledger().await, |o| {
        o.harnesses = Some(vec![
            ok(HarnessKind::ClaudeCode, HarnessAuth::SignedOut),
            ok(HarnessKind::Codex, HarnessAuth::ApiKey),
        ]);
    })
    .await;
    assert!(
        !joined(&usable.missing).contains("sign in to Claude Code"),
        "{:?}",
        usable.missing
    );
}

#[tokio::test]
async fn agents_where_cloud_keeps_the_ssm_path_the_runner_ami_is_required_again() {
    let (rt, _r) = hybrid(&ledger().await, |o| {
        o.live.agents_where = Some(AgentsWhere::Cloud)
    })
    .await;
    assert_eq!(rt.agents_where, AgentsWhere::Cloud);
    assert!(joined(&rt.missing).contains("runner AMI"));
}

#[tokio::test]
async fn create_live_runtime_without_local_agents_is_unchanged_cloud_no_local_control_and_refuses_agents_where_local_without_them()
 {
    let ledger = ledger().await;
    let b = build(&ledger, |_| {}).await;
    assert_eq!(b.rt.agents_where, AgentsWhere::Cloud);
    assert!(b.rt.local_runners.is_none());
    assert!(b.rt.harnesses.is_empty());
    let root = tempfile::tempdir().unwrap();
    let mut o = LiveOptions::new(ledger.clone(), "us-east-1", "sbx", "t", "m", root.path());
    o.agents_where = Some(AgentsWhere::Local);
    let err = create_live_runtime(o).await.err().expect("must refuse");
    assert!(err.message.contains("no localAgents"), "{}", err.message);
}

// ---------------------------------------------------------------------------------------- images by tag, region from the repo

fn tagged(id: &str, kind: &str, v: &str) -> RawImage {
    RawImage {
        id: Some(id.into()),
        name: None,
        state: Some("available".into()),
        architecture: Some("x86_64".into()),
        creation_date: Some("2026-10-20T10:00:00.000Z".into()),
        tags: vec![
            ("ratchet:image".into(), kind.into()),
            ("ratchet:image_version".into(), v.into()),
        ],
    }
}

fn base(ledger: &Ledger) -> LiveOptions {
    LiveOptions::new(
        ledger.clone(),
        "us-east-1",
        "sbx",
        "/opt/tofu",
        "/m",
        "/tmp/x",
    )
}

#[tokio::test]
async fn resolve_live_images_fills_both_amis_from_the_tags_for_the_app_version_an_explicit_ref_wins()
 {
    let ledger = ledger().await;
    let ec2 = MockImageLookup::new(vec![
        tagged("ami-0000000000000001", "runner", "0.4.0-1"),
        tagged("ami-0000000000000002", "bench", "0.4.0-1"),
    ]);
    let mut o = base(&ledger);
    o.app_version = Some("0.4.0".into());
    let r = resolve_live_images(o.clone(), Some(&ec2)).await.unwrap();
    assert_eq!(
        (r.runner_image_ref.as_deref(), r.bench_image_ref.as_deref()),
        (Some("ami-0000000000000001"), Some("ami-0000000000000002"))
    );
    o.bench_image_ref = Some("ami-0fedcba9876543210".into());
    let r = resolve_live_images(o, Some(&ec2)).await.unwrap();
    assert_eq!(
        (r.runner_image_ref.as_deref(), r.bench_image_ref.as_deref()),
        (Some("ami-0000000000000001"), Some("ami-0fedcba9876543210"))
    );
    let none = resolve_live_images(base(&ledger), Some(&ec2))
        .await
        .unwrap();
    assert!(none.runner_image_ref.is_none()); // no app version: nothing looked up
}

#[tokio::test]
async fn create_live_runtime_for_uses_environment_region_of_the_repo_then_the_setting_the_lookup_is_skipped_before_a_sandbox_exists()
 {
    let ledger = ledger().await;
    let root = tempfile::tempdir().unwrap();
    let ec2 = MockImageLookup::new(vec![]);
    let mut o = base(&ledger);
    o.state_root = root.path().to_path_buf();
    o.tofu_runner = Some(Arc::new(FakeTofu::default()));
    o.app_version = Some("0.4.0".into());
    o.region = "us-west-2".into();
    let rt = create_live_runtime_for(
        o.clone(),
        &serde_json::json!({ "environment": { "region": "eu-west-1" } }),
        Some(&ec2),
    )
    .await
    .unwrap();
    assert!(ec2.calls().is_empty());
    rt.dispose().await;
    let err = create_live_runtime_for(
        base(&ledger),
        &serde_json::json!({ "environment": { "region": "mars-1" } }),
        None,
    )
    .await
    .err()
    .expect("bad region");
    assert!(
        err.message.contains("not an AWS region name"),
        "{}",
        err.message
    );
}
