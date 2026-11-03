//! Port of `packages/aws/src/sandbox/tofu.test.ts`. The `tofu` subprocess is faked (the TS tests did the same); the real
//! process runner is exercised separately with `sh`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::ports::SandboxProvider;
use ratchet_core::ports::{PlanAction, PortError, SandboxInput};
use ratchet_sandbox::sandbox::tofu::{
    SharedRoles, map_actions, parse_tofu_outputs, summarize_plan,
};
use ratchet_sandbox::sandbox::{
    MockSandboxProvider, ProcessResult, ProcessRunner, ProcessSpec, TofuSandboxProvider,
    TofuSandboxProviderOptions, TokioProcessRunner,
};
use ratchet_sandbox::{Error, Result};
use serde_json::{Value, json};

fn input() -> SandboxInput {
    SandboxInput {
        sandbox_id: "demo".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.large".into(), "c7i.2xlarge".into()],
        max_instances_per_agent: 4,
        max_instance_lifetime_hours: 2.0,
        tags: Some(BTreeMap::from([("owner".to_string(), "me".to_string())])),
        environment: None,
    }
}

fn plan_json() -> String {
    json!({ "resource_changes": [
        { "address": "aws_s3_bucket.artifacts", "mode": "managed", "type": "aws_s3_bucket", "change": { "actions": ["create"] } },
        { "address": "aws_s3_bucket.sealed", "mode": "managed", "type": "aws_s3_bucket", "change": { "actions": ["update"] } },
        { "address": "aws_iam_role.bench_runner", "mode": "managed", "type": "aws_iam_role", "change": { "actions": ["delete", "create"] } },
        { "address": "aws_security_group.bench", "mode": "managed", "type": "aws_security_group", "change": { "actions": ["delete"] } },
        { "address": "aws_iam_policy.agent_boundary", "mode": "managed", "type": "aws_iam_policy", "change": { "actions": ["no-op"] } },
        { "address": "data.aws_availability_zones.available", "mode": "data", "type": "aws_availability_zones", "change": { "actions": ["read"] } },
    ] })
    .to_string()
}

fn outputs_doc() -> Value {
    let identity = |n: &str| {
        json!({
            "role_arn": format!("arn:aws:iam::1:role/ratchet/bench/ratchet-demo-{n}"),
            "role_name": format!("ratchet-demo-{n}"),
            "instance_profile_arn": format!("arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-{n}"),
            "instance_profile_name": format!("ratchet-demo-{n}"),
        })
    };
    json!({
        "artifacts_bucket": { "value": "ratchet-demo-artifacts" },
        "sealed_bucket": { "value": "ratchet-demo-sealed" },
        "agent_boundary_ref": { "value": "arn:aws:iam::1:policy/ratchet/ratchet-demo-agent-boundary" },
        "eval_identity_ref": { "value": identity("bench-runner") },
        "confirm_identity_ref": { "value": identity("confirm-runner") },
        "deps_identity_ref": { "value": identity("deps-runner") },
        "sealed_confirm_prefix": { "value": "holdout/" },
        "network_ref": { "value": {
            "vpc_id": "vpc-1", "subnet_id": "subnet-1", "bench_subnet_id": "subnet-bench",
            "security_group_ids": { "agent_runner": "sg-r", "bench": "sg-b" }, "egress_proxy_url": null,
        } },
        "agent_role_path": { "value": "/ratchet/agent/" },
        "limits": { "value": { "max_instances_per_agent": 4, "max_instance_lifetime_hours": 2 } },
    })
}

/// A `tofu` stand-in: records every invocation, answers like the real one would, and tracks concurrency.
struct FakeRunner {
    calls: Mutex<Vec<ProcessSpec>>,
    plan_exit: Mutex<i32>,
    fail_on: Mutex<Option<&'static str>>,
    inflight: AtomicUsize,
    max_inflight: AtomicUsize,
    outputs: Mutex<Value>,
}

