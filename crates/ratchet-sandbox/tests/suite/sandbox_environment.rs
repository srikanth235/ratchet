//! Port of `packages/aws/src/sandbox/environment.test.ts`: materializing a drafted / custom environment, the two-stage
//! `TofuSandboxProvider` (with a faked `tofu`), the mock, and `TofuEnvironmentChecker`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::{
    PlanAction, PortError, SandboxEnvironment, SandboxEnvironmentFile, SandboxEnvironmentKind,
    SandboxEnvironmentMachine, SandboxInput, SandboxProvider,
};
use ratchet_env::{
    EnvironmentChecker, EnvironmentFile, EnvironmentPlanInput, hash_environment_files,
};
use ratchet_sandbox::sandbox::tofu::parse_tofu_outputs;
use ratchet_sandbox::sandbox::{
    BASE_MODULE_DIRNAME, EnvironmentRejectedError, EnvironmentVarsOptions, GENERATED_INPUTS_FILE,
    GENERATED_OUTPUTS_FILE, MockSandboxProvider, ProcessResult, ProcessRunner, ProcessSpec,
    SharedRoles, TofuEnvironmentChecker, TofuEnvironmentCheckerOptions, TofuSandboxProvider,
    TofuSandboxProviderOptions, assert_drafted_environment, environment_vars,
    materialize_custom_environment, materialize_drafted_environment, parse_environment_outputs,
};
use ratchet_sandbox::{Error, Result};
use serde_json::{Value, json};

fn base_module_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../infra/modules/sandbox")
}
fn module_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../infra/aws")
}

const MAIN_TF: &str = r#"module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "the service"
      instance_type = "c7i.2xlarge"
      user_data     = file("${path.module}/api.sh")
    }
    db = {
      role           = "database"
      label          = "Postgres"
      instance_type  = "r7g.large"
      data_volume_gb = 20
    }
  }
}
"#;

fn files(list: &[(&str, &str)]) -> Vec<SandboxEnvironmentFile> {
    list.iter()
        .map(|(p, c)| SandboxEnvironmentFile {
            path: (*p).into(),
            content: (*c).into(),
        })
        .collect()
}

fn env_of(list: &[(&str, &str)]) -> SandboxEnvironment {
    SandboxEnvironment {
        kind: SandboxEnvironmentKind::Drafted,
        scope: EnvironmentScope::Full,
        path: "infra/ratchet".into(),
        commit: "abc123".into(),
        hash: hash_environment_files(list.iter().copied()),
        files: files(list),
    }
}

fn env() -> SandboxEnvironment {
    env_of(&[("main.tf", MAIN_TF), ("api.sh", "#!/bin/bash\n")])
}