impl FakeRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            plan_exit: Mutex::new(2),
            fail_on: Mutex::new(None),
            inflight: AtomicUsize::new(0),
            max_inflight: AtomicUsize::new(0),
            outputs: Mutex::new(outputs_doc()),
        })
    }
    fn subs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.args[0].clone())
            .collect()
    }
    fn call(&self, i: usize) -> ProcessSpec {
        self.calls.lock().unwrap()[i].clone()
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    fn max_inflight(&self) -> usize {
        self.max_inflight.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ProcessRunner for FakeRunner {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessResult> {
        self.calls.lock().unwrap().push(spec.clone());
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_inflight.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1)).await;
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        let sub = spec.args[0].as_str();
        let ok = |stdout: String| {
            Ok(ProcessResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            })
        };
        if *self.fail_on.lock().unwrap() == Some(sub) {
            return Ok(ProcessResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "Error: boom".into(),
            });
        }
        match sub {
            "plan" => Ok(ProcessResult {
                exit_code: *self.plan_exit.lock().unwrap(),
                stdout: String::new(),
                stderr: String::new(),
            }),
            "show" => ok(plan_json()),
            "output" => ok(self.outputs.lock().unwrap().to_string()),
            _ => ok(String::new()),
        }
    }
}

struct Rig {
    root: tempfile::TempDir,
    runner: Arc<FakeRunner>,
    roles: SharedRoles,
    tofu: TofuSandboxProvider,
}

fn rig() -> Rig {
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::new();
    let roles = SharedRoles::default();
    let opts = TofuSandboxProviderOptions::new(
        "/opt/tofu",
        "/repo/infra/aws",
        root.path(),
        Arc::new(roles.clone()),
    )
    .runner(runner.clone())
    .env(BTreeMap::from([(
        "AWS_PROFILE".to_string(),
        "sandbox".to_string(),
    )]));
    Rig {
        root,
        runner,
        roles,
        tofu: TofuSandboxProvider::new(opts),
    }
}

impl Rig {
    fn dir(&self) -> PathBuf {
        self.root.path().join("demo")
    }
}

// ---- plan ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn plan_runs_init_plan_out_show_json_with_a_per_sandbox_dir_and_vars_file() {
    let r = rig();
    let plan = r.tofu.plan(&input()).await.unwrap();
    assert_eq!(r.runner.subs(), ["init", "plan", "show"]);
    for i in 0..3 {
        let c = r.runner.call(i);
        assert_eq!(
            (c.command.as_str(), c.cwd),
            ("/opt/tofu", PathBuf::from("/repo/infra/aws"))
        );
    }
    let dir = r.dir().to_string_lossy().into_owned();
    let p = r.runner.call(1);
    for want in [
        "-detailed-exitcode".to_string(),
        format!("-out={dir}/plan.tfplan"),
        format!("-state={dir}/terraform.tfstate"),
        format!("-var-file={dir}/vars.tfvars.json"),
    ] {
        assert!(p.args.contains(&want), "{want} in {:?}", p.args);
    }
    assert!(
        r.runner
            .call(0)
            .args
            .contains(&"-backend=false".to_string())
    );
    assert_eq!(
        r.runner.call(2).args,
        ["show", "-json", "-no-color", &format!("{dir}/plan.tfplan")]
    );
    assert_eq!(p.env["AWS_PROFILE"], "sandbox");
    assert_eq!(p.env["TF_DATA_DIR"], format!("{dir}/.terraform-data"));
    assert_eq!(p.env["TF_IN_AUTOMATION"], "1");

    let vars: Value =
        serde_json::from_str(&std::fs::read_to_string(r.dir().join("vars.tfvars.json")).unwrap())
            .unwrap();
    assert_eq!(
        vars,
        json!({
            "sandbox_id": "demo", "region": "us-east-1", "allowed_instance_types": ["c7i.large", "c7i.2xlarge"],
            "max_instances_per_agent": 4, "max_instance_lifetime_hours": 2, "tags": { "owner": "me" },
        })
    );
    let got: Vec<_> = plan
        .changes
        .iter()
        .map(|c| (c.action, c.address.as_str(), c.resource_type.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                PlanAction::Create,
                "aws_s3_bucket.artifacts",
                "aws_s3_bucket"
            ),
            (PlanAction::Update, "aws_s3_bucket.sealed", "aws_s3_bucket"),
            (
                PlanAction::Replace,
                "aws_iam_role.bench_runner",
                "aws_iam_role"
            ),
            (
                PlanAction::Delete,
                "aws_security_group.bench",
                "aws_security_group"
            ),
            (
                PlanAction::NoOp,
                "aws_iam_policy.agent_boundary",
                "aws_iam_policy"
            ),
        ]
    );
    assert_eq!(plan.summary, "Plan: 2 to add, 1 to change, 2 to destroy.");
}