fn custom(list: &[(&str, &str)]) -> SandboxEnvironment {
    SandboxEnvironment {
        kind: SandboxEnvironmentKind::Custom,
        scope: EnvironmentScope::Full,
        path: "ops/sandbox".into(),
        commit: "c".into(),
        hash: "h".into(),
        files: files(list),
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn rejected(e: Error) -> EnvironmentRejectedError {
    match e {
        Error::EnvironmentRejected(r) => r,
        other => panic!("expected EnvironmentRejectedError, got {other:?}"),
    }
}

// ---- materializeDraftedEnvironment -------------------------------------------------------------------------------

// "materializeDraftedEnvironment writes the draft, a copy of the base module and the two generated files"
#[tokio::test]
async fn writes_the_draft_a_copy_of_the_base_module_and_the_two_generated_files() {
    let root = tempfile::tempdir().unwrap();
    let dest =
        materialize_drafted_environment(&env(), &base_module_dir(), &root.path().join("env"))
            .await
            .unwrap();
    let mut want = vec![
        "api.sh".to_string(),
        "main.tf".into(),
        GENERATED_INPUTS_FILE.into(),
        GENERATED_OUTPUTS_FILE.into(),
        BASE_MODULE_DIRNAME.into(),
    ];
    want.sort();
    assert_eq!(names(&dest), want);
    assert_eq!(
        names(&dest.join(BASE_MODULE_DIRNAME)),
        ["main.tf", "outputs.tf", "variables.tf", "versions.tf"]
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("main.tf")).unwrap(),
        MAIN_TF
    );
    // the copy is the shipped module, byte for byte (the draft cannot supply its own)
    assert_eq!(
        std::fs::read(dest.join(BASE_MODULE_DIRNAME).join("main.tf")).unwrap(),
        std::fs::read(base_module_dir().join("main.tf")).unwrap()
    );
    let inputs = std::fs::read_to_string(dest.join(GENERATED_INPUTS_FILE)).unwrap();
    assert!(inputs.contains("variable \"ratchet\""));
    assert!(inputs.contains("provider \"aws\""));
    assert!(inputs.contains("region = var.ratchet.region"));
    assert!(
        std::fs::read_to_string(dest.join(GENERATED_OUTPUTS_FILE))
            .unwrap()
            .contains("module.sandbox.environment")
    );
}

// "materializeDraftedEnvironment starts from an empty directory every time (stale files of an earlier draft never survive)"
#[tokio::test]
async fn starts_from_an_empty_directory_every_time() {
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("env");
    materialize_drafted_environment(&env(), &base_module_dir(), &dest)
        .await
        .unwrap();
    let _ = materialize_drafted_environment(
        &env_of(&[("main.tf", MAIN_TF)]),
        &base_module_dir(),
        &dest,
    )
    .await;
    let no_script = MAIN_TF.replace("file(\"${path.module}/api.sh\")", "\"\"");
    materialize_drafted_environment(
        &env_of(&[("main.tf", &no_script)]),
        &base_module_dir(),
        &dest,
    )
    .await
    .unwrap();
    assert!(!names(&dest).contains(&"api.sh".to_string()));
}

// "materializeDraftedEnvironment refuses a draft that fails the validator, before writing anything"
#[tokio::test]
async fn refuses_a_draft_that_fails_the_validator_before_writing_anything() {
    let root = tempfile::tempdir().unwrap();
    let bad_tf = format!(
        "{MAIN_TF}\nresource \"aws_internet_gateway\" \"igw\" {{\n  tags = module.sandbox.tags\n}}\n"
    );
    let e = materialize_drafted_environment(
        &env_of(&[("main.tf", &bad_tf)]),
        &base_module_dir(),
        &root.path().join("env"),
    )
    .await
    .unwrap_err();
    let r = rejected(e);
    assert!(
        r.problems
            .join(",")
            .contains("Isolated: no internet on the measuring hosts"),
        "{r}"
    );
    assert!(!root.path().join("env").exists());
}

// "materializeDraftedEnvironment refuses a draft that does not use the base module, and unsafe paths"
#[tokio::test]
async fn refuses_a_draft_without_the_base_module_and_unsafe_paths() {
    let e = assert_drafted_environment(&env_of(&[("main.tf", "locals {\n a = 1\n}\n")]), None)
        .unwrap_err();
    assert!(e.to_string().contains("missing the base module"), "{e}");
    let root = tempfile::tempdir().unwrap();
    let evil = env_of(&[("../escape.tf", MAIN_TF)]);
    let e = materialize_drafted_environment(&evil, &base_module_dir(), &root.path().join("e2"))
        .await
        .unwrap_err();
    rejected(e);
}

// "materializeDraftedEnvironment refuses when the base module is not where it should be"
#[tokio::test]
async fn refuses_when_the_base_module_is_not_where_it_should_be() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        materialize_drafted_environment(
            &env(),
            &root.path().join("missing"),
            &root.path().join("e3")
        )
        .await
        .is_err()
    );
    let e = materialize_drafted_environment(&env(), root.path(), &root.path().join("e4"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("base module not found"), "{e}");
}

// ---- materializeCustomEnvironment --------------------------------------------------------------------------------

// "materializeCustomEnvironment (infra: in ratchet.yaml) writes the module and nothing else"
#[tokio::test]
async fn custom_writes_the_module_and_nothing_else() {
    let root = tempfile::tempdir().unwrap();
    let dest = materialize_custom_environment(
        &custom(&[
            ("main.tf", "resource \"aws_s3_bucket\" \"a\" {}\n"),
            ("sub/x.tf", "# x\n"),
        ]),
        &root.path().join("c"),
    )
    .await
    .unwrap();
    assert_eq!(names(&dest), ["main.tf", "sub"]);
}

// "materializeCustomEnvironment (infra: in ratchet.yaml) refuses a module without .tf files or that can run commands here"
#[tokio::test]
async fn custom_refuses_a_module_without_tf_files_or_that_can_run_commands() {
    let root = tempfile::tempdir().unwrap();
    let e = materialize_custom_environment(&custom(&[("README", "x")]), &root.path().join("c1"))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::InfraOverride(_)), "{e:?}");
    let e = materialize_custom_environment(
        &custom(&[(
            "main.tf",
            "resource \"null_resource\" \"x\" {\n provisioner \"local-exec\" { command = \"id\" }\n}",
        )]),
        &root.path().join("c2"),
    )
    .await
    .unwrap_err();
    assert!(
        e.to_string().contains("can run commands on this machine"),
        "{e}"
    );
}

// ---- environmentVars / parseEnvironmentOutputs -------------------------------------------------------------------