#[tokio::test]
async fn plan_exit_code_0_no_changes_is_fine_1_is_an_error_with_stderr() {
    let r = rig();
    *r.runner.plan_exit.lock().unwrap() = 0;
    r.tofu.plan(&input()).await.unwrap();
    *r.runner.plan_exit.lock().unwrap() = 1;
    let e = r.tofu.plan(&input()).await.unwrap_err();
    assert!(matches!(e, PortError::Failed(_)), "{e:?}");
    assert!(e.to_string().contains("tofu plan failed (exit 1)"), "{e}");
}

#[tokio::test]
async fn plan_validates_the_sandbox_id_and_instance_types() {
    let r = rig();
    let mut bad = input();
    bad.sandbox_id = "../evil".into();
    assert!(
        r.tofu
            .plan(&bad)
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid sandbox id")
    );
    let mut bad = input();
    bad.allowed_instance_types.clear();
    assert!(
        r.tofu
            .plan(&bad)
            .await
            .unwrap_err()
            .to_string()
            .contains("must not be empty")
    );
    assert_eq!(r.runner.count(), 0);
}

#[tokio::test]
async fn rejects_bad_sandbox_ids_at_every_provider_entry_point() {
    let r = rig();
    let long = "x".repeat(33);
    for bad in [
        "*",
        "?",
        "/",
        "..",
        "a/b",
        "a*",
        "a?",
        "../x",
        "a",
        "Demo",
        long.as_str(),
    ] {
        let mut i = input();
        i.sandbox_id = bad.into();
        let has = |e: &dyn std::fmt::Display| {
            assert!(e.to_string().contains("invalid sandbox id"), "{bad:?}: {e}")
        };
        has(&r.tofu.plan(&i).await.unwrap_err());
        has(&r.tofu.apply(&i).await.unwrap_err());
        has(&r.tofu.destroy(&i).await.unwrap_err());
        has(&r.tofu.outputs(bad).await.unwrap_err());
        has(&r.tofu.work_dir(bad).unwrap_err());
        let mock = MockSandboxProvider::default();
        has(&mock.plan(&i).await.unwrap_err());
        has(&mock.apply(&i).await.unwrap_err());
    }
    assert_eq!(r.runner.count(), 0);
}

#[tokio::test]
async fn does_not_put_anything_secret_shaped_in_the_vars_file() {
    let r = rig();
    r.tofu.plan(&input()).await.unwrap();
    let text = std::fs::read_to_string(r.dir().join("vars.tfvars.json"))
        .unwrap()
        .to_lowercase();
    for needle in ["key", "token", "secret"] {
        assert!(!text.contains(needle), "{needle} in {text}");
    }
}

// ---- apply --------------------------------------------------------------------------------------------