fn sandbox_out() -> ratchet_core::ports::SandboxOutputs {
    let ident = |k: &str, n: &str| {
        json!({ "role_arn": format!("arn:aws:iam::1:role/{k}"), "role_name": k,
                "instance_profile_arn": format!("arn:aws:iam::1:instance-profile/{k}"), "instance_profile_name": n })
    };
    parse_tofu_outputs(
        &json!({
            "artifacts_bucket": { "value": "ratchet-demo-artifacts" },
            "sealed_bucket": { "value": "ratchet-demo-sealed" },
            "agent_boundary_ref": { "value": "arn:aws:iam::1:policy/b" },
            "eval_identity_ref": { "value": ident("e", "ratchet-demo-bench-runner") },
            "confirm_identity_ref": { "value": ident("c", "c") },
            "sealed_confirm_prefix": { "value": "holdout/" },
            "network_ref": { "value": { "vpc_id": "vpc-1", "subnet_id": "subnet-a", "bench_subnet_id": "subnet-b",
                "security_group_ids": { "agent_runner": "sg-a", "bench": "sg-b" }, "egress_proxy_url": null } },
            "agent_role_path": { "value": "/ratchet/agent/" },
            "limits": { "value": { "max_instances_per_agent": 2, "max_instance_lifetime_hours": 4 } },
        })
        .to_string(),
        Some("us-east-1"),
    )
    .unwrap()
    .unwrap()
}

// "environmentVars / parseEnvironmentOutputs feeds the base module from the sandbox contract outputs: the bench subnet and group, the evaluation-only profile"
#[test]
fn feeds_the_base_module_from_the_sandbox_contract_outputs() {
    let v = environment_vars(
        &sandbox_out(),
        &env(),
        &EnvironmentVarsOptions {
            sandbox_id: "ratchet-env-1234abcd".into(),
            tags: Some(BTreeMap::from([("owner".to_string(), "me".to_string())])),
            machine_image_id: None,
            image_version: Some("0.4.0".into()),
        },
    );
    assert_eq!(
        v["ratchet"],
        json!({
            "sandbox_id": "ratchet-env-1234abcd",
            "region": "us-east-1",
            "vpc_id": "vpc-1",
            "bench_subnet_id": "subnet-b",
            "bench_security_group_id": "sg-b",
            "artifacts_bucket": "ratchet-demo-artifacts",
            "eval_instance_profile_name": "ratchet-demo-bench-runner",
            "environment_hash": env().hash,
            "machine_image_id": "",
            "image_version": "0.4.0",
            "tags": { "owner": "me" },
        })
    );
    let text = v.to_string();
    for banned in ["confirm", "sealed", "agent-runner", "subnet-a", "sg-a"] {
        assert!(!text.contains(banned), "{banned} leaked into {text}");
    }
}

// "environmentVars / parseEnvironmentOutputs reads the machines and the created resources (data sources excluded)"
#[test]
fn reads_the_machines_and_the_created_resources_data_sources_excluded() {
    let r = parse_environment_outputs(
        &env(),
        &json!({ "environment": { "value": { "hash": "h", "machines": {
            "api": { "id": "i-1", "role": "app", "private_ip": "10.0.1.5" },
            "db": { "id": "i-2", "role": "database", "private_ip": "10.0.1.6" } } } } })
        .to_string(),
        "module.sandbox.aws_instance.machine[\"api\"]\nmodule.sandbox.data.aws_ami.bench[0]\nmodule.sandbox.aws_instance.machine[\"db\"]\n\n",
    )
    .unwrap();
    assert_eq!(r.kind, SandboxEnvironmentKind::Drafted);
    assert_eq!(r.scope, EnvironmentScope::Full);
    assert_eq!(r.hash, env().hash);
    assert_eq!(
        r.resources,
        [
            "module.sandbox.aws_instance.machine[\"api\"]",
            "module.sandbox.aws_instance.machine[\"db\"]"
        ]
    );
    let m = |id: &str, role: &str, ip: &str| SandboxEnvironmentMachine {
        id: id.into(),
        role: role.into(),
        private_ip: ip.into(),
    };
    assert_eq!(
        r.machines,
        BTreeMap::from([
            ("api".to_string(), m("i-1", "app", "10.0.1.5")),
            ("db".to_string(), m("i-2", "database", "10.0.1.6")),
        ])
    );
}

// ---- the provider ------------------------------------------------------------------------------------------------

fn sbx_out() -> Value {
    let ident = |n: &str| {
        json!({
            "role_arn": format!("arn:aws:iam::123456789012:role/ratchet/{n}"), "role_name": n,
            "instance_profile_arn": format!("arn:aws:iam::123456789012:instance-profile/ratchet/{n}"), "instance_profile_name": n,
        })
    };
    json!({
        "artifacts_bucket": { "value": "ratchet-envsbx-artifacts" },
        "sealed_bucket": { "value": "ratchet-envsbx-sealed" },
        "agent_boundary_ref": { "value": "arn:aws:iam::123456789012:policy/ratchet/b" },
        "eval_identity_ref": { "value": ident("e") },
        "confirm_identity_ref": { "value": ident("c") },
        "deps_identity_ref": { "value": ident("d") },
        "sealed_confirm_prefix": { "value": "holdout/" },
        "network_ref": { "value": { "vpc_id": "vpc-0abc", "subnet_id": "subnet-0a", "bench_subnet_id": "subnet-0b",
            "security_group_ids": { "agent_runner": "sg-0a", "bench": "sg-0b" }, "egress_proxy_url": null } },
        "agent_role_path": { "value": "/ratchet/agent/" },
        "limits": { "value": { "max_instances_per_agent": 2, "max_instance_lifetime_hours": 4 } },
    })
}

fn env_out() -> Value {
    json!({ "environment": { "value": { "hash": "h", "machines": {
        "api": { "id": "i-0api", "role": "app", "private_ip": "10.42.1.10" },
        "db": { "id": "i-0db", "role": "database", "private_ip": "10.42.1.11" } } } } })
}

struct Runner {
    state_root: PathBuf,
    module_dir: PathBuf,
    calls: Mutex<Vec<ProcessSpec>>,
    sandbox_outputs: Mutex<Value>,
    fail_env_on: Mutex<Option<&'static str>>,
}

impl Runner {
    fn new(state_root: &Path) -> Arc<Self> {
        Arc::new(Self {
            state_root: state_root.to_path_buf(),
            module_dir: module_dir(),
            calls: Mutex::default(),
            sandbox_outputs: Mutex::new(sbx_out()),
            fail_env_on: Mutex::new(None),
        })
    }
    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
    fn calls(&self) -> Vec<ProcessSpec> {
        self.calls.lock().unwrap().clone()
    }
    /// `<where>:<subcommand>` per call: `env` for the drafted stage, `sbx` for the standard module, `custom` for a custom module.
    fn trace(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|c| {
                let w = if c.cwd.ends_with("env") {
                    "env".to_string()
                } else if c.cwd.ends_with("custom-module") {
                    "custom".to_string()
                } else if c.cwd == self.module_dir {
                    "sbx".to_string()
                } else {
                    c.cwd.display().to_string()
                };
                format!("{w}:{}", c.args[0])
            })
            .collect()
    }
    fn find(&self, sub: &str, where_env: bool) -> ProcessSpec {
        self.calls()
            .into_iter()
            .find(|c| c.args[0] == sub && c.cwd.ends_with("env") == where_env)
            .unwrap_or_else(|| panic!("no {sub} call"))
    }
}

#[async_trait]
impl ProcessRunner for Runner {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessResult> {
        self.calls.lock().unwrap().push(spec.clone());
        let sub = spec.args[0].as_str();
        let in_env = spec.cwd.ends_with("env");
        let res = |exit_code: i32, stdout: String, stderr: &str| {
            Ok(ProcessResult {
                exit_code,
                stdout,
                stderr: stderr.into(),
            })
        };
        if in_env && *self.fail_env_on.lock().unwrap() == Some(sub) {
            return res(1, String::new(), "Error: nope");
        }
        if sub == "apply" {
            // what tofu does
            let f = if in_env {
                "env.tfstate"
            } else {
                "terraform.tfstate"
            };
            std::fs::write(self.state_root.join("envsbx").join(f), "{}").unwrap();
        }
        match sub {
            "plan" => res(2, String::new(), ""),
            "show" => res(
                0,
                json!({ "resource_changes": [{ "address": "aws_vpc.sandbox", "mode": "managed", "type": "aws_vpc", "change": { "actions": ["create"] } }] }).to_string(),
                "",
            ),
            "output" => res(
                0,
                if in_env {
                    env_out().to_string()
                } else {
                    self.sandbox_outputs.lock().unwrap().to_string()
                },
                "",
            ),
            "state" => res(
                0,
                "module.sandbox.aws_instance.machine[\"api\"]\nmodule.sandbox.aws_instance.machine[\"db\"]\nmodule.sandbox.aws_ebs_volume.data[\"db\"]\n".into(),
                "",
            ),
            _ => res(0, String::new(), ""),
        }
    }
}

fn input(environment: Option<SandboxEnvironment>) -> SandboxInput {
    SandboxInput {
        sandbox_id: "envsbx".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.large".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 4.0,
        tags: Some(BTreeMap::from([(
            "ratchet:managed".to_string(),
            "true".to_string(),
        )])),
        environment,
    }
}

struct Rig {
    root: tempfile::TempDir,
    runner: Arc<Runner>,
    provider: TofuSandboxProvider,
}

fn rig_with(roles: Vec<String>, image_version: Option<&str>) -> Rig {
    let root = tempfile::tempdir().unwrap();
    let runner = Runner::new(root.path());
    let shared = SharedRoles::default();
    shared.set(roles);
    let mut opts =
        TofuSandboxProviderOptions::new("/opt/tofu", module_dir(), root.path(), Arc::new(shared))
            .runner(runner.clone())
            .env(BTreeMap::from([(
                "AWS_PROFILE".to_string(),
                "p".to_string(),
            )]));
    if let Some(v) = image_version {
        opts = opts.image_version(v);
    }
    Rig {
        provider: TofuSandboxProvider::new(opts),
        runner,
        root,
    }
}

fn rig() -> Rig {
    rig_with(vec![], Some("0.4.0"))
}