#[tokio::test]
async fn apply_plans_applies_the_saved_plan_and_maps_outputs_per_the_contract() {
    let r = rig();
    let out = r.tofu.apply(&input()).await.unwrap();
    assert_eq!(r.runner.subs(), ["init", "plan", "show", "apply", "output"]);
    let apply = r.runner.call(3);
    assert_eq!(
        apply.args.last().unwrap(),
        &format!("{}/plan.tfplan", r.dir().display())
    );
    assert!(
        !apply.args.contains(&"-auto-approve".to_string()),
        "saved plan only"
    );

    assert_eq!(out.artifacts_bucket, "ratchet-demo-artifacts");
    assert_eq!(out.sealed_bucket, "ratchet-demo-sealed");
    assert_eq!(
        out.agent_boundary_ref,
        "arn:aws:iam::1:policy/ratchet/ratchet-demo-agent-boundary"
    );
    assert_eq!(
        out.eval_identity_ref,
        "arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-bench-runner"
    );
    assert_eq!(out.agent_role_path, "/ratchet/agent/");
    assert_eq!(out.network.subnet_id, "subnet-1");
    assert_eq!(out.network.bench_subnet_id, "subnet-bench");
    assert_eq!(out.network.agent_runner_security_group_id, "sg-r");
    assert_eq!(out.network.bench_security_group_id, "sg-b");
    assert_eq!(out.network.egress_proxy_url, None);
    assert_eq!(
        (
            out.sealed_confirm_prefix.as_str(),
            out.region.as_str(),
            out.vpc_id.as_str()
        ),
        ("holdout/", "us-east-1", "vpc-1")
    );
    assert_eq!(
        (
            out.limits.max_instances_per_agent,
            out.limits.max_instance_lifetime_hours
        ),
        (4, 2.0)
    );
    assert_eq!(out.eval_identity.role_name, "ratchet-demo-bench-runner");
    // two identities: eval_identity_ref has NO sealed access, confirm_identity_ref does
    assert_eq!(
        out.confirm_identity.instance_profile_arn,
        "arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-confirm-runner"
    );
    assert_ne!(out.confirm_identity.role_arn, out.eval_identity.role_arn);
    // ... and a third for dependency snapshots: it writes bench-deps/ and has no sealed access
    let deps = out.deps_identity.as_ref().unwrap();
    assert_eq!(
        deps.instance_profile_arn,
        "arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-deps-runner"
    );
    let roles: std::collections::HashSet<_> = [
        &out.eval_identity.role_arn,
        &out.confirm_identity.role_arn,
        &deps.role_arn,
    ]
    .into_iter()
    .collect();
    assert_eq!(roles.len(), 3);
}

#[test]
fn parses_outputs_of_a_sandbox_applied_before_dependency_snapshots_the_identity_is_simply_absent() {
    let mut doc = outputs_doc();
    doc.as_object_mut()
        .unwrap()
        .shift_remove("deps_identity_ref");
    let out = parse_tofu_outputs(&doc.to_string(), Some("us-east-1"))
        .unwrap()
        .unwrap();
    assert!(out.deps_identity.is_none());
    assert_eq!(
        out.confirm_identity.role_name,
        "ratchet-demo-confirm-runner"
    );
}

#[tokio::test]
async fn apply_stops_after_a_failed_apply() {
    let r = rig();
    *r.runner.fail_on.lock().unwrap() = Some("apply");
    let e = r.tofu.apply(&input()).await.unwrap_err();
    assert!(e.to_string().contains("tofu apply failed"), "{e}");
    assert!(!r.runner.subs().contains(&"output".to_string()));
}

#[tokio::test]
async fn serializes_concurrent_operations_on_the_same_sandbox() {
    let r = rig();
    let i = input();
    let (a, b, c) = tokio::join!(r.tofu.plan(&i), r.tofu.plan(&i), r.tofu.apply(&i));
    a.unwrap();
    b.unwrap();
    c.unwrap();
    assert_eq!(r.runner.max_inflight(), 1);
}

#[tokio::test]
async fn does_not_block_different_sandboxes_on_each_other() {
    let r = rig();
    let mut other = input();
    other.sandbox_id = "other".into();
    let first = input();
    let (a, b) = tokio::join!(r.tofu.plan(&first), r.tofu.plan(&other));
    a.unwrap();
    b.unwrap();
    assert_eq!(r.runner.max_inflight(), 2);
}

#[tokio::test]
async fn a_failed_operation_does_not_wedge_the_queue_for_that_sandbox() {
    let r = rig();
    *r.runner.fail_on.lock().unwrap() = Some("init");
    assert!(r.tofu.plan(&input()).await.is_err());
    *r.runner.fail_on.lock().unwrap() = None;
    r.tofu.plan(&input()).await.unwrap();
}

// ---- destroy ------------------------------------------------------------------------------------------