fn t(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

// "TofuSandboxProvider with a drafted environment (two stages) applies the standard sandbox first, then the environment on top of it, and reports what it created"
#[tokio::test]
async fn applies_the_standard_sandbox_first_then_the_environment_on_top_of_it() {
    let r = rig();
    let out = r.provider.apply(&input(Some(env()))).await.unwrap();
    assert_eq!(
        r.runner.trace(),
        t(&[
            "sbx:init",
            "sbx:plan",
            "sbx:show",
            "sbx:apply",
            "sbx:output",
            "env:init",
            "env:plan",
            "env:apply",
            "env:output",
            "env:state"
        ])
    );
    let e = out.environment.unwrap();
    assert_eq!(e.kind, SandboxEnvironmentKind::Drafted);
    assert_eq!(e.scope, EnvironmentScope::Full);
    assert_eq!(e.hash, env().hash);
    assert_eq!(
        e.resources,
        [
            "module.sandbox.aws_instance.machine[\"api\"]",
            "module.sandbox.aws_instance.machine[\"db\"]",
            "module.sandbox.aws_ebs_volume.data[\"db\"]"
        ]
    );
    assert_eq!(
        e.machines["api"],
        SandboxEnvironmentMachine {
            id: "i-0api".into(),
            role: "app".into(),
            private_ip: "10.42.1.10".into()
        }
    );
    assert_eq!(e.machines["db"].private_ip, "10.42.1.11");

    // the second stage has its own state, data dir and variables; the standard sandbox's files are untouched
    let dir = r.root.path().join("envsbx");
    let d = dir.display().to_string();
    let plan_env = r.runner.find("plan", true);
    for want in [
        format!("-state={d}/env.tfstate"),
        format!("-var-file={d}/env.tfvars.json"),
        "-detailed-exitcode".to_string(),
    ] {
        assert!(
            plan_env.args.contains(&want),
            "{want} not in {:?}",
            plan_env.args
        );
    }
    assert_eq!(
        plan_env.env["TF_DATA_DIR"],
        format!("{d}/.terraform-data-env")
    );
    let sbx_plan = r.runner.find("plan", false);
    assert!(
        sbx_plan
            .args
            .contains(&format!("-state={d}/terraform.tfstate"))
    );
    assert_eq!(sbx_plan.env["TF_DATA_DIR"], format!("{d}/.terraform-data"));
    let vars: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("env.tfvars.json")).unwrap())
            .unwrap();
    let ratchet = &vars["ratchet"];
    assert_eq!(ratchet["bench_subnet_id"], "subnet-0b");
    assert_eq!(ratchet["bench_security_group_id"], "sg-0b");
    assert_eq!(ratchet["eval_instance_profile_name"], "e");
    assert_eq!(ratchet["artifacts_bucket"], "ratchet-envsbx-artifacts");
    assert_eq!(ratchet["image_version"], "0.4.0");
    assert_eq!(ratchet["environment_hash"], env().hash.as_str());
    // the module that was applied is the draft on top of the shipped base module
    let top = names(&dir.join("env"));
    for want in [
        "main.tf",
        "api.sh",
        GENERATED_INPUTS_FILE,
        GENERATED_OUTPUTS_FILE,
        BASE_MODULE_DIRNAME,
    ] {
        assert!(top.contains(&want.to_string()), "{want} missing in {top:?}");
    }
    let meta: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("env-meta.json")).unwrap()).unwrap();
    assert_eq!(meta["hash"], env().hash.as_str());
}

// "TofuSandboxProvider with a drafted environment (two stages) applies the environment saved plan, never -auto-approve"
#[tokio::test]
async fn applies_the_environment_saved_plan_never_auto_approve() {
    let r = rig();
    r.provider.apply(&input(Some(env()))).await.unwrap();
    let apply = r.runner.find("apply", true);
    assert!(!apply.args.contains(&"-auto-approve".to_string()));
    assert_eq!(
        apply.args.last().unwrap(),
        &r.root
            .path()
            .join("envsbx")
            .join("env.tfplan")
            .display()
            .to_string()
    );
}

// "TofuSandboxProvider with a drafted environment (two stages) refuses a draft that fails the contract BEFORE running any tofu command"
#[tokio::test]
async fn refuses_a_draft_that_fails_the_contract_before_running_any_tofu_command() {
    let r = rig();
    let bad_tf = format!(
        "{MAIN_TF}\nresource \"aws_iam_role\" \"r\" {{\n  tags = module.sandbox.tags\n}}\n"
    );
    let bad = env_of(&[("main.tf", &bad_tf)]);
    let e = r
        .provider
        .apply(&input(Some(bad.clone())))
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("does not satisfy the sandbox contract"),
        "{e}"
    );
    let e = r.provider.plan(&input(Some(bad))).await.unwrap_err();
    assert!(e.to_string().contains("Results sealed"), "{e}");
    assert!(r.runner.calls().is_empty());
}

// "TofuSandboxProvider with a drafted environment (two stages) a failed environment stage leaves the sandbox applied (the lifecycle marks it failed) and destroy still removes both stages"
#[tokio::test]
async fn a_failed_environment_stage_still_lets_destroy_remove_both_stages() {
    let r = rig();
    *r.runner.fail_env_on.lock().unwrap() = Some("apply");
    let e = r.provider.apply(&input(Some(env()))).await.unwrap_err();
    assert!(e.to_string().contains("tofu apply failed"), "{e}");
    *r.runner.fail_env_on.lock().unwrap() = None;
    // the stage wrote its state before failing: simulate that
    std::fs::write(r.root.path().join("envsbx").join("env.tfstate"), "{}").unwrap();
    r.runner.clear();
    r.provider.destroy(&input(None)).await.unwrap();
    assert_eq!(
        r.runner.trace(),
        t(&["env:init", "env:destroy", "sbx:init", "sbx:destroy"])
    );
}

// "TofuSandboxProvider with a drafted environment (two stages) destroy removes the environment first, then the sandbox, from the files that were applied"
#[tokio::test]
async fn destroy_removes_the_environment_first_then_the_sandbox() {
    let r = rig();
    r.provider.apply(&input(Some(env()))).await.unwrap();
    r.runner.clear();
    r.provider.destroy(&input(None)).await.unwrap(); // the caller passes no environment: the stored copy is used
    assert_eq!(
        r.runner.trace(),
        t(&["env:init", "env:destroy", "sbx:init", "sbx:destroy"])
    );
    let d = r.runner.find("destroy", true);
    let dir = r.root.path().join("envsbx");
    for want in [
        "-auto-approve".to_string(),
        format!("-state={}", dir.join("env.tfstate").display()),
        format!("-var-file={}", dir.join("env.tfvars.json").display()),
    ] {
        assert!(d.args.contains(&want), "{want} not in {:?}", d.args);
    }
    assert!(!dir.join("env.tfstate").exists()); // gone, so a second destroy skips the stage
}

// "TofuSandboxProvider with a drafted environment (two stages) keeps the runtime-roles guard: nothing is destroyed while agent roles remain"
#[tokio::test]
async fn keeps_the_runtime_roles_guard() {
    let r = rig();
    r.provider.apply(&input(Some(env()))).await.unwrap();
    r.runner.clear();
    let shared = SharedRoles::default();
    shared.set(vec!["role-1".into()]);
    let guarded = TofuSandboxProvider::new(
        TofuSandboxProviderOptions::new("/opt/tofu", module_dir(), r.root.path(), Arc::new(shared))
            .runner(r.runner.clone()),
    );
    let e = guarded.destroy(&input(None)).await.unwrap_err();
    assert!(matches!(e, PortError::TeardownBlocked(_)), "{e:?}");
    assert!(r.runner.calls().is_empty());
}

// "TofuSandboxProvider with a drafted environment (two stages) outputs() of an applied environment sandbox includes the environment"
#[tokio::test]
async fn outputs_of_an_applied_environment_sandbox_includes_the_environment() {
    let r = rig();
    r.provider.apply(&input(Some(env()))).await.unwrap();
    let o = r.provider.outputs("envsbx").await.unwrap().unwrap();
    assert_eq!(
        o.environment.unwrap().machines["api"],
        SandboxEnvironmentMachine {
            id: "i-0api".into(),
            role: "app".into(),
            private_ip: "10.42.1.10".into()
        }
    );
    assert_eq!(o.artifacts_bucket, "ratchet-envsbx-artifacts");
}

// "TofuSandboxProvider with a drafted environment (two stages) plan lists the environment machines next to the sandbox changes (before the sandbox exists the stage cannot be planned for real)"
#[tokio::test]
async fn plan_lists_the_environment_machines_next_to_the_sandbox_changes() {
    let r = rig();
    let p = r.provider.plan(&input(Some(env()))).await.unwrap();
    assert_eq!(r.runner.trace(), t(&["sbx:init", "sbx:plan", "sbx:show"]));
    let addrs: Vec<&str> = p.changes.iter().map(|c| c.address.as_str()).collect();
    assert_eq!(
        addrs,
        [
            "aws_vpc.sandbox",
            "module.sandbox.aws_instance.machine[\"api\"]",
            "module.sandbox.aws_instance.machine[\"db\"]",
            "module.sandbox.aws_ebs_volume.data[\"db\"]"
        ]
    );
    assert!(
        p.summary.contains("includes the environment: 2 machines"),
        "{}",
        p.summary
    );
}

// "TofuSandboxProvider with a drafted environment (two stages) without an environment nothing changes: the standard module only"
#[tokio::test]
async fn without_an_environment_nothing_changes() {
    let r = rig();
    let out = r.provider.apply(&input(None)).await.unwrap();
    assert_eq!(
        r.runner.trace(),
        t(&[
            "sbx:init",
            "sbx:plan",
            "sbx:show",
            "sbx:apply",
            "sbx:output"
        ])
    );
    assert!(out.environment.is_none());
}

// ---- a custom module ---------------------------------------------------------------------------------------------

const CUSTOM_FILES: [(&str, &str); 1] = [("main.tf", "resource \"aws_s3_bucket\" \"a\" {}\n")];