#[tokio::test]
async fn destroy_refuses_while_runtime_agent_roles_exist_and_runs_nothing() {
    let r = rig();
    r.roles.set(vec!["ratchet-agent-opt1-a1".into()]);
    let e = r.tofu.destroy(&input()).await.unwrap_err();
    match &e {
        PortError::TeardownBlocked(m) => assert!(
            m.contains("1 runtime agent role(s) still exist (ratchet-agent-opt1-a1)"),
            "{m}"
        ),
        other => panic!("expected TeardownBlocked, got {other:?}"),
    }
    assert!(e.to_string().contains("kill switch"), "{e}");
    assert_eq!(r.runner.count(), 0);
}

#[tokio::test]
async fn destroy_runs_once_the_roles_are_gone() {
    let r = rig();
    r.tofu.destroy(&input()).await.unwrap();
    assert_eq!(r.runner.subs(), ["init", "destroy"]);
    let dir = r.dir().display().to_string();
    let args = r.runner.call(1).args;
    for want in [
        "-auto-approve".to_string(),
        format!("-state={dir}/terraform.tfstate"),
        format!("-var-file={dir}/vars.tfvars.json"),
    ] {
        assert!(args.contains(&want), "{want} in {args:?}");
    }
}

#[tokio::test]
async fn destroy_surfaces_tofu_failures() {
    let r = rig();
    *r.runner.fail_on.lock().unwrap() = Some("destroy");
    assert!(matches!(
        r.tofu.destroy(&input()).await.unwrap_err(),
        PortError::Failed(m) if m.starts_with("tofu destroy failed")
    ));
}

// ---- outputs ------------------------------------------------------------------------------------------

#[tokio::test]
async fn outputs_is_none_for_an_unknown_sandbox_without_spawning_tofu() {
    let r = rig();
    assert!(r.tofu.outputs("nope").await.unwrap().is_none());
    assert_eq!(r.runner.count(), 0);
}

#[tokio::test]
async fn outputs_reads_outputs_and_region_for_an_applied_sandbox() {
    let r = rig();
    r.tofu.apply(&input()).await.unwrap();
    r.runner.calls.lock().unwrap().clear();
    std::fs::write(r.dir().join("terraform.tfstate"), "{}").unwrap();
    let out = r.tofu.outputs("demo").await.unwrap().unwrap();
    assert_eq!(out.region, "us-east-1");
    assert_eq!(out.artifacts_bucket, "ratchet-demo-artifacts");
    assert_eq!(r.runner.subs(), ["output"]);
}

// ---- pure parsers -------------------------------------------------------------------------------------

#[test]
fn map_actions_matches_the_terraform_action_pairs() {
    let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(map_actions(&s(&["create", "delete"])), PlanAction::Replace);
    assert_eq!(map_actions(&s(&["delete", "create"])), PlanAction::Replace);
    assert_eq!(map_actions(&s(&["read"])), PlanAction::NoOp);
    assert_eq!(map_actions(&s(&["update"])), PlanAction::Update);
    assert_eq!(map_actions(&s(&[])), PlanAction::Update);
}

#[test]
fn summarize_plan_tolerates_an_empty_plan() {
    let p = summarize_plan("{}").unwrap();
    assert!(p.changes.is_empty());
    assert_eq!(p.summary, "Plan: 0 to add, 0 to change, 0 to destroy.");
}

#[test]
fn parse_tofu_outputs_empty_is_none_missing_output_is_a_clear_error() {
    assert!(parse_tofu_outputs("{}", None).unwrap().is_none());
    let e = parse_tofu_outputs(
        &json!({ "artifacts_bucket": { "value": "x" } }).to_string(),
        None,
    )
    .unwrap_err();
    assert!(e.to_string().contains("contract"), "{e}");
}

#[test]
fn the_outputs_round_trip_through_json_in_the_typescript_shape() {
    let out = parse_tofu_outputs(&outputs_doc().to_string(), Some("us-east-1"))
        .unwrap()
        .unwrap();
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v["artifactsBucket"], "ratchet-demo-artifacts");
    assert_eq!(v["network"]["benchSubnetId"], "subnet-bench");
    assert_eq!(v["network"]["egressProxyUrl"], Value::Null);
    assert_eq!(
        v["evalIdentity"]["instanceProfileName"],
        "ratchet-demo-bench-runner"
    );
    assert_eq!(
        v["limits"],
        json!({ "maxInstancesPerAgent": 4, "maxInstanceLifetimeHours": 2 })
    );
    assert_eq!(
        serde_json::from_value::<ratchet_core::ports::SandboxOutputs>(v).unwrap(),
        out
    );
}