// "TofuSandboxProvider with a custom module (infra:) runs tofu in the materialized custom module instead of infra/aws, and validates its outputs against the contract"
#[tokio::test]
async fn custom_runs_tofu_in_the_materialized_module_and_validates_its_outputs() {
    let r = rig_with(vec![], None);
    r.provider
        .apply(&input(Some(custom(&CUSTOM_FILES))))
        .await
        .unwrap();
    let wheres: std::collections::BTreeSet<String> = r
        .runner
        .trace()
        .iter()
        .map(|t| t.split(':').next().unwrap().to_string())
        .collect();
    assert_eq!(wheres, ["custom".to_string()].into());
    let dir = r.root.path().join("envsbx").join("custom-module");
    assert_eq!(r.runner.calls()[0].cwd, dir);
    assert!(
        std::fs::read_to_string(dir.join("main.tf"))
            .unwrap()
            .contains("aws_s3_bucket")
    );
}

// "TofuSandboxProvider with a custom module (infra:) fails apply with the list of contract problems even though validateContract was not set (a custom module is always checked)"
#[tokio::test]
async fn custom_is_always_checked_against_the_contract() {
    let r = rig_with(vec![], None);
    {
        let mut o = sbx_out();
        o.as_object_mut().unwrap().remove("confirm_identity_ref");
        *r.runner.sandbox_outputs.lock().unwrap() = o;
    }
    let e = r
        .provider
        .apply(&input(Some(custom(&CUSTOM_FILES))))
        .await
        .unwrap_err();
    let msg = e.to_string();
    assert!(
        msg.contains("does not satisfy the sandbox contract"),
        "{msg}"
    );
    assert!(msg.contains("confirm_identity_ref"), "{msg}");
    // ... while the module that ships with the app is not re-checked on every apply
    let shipped = rig_with(vec![], None);
    let mut plain = input(None);
    plain.sandbox_id = "envsbx".into();
    shipped.provider.apply(&plain).await.unwrap();
}

// "TofuSandboxProvider with a custom module (infra:) destroys and reads outputs from the same custom module"
#[tokio::test]
async fn custom_destroys_and_reads_outputs_from_the_same_module() {
    let r = rig_with(vec![], None);
    r.provider
        .apply(&input(Some(custom(&CUSTOM_FILES))))
        .await
        .unwrap();
    r.runner.clear();
    r.provider.destroy(&input(None)).await.unwrap();
    assert_eq!(r.runner.trace(), t(&["custom:init", "custom:destroy"]));
}

// "TofuSandboxProvider with a custom module (infra:) refuses a module that can run commands before running tofu"
#[tokio::test]
async fn custom_refuses_a_module_that_can_run_commands_before_running_tofu() {
    let r = rig_with(vec![], None);
    let bad = custom(&[("main.tf", "data \"external\" \"x\" { program = [\"sh\"] }")]);
    let e = r.provider.apply(&input(Some(bad))).await.unwrap_err();
    assert!(e.to_string().contains("can run commands"), "{e}");
    assert!(r.runner.calls().is_empty());
}

// ---- the mock ----------------------------------------------------------------------------------------------------

// "MockSandboxProvider with a drafted environment applies it like the real provider (same refusal), reports the machines and plans them"
#[tokio::test]
async fn mock_applies_it_like_the_real_provider_reports_the_machines_and_plans_them() {
    let mock = MockSandboxProvider::default();
    let out = mock.apply(&input(Some(env()))).await.unwrap();
    let e = out.environment.unwrap();
    assert_eq!(
        e.machines.keys().cloned().collect::<Vec<_>>(),
        ["api", "db"]
    );
    assert_eq!(e.machines["api"].id, "i-0mockenv0000");
    assert_eq!(e.machines["db"].private_ip, "10.42.1.11");
    assert!(
        e.resources
            .contains(&"module.sandbox.aws_ebs_volume.data[\"db\"]".to_string())
    );
    let plan = MockSandboxProvider::default()
        .plan(&input(Some(env())))
        .await
        .unwrap();
    assert!(plan.changes.iter().any(|c| c.address
        == "module.sandbox.aws_instance.machine[\"api\"]"
        && c.resource_type == "aws_instance"
        && c.action == PlanAction::Create));
    let e = MockSandboxProvider::default()
        .apply(&input(Some(env_of(&[("main.tf", "locals {}\n")]))))
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("does not satisfy the sandbox contract"),
        "{e}"
    );
}

// ---- the checker -------------------------------------------------------------------------------------------------

struct FakeTofu {
    calls: Mutex<Vec<ProcessSpec>>,
    fail: Option<&'static str>,
}

#[async_trait]
impl ProcessRunner for FakeTofu {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessResult> {
        self.calls.lock().unwrap().push(spec.clone());
        let sub = spec.args[0].as_str();
        if self.fail == Some(sub) {
            return Ok(ProcessResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("Error: {sub} exploded"),
            });
        }
        Ok(ProcessResult {
            // `plan -detailed-exitcode` answers 2 when there are changes: success
            exit_code: if sub == "plan" { 2 } else { 0 },
            stdout: if sub == "plan" {
                "Plan: 2 to add, 0 to change, 0 to destroy.".into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

fn checker(root: &Path, fail: Option<&'static str>) -> (Arc<FakeTofu>, TofuEnvironmentChecker) {
    let runner = Arc::new(FakeTofu {
        calls: Mutex::default(),
        fail,
    });
    let mut o = TofuEnvironmentCheckerOptions::new("/opt/tofu", module_dir());
    o.work_root = Some(root.to_path_buf());
    o.runner = runner.clone();
    o.image_version = Some("0.4.0".into());
    (runner, TofuEnvironmentChecker::new(o))
}

fn draft_files() -> Vec<EnvironmentFile> {
    vec![
        EnvironmentFile::new("infra/ratchet/main.tf", MAIN_TF),
        EnvironmentFile::new("infra/ratchet/api.sh", "#!/bin/bash\n"),
    ]
}

fn plan_input(profile: Option<&str>, region: &str) -> EnvironmentPlanInput {
    EnvironmentPlanInput {
        scope: EnvironmentScope::Full,
        files: draft_files(),
        region: region.into(),
        aws_profile: profile.map(str::to_owned),
    }
}

// "TofuEnvironmentChecker (tofu validate + plan in the user account) runs init, validate and a no-refresh plan in the selected profile and region"
#[tokio::test]
async fn checker_runs_init_validate_and_a_no_refresh_plan_in_the_selected_profile_and_region() {
    let root = tempfile::tempdir().unwrap();
    let (runner, c) = checker(root.path(), None);
    let r = c
        .plan(&plan_input(Some("acme-prod"), "eu-west-1"))
        .await
        .unwrap();
    assert!(r.ok, "{}", r.detail);
    assert_eq!(
        r.detail,
        "tofu validate and plan succeeded (2 resources to add) in eu-west-1"
    );
    let calls = runner.calls.lock().unwrap().clone();
    assert_eq!(
        calls.iter().map(|c| c.args[0].as_str()).collect::<Vec<_>>(),
        ["init", "validate", "plan"]
    );
    for want in ["-refresh=false", "-lock=false", "-detailed-exitcode"] {
        assert!(calls[2].args.contains(&want.to_string()), "{want}");
    }
    assert_eq!(calls[2].env["AWS_PROFILE"], "acme-prod");
    assert_eq!(calls[2].env["AWS_REGION"], "eu-west-1");
    assert!(calls[0].args.contains(&"-backend=false".to_string()));
    assert!(!calls[0].cwd.exists()); // the scratch directory is removed
}

// "TofuEnvironmentChecker (tofu validate + plan in the user account) reports a failing %s with the reason and does not claim a plan"
#[tokio::test]
async fn checker_reports_a_failing_step_with_the_reason_and_does_not_claim_a_plan() {
    for sub in ["init", "validate", "plan"] {
        let root = tempfile::tempdir().unwrap();
        let (_runner, c) = checker(root.path(), Some(sub));
        let r = c.plan(&plan_input(Some("p"), "us-east-1")).await.unwrap();
        assert!(!r.ok, "{sub}");
        assert!(
            r.detail.contains(&format!("tofu {sub} failed")),
            "{}",
            r.detail
        );
        assert!(
            r.detail.contains(&format!("{sub} exploded")),
            "{}",
            r.detail
        );
    }
}

// "TofuEnvironmentChecker (tofu validate + plan in the user account) rejects a draft that violates the contract without calling tofu"
#[tokio::test]
async fn checker_rejects_a_draft_that_violates_the_contract_without_calling_tofu() {
    let root = tempfile::tempdir().unwrap();
    let (runner, c) = checker(root.path(), None);
    let mut input = plan_input(None, "us-east-1");
    input.files = vec![EnvironmentFile::new("infra/ratchet/main.tf", "locals {}\n")];
    let r = c.plan(&input).await.unwrap();
    assert!(!r.ok);
    assert!(r.detail.contains("missing the base module"), "{}", r.detail);
    assert!(runner.calls.lock().unwrap().is_empty());
}

// "a materialized drafted environment with the real tofu passes tofu init and tofu validate" (opt-in:
// RATCHET_TOFU_BIN=/path/to/tofu; downloads the AWS provider on first run)
#[tokio::test]
async fn a_materialized_drafted_environment_passes_the_real_tofu_init_and_validate() {
    let Some(bin) = std::env::var_os("RATCHET_TOFU_BIN")
        .map(PathBuf::from)
        .filter(|p| p.exists())
    else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let dest =
        materialize_drafted_environment(&env(), &base_module_dir(), &root.path().join("real"))
            .await
            .unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&bin)
            .args(args)
            .current_dir(&dest)
            .env("TF_DATA_DIR", root.path().join(".data"))
            .env("CHECKPOINT_DISABLE", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    run(&["init", "-backend=false", "-input=false", "-no-color"]);
    assert!(run(&["validate", "-no-color"]).contains("valid"));
}