// ---- generated tofu arguments (snapshot) ----------------------------------------------------------------

#[tokio::test]
async fn tofu_invocations_for_apply_and_destroy_are_pinned() {
    let r = rig();
    r.tofu.apply(&input()).await.unwrap();
    r.tofu.destroy(&input()).await.unwrap();
    let root = r.root.path().to_string_lossy().into_owned();
    let calls: Vec<Value> = r
        .runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| {
            let clean = |s: &str| s.replace(&root, "<ROOT>");
            json!({
                "cmd": c.command, "cwd": c.cwd, "args": c.args.iter().map(|a| clean(a)).collect::<Vec<_>>(),
                "env": c.env.iter().map(|(k, v)| (k.clone(), clean(v))).collect::<BTreeMap<_, _>>(),
            })
        })
        .collect();
    insta::assert_json_snapshot!("tofu_invocations", calls);
    let vars = std::fs::read_to_string(r.dir().join("vars.tfvars.json")).unwrap();
    insta::assert_snapshot!("tofu_vars_file", vars);
}

// ---- the real process runner ----------------------------------------------------------------------------

#[tokio::test]
async fn the_real_runner_captures_stdout_stderr_exit_code_env_and_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let spec = ProcessSpec {
        command: "sh".into(),
        args: vec![
            "-c".into(),
            "echo out; echo err >&2; echo \"$TF_PROBE\"; pwd; exit 3".into(),
        ],
        cwd: dir.path().to_path_buf(),
        env: BTreeMap::from([("TF_PROBE".to_string(), "probe-value".to_string())]),
    };
    let r = TokioProcessRunner.run(&spec).await.unwrap();
    assert_eq!(r.exit_code, 3);
    assert_eq!(r.stderr, "err\n");
    let mut lines = r.stdout.lines();
    assert_eq!(
        (lines.next(), lines.next()),
        (Some("out"), Some("probe-value"))
    );
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(std::fs::canonicalize(lines.next().unwrap()).unwrap(), cwd);
}

#[tokio::test]
async fn the_real_runner_reports_a_missing_binary_as_an_error_and_closes_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let missing = ProcessSpec {
        command: "/nonexistent/tofu".into(),
        args: vec![],
        cwd: dir.path().to_path_buf(),
        env: BTreeMap::new(),
    };
    assert!(matches!(
        TokioProcessRunner.run(&missing).await.unwrap_err(),
        Error::Io(_)
    ));
    // stdin is closed, so a command that reads it sees EOF instead of hanging
    let cat = ProcessSpec {
        command: "cat".into(),
        args: vec![],
        cwd: dir.path().to_path_buf(),
        env: BTreeMap::new(),
    };
    let r = tokio::time::timeout(Duration::from_secs(10), TokioProcessRunner.run(&cat))
        .await
        .expect("cat must not hang")
        .unwrap();
    assert_eq!((r.exit_code, r.stdout.as_str()), (0, ""));
}

#[tokio::test]
async fn tofu_error_messages_keep_the_tail_of_stderr() {
    let r = rig();
    struct Noisy;
    #[async_trait]
    impl ProcessRunner for Noisy {
        async fn run(&self, _: &ProcessSpec) -> Result<ProcessResult> {
            Ok(ProcessResult {
                exit_code: 1,
                stdout: "stdout only".into(),
                stderr: format!("{}TAIL", "x".repeat(5000)),
            })
        }
    }
    let opts = TofuSandboxProviderOptions::new(
        "/opt/tofu",
        "/m",
        r.root.path(),
        Arc::new(SharedRoles::default()),
    )
    .runner(Arc::new(Noisy));
    let e = TofuSandboxProvider::new(opts)
        .plan(&input())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("tofu init failed (exit 1): "), "{e}");
    assert!(e.ends_with("xxxTAIL"));
    assert_eq!(e.len(), "tofu init failed (exit 1): ".len() + 2000);
}
