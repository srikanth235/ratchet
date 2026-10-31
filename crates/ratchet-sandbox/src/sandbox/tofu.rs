//! `TofuSandboxProvider`: drives the `tofu` binary as a subprocess. (TS `sandbox/tofu.ts`)
//!
//! Per sandbox there is one working directory `<state_root>/<sandbox_id>/` holding the local state, the saved plan, the
//! contract-inputs var file and `TF_DATA_DIR`. Operations on one sandbox are serialized; different sandboxes run in
//! parallel. `apply` always applies the saved plan it just made (never `-auto-approve`); `destroy` refuses while
//! runtime-created agent roles exist (the boundary policy cannot be deleted while roles still reference it).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex as AsyncMutex;

use super::infra_override::{ContractCheckOptions, assert_sandbox_contract};
use super::process::{ProcessResult, ProcessRunner, ProcessSpec, TokioProcessRunner};
use crate::error::{Error, Result};
use crate::util::{assert_valid_sandbox_id, js_number};
use ratchet_core::ports::{
    AwsIdentityRef, PlanAction, PlanChange, PortError, PortResult, SandboxInput, SandboxLimits,
    SandboxNetwork, SandboxOutputs, SandboxPlan, SandboxProvider,
};

/// The TypeScript name of [`SandboxOutputs`] in `packages/aws`: core's outputs type is the full AWS shape.
pub type AwsSandboxOutputs = SandboxOutputs;

/// A failed `tofu` invocation.
#[derive(Debug, Clone)]
pub struct TofuError {
    pub args: Vec<String>,
    pub result: ProcessResult,
}

fn last_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    let skip = count - n;
    let idx = s.char_indices().nth(skip).map_or(0, |(i, _)| i);
    &s[idx..]
}

impl fmt::Display for TofuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stderr = last_chars(self.result.stderr.trim(), 2000);
        let detail = if stderr.is_empty() {
            last_chars(self.result.stdout.trim(), 500)
        } else {
            stderr
        };
        write!(
            f,
            "tofu {} failed (exit {}): {detail}",
            self.args.first().map_or("", String::as_str),
            self.result.exit_code
        )
    }
}

impl std::error::Error for TofuError {}

/// `destroy` was refused because runtime-created agent roles still exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeardownBlockedError {
    pub roles: Vec<String>,
}

impl fmt::Display for TeardownBlockedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing to destroy sandbox: {} runtime agent role(s) still exist ({}). Teardown order: kill switch -> terminate tagged instances -> delete parameters -> delete agent roles/profiles -> tofu destroy.",
            self.roles.len(),
            self.roles.join(", ")
        )
    }
}

impl std::error::Error for TeardownBlockedError {}

/// Teardown guard (infra/README.md): the runtime-created agent roles that still exist for this sandbox.
#[async_trait]
pub trait RuntimeRoleLister: Send + Sync {
    async fn list_runtime_agent_roles(&self, input: &SandboxInput) -> Result<Vec<String>>;
}

/// Lists every IAM role under the agent role path.
pub struct IamRoleLister {
    iam: aws_sdk_iam::Client,
    agent_role_path: String,
}

impl IamRoleLister {
    pub fn new(iam: aws_sdk_iam::Client, agent_role_path: impl Into<String>) -> Self {
        Self {
            iam,
            agent_role_path: agent_role_path.into(),
        }
    }
}

#[async_trait]
impl RuntimeRoleLister for IamRoleLister {
    async fn list_runtime_agent_roles(&self, _input: &SandboxInput) -> Result<Vec<String>> {
        crate::compute::aws::list_agent_roles(&self.iam, &self.agent_role_path).await
    }
}

/// A role list you can change from outside (simulation and tests).
#[derive(Debug, Clone, Default)]
pub struct SharedRoles(pub Arc<Mutex<Vec<String>>>);

impl SharedRoles {
    pub fn set(&self, roles: Vec<String>) {
        *self.0.lock().expect("roles poisoned") = roles;
    }
}

#[async_trait]
impl RuntimeRoleLister for SharedRoles {
    async fn list_runtime_agent_roles(&self, _input: &SandboxInput) -> Result<Vec<String>> {
        Ok(self.0.lock().expect("roles poisoned").clone())
    }
}

pub struct TofuSandboxProviderOptions {
    /// Path to the bundled, pinned `tofu` binary.
    pub tofu_binary: PathBuf,
    /// Directory holding the OpenTofu module (`infra/aws`).
    pub module_dir: PathBuf,
    /// Root for per-sandbox working dirs: `<state_root>/<sandbox_id>/`.
    pub state_root: PathBuf,
    pub role_lister: Arc<dyn RuntimeRoleLister>,
    pub runner: Arc<dyn ProcessRunner>,
    /// Extra env for every tofu invocation (e.g. `AWS_PROFILE`).
    pub env: BTreeMap<String, String>,
    /// Validate the module's outputs against the sandbox contract (infra/contract.md) after every apply and when outputs
    /// are read, failing with a `ContractViolationError` that lists every problem. Off (`None`) for the module that
    /// ships with the app (its own tests pin the contract); ON for a custom module (`infra:` in ratchet.yaml).
    pub validate_contract: Option<ContractCheckOptions>,
}

impl TofuSandboxProviderOptions {
    pub fn new(
        tofu_binary: impl Into<PathBuf>,
        module_dir: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
        role_lister: Arc<dyn RuntimeRoleLister>,
    ) -> Self {
        Self {
            tofu_binary: tofu_binary.into(),
            module_dir: module_dir.into(),
            state_root: state_root.into(),
            role_lister,
            runner: Arc::new(TokioProcessRunner),
            env: BTreeMap::new(),
            validate_contract: None,
        }
    }

    pub fn runner(mut self, runner: Arc<dyn ProcessRunner>) -> Self {
        self.runner = runner;
        self
    }

    pub fn env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub fn validate_contract(mut self, opts: ContractCheckOptions) -> Self {
        self.validate_contract = Some(opts);
        self
    }
}

const COMMON: [&str; 2] = ["-input=false", "-no-color"];

struct Ctx {
    input_region: String,
    dir: PathBuf,
    vars_path: PathBuf,
    state_path: PathBuf,
    plan_path: PathBuf,
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn args(parts: &[&str], more: Vec<String>) -> Vec<String> {
    parts.iter().map(|p| (*p).to_string()).chain(more).collect()
}

pub struct TofuSandboxProvider {
    opts: TofuSandboxProviderOptions,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl TofuSandboxProvider {
    pub fn new(opts: TofuSandboxProviderOptions) -> Self {
        Self {
            opts,
            locks: Mutex::default(),
        }
    }

    /// `<state_root>/<sandbox_id>`: state, plan file, tfvars and `TF_DATA_DIR` for one sandbox.
    pub fn work_dir(&self, sandbox_id: &str) -> Result<PathBuf> {
        assert_valid_sandbox_id(sandbox_id)?;
        Ok(self.opts.state_root.join(sandbox_id))
    }

    /// Serialize operations per sandbox so two tofu runs never share a state file. The id is validated first.
    async fn lock(&self, sandbox_id: &str) -> Result<tokio::sync::OwnedMutexGuard<()>> {
        assert_valid_sandbox_id(sandbox_id)?;
        let m = self
            .locks
            .lock()
            .expect("locks poisoned")
            .entry(sandbox_id.to_string())
            .or_default()
            .clone();
        Ok(m.lock_owned().await)
    }

    async fn prepare(&self, input: &SandboxInput) -> Result<Ctx> {
        assert_valid_sandbox_id(&input.sandbox_id)?;
        if input.allowed_instance_types.is_empty() {
            return Err(Error::invalid("allowedInstanceTypes must not be empty"));
        }
        let dir = self.work_dir(&input.sandbox_id)?;
        tokio::fs::create_dir_all(&dir).await?;
        let vars_path = dir.join("vars.tfvars.json");
        // Contract inputs only; no secrets ever reach OpenTofu (contract invariant 11).
        let vars = json!({
            "sandbox_id": input.sandbox_id,
            "region": input.region,
            "allowed_instance_types": input.allowed_instance_types,
            "max_instances_per_agent": input.max_instances_per_agent,
            "max_instance_lifetime_hours": js_number(input.max_instance_lifetime_hours),
            "tags": input.tags.clone().unwrap_or_default(),
        });
        tokio::fs::write(&vars_path, serde_json::to_string_pretty(&vars)?).await?;
        Ok(Ctx {
            input_region: input.region.clone(),
            state_path: dir.join("terraform.tfstate"),
            plan_path: dir.join("plan.tfplan"),
            vars_path,
            dir,
        })
    }

    async fn init(&self, ctx: &Ctx) -> Result<()> {
        self.tofu(
            ctx,
            args(&["init", COMMON[0], COMMON[1], "-backend=false"], vec![]),
            &[0],
        )
        .await?;
        Ok(())
    }

    async fn make_plan(&self, ctx: &Ctx) -> Result<SandboxPlan> {
        // -detailed-exitcode: 0 = no changes, 2 = changes present; both are success.
        self.tofu(
            ctx,
            args(
                &["plan", COMMON[0], COMMON[1], "-detailed-exitcode"],
                vec![
                    format!("-out={}", s(&ctx.plan_path)),
                    format!("-state={}", s(&ctx.state_path)),
                    format!("-var-file={}", s(&ctx.vars_path)),
                ],
            ),
            &[0, 2],
        )
        .await?;
        let shown = self
            .tofu(
                ctx,
                args(&["show", "-json", "-no-color"], vec![s(&ctx.plan_path)]),
                &[0],
            )
            .await?;
        summarize_plan(&shown.stdout)
    }

    async fn read_outputs(&self, ctx: &Ctx) -> Result<Option<SandboxOutputs>> {
        let res = self
            .tofu(
                ctx,
                args(
                    &["output", "-json", "-no-color"],
                    vec![format!("-state={}", s(&ctx.state_path))],
                ),
                &[0],
            )
            .await?;
        if let Some(v) = &self.opts.validate_contract {
            let non_empty = serde_json::from_str::<Value>(&res.stdout)?
                .as_object()
                .is_some_and(|o| !o.is_empty());
            if non_empty {
                assert_sandbox_contract(&res.stdout, v)?;
            }
        }
        parse_tofu_outputs(&res.stdout, Some(&ctx.input_region))
    }

    async fn tofu(&self, ctx: &Ctx, args: Vec<String>, ok_codes: &[i32]) -> Result<ProcessResult> {
        let mut env = self.opts.env.clone();
        env.insert("TF_DATA_DIR".into(), s(&ctx.dir.join(".terraform-data")));
        env.insert("TF_IN_AUTOMATION".into(), "1".into());
        env.insert("TF_INPUT".into(), "0".into());
        env.insert("CHECKPOINT_DISABLE".into(), "1".into());
        let spec = ProcessSpec {
            command: s(&self.opts.tofu_binary),
            args: args.clone(),
            cwd: self.opts.module_dir.clone(),
            env,
        };
        tracing::debug!(subcommand = %args.first().map_or("", String::as_str), "tofu");
        let result = self.opts.runner.run(&spec).await?;
        if !ok_codes.contains(&result.exit_code) {
            return Err(TofuError { args, result }.into());
        }
        Ok(result)
    }
}

#[async_trait]
impl SandboxProvider for TofuSandboxProvider {
    async fn plan(&self, input: &SandboxInput) -> PortResult<SandboxPlan> {
        let _g = self.lock(&input.sandbox_id).await?;
        let ctx = self.prepare(input).await?;
        self.init(&ctx).await?;
        Ok(self.make_plan(&ctx).await?)
    }

    async fn apply(&self, input: &SandboxInput) -> PortResult<SandboxOutputs> {
        let _g = self.lock(&input.sandbox_id).await?;
        let ctx = self.prepare(input).await?;
        self.init(&ctx).await?;
        self.make_plan(&ctx).await?;
        // The saved plan only: what was shown is what is applied.
        self.tofu(
            &ctx,
            args(
                &["apply", COMMON[0], COMMON[1]],
                vec![format!("-state={}", s(&ctx.state_path)), s(&ctx.plan_path)],
            ),
            &[0],
        )
        .await?;
        let _ = tokio::fs::remove_file(&ctx.plan_path).await;
        self.read_outputs(&ctx).await?.ok_or_else(|| {
            PortError::failed("tofu apply succeeded but produced no contract outputs")
        })
    }

    async fn destroy(&self, input: &SandboxInput) -> PortResult<()> {
        let _g = self.lock(&input.sandbox_id).await?;
        let roles = self
            .opts
            .role_lister
            .list_runtime_agent_roles(input)
            .await?;
        if !roles.is_empty() {
            return Err(PortError::TeardownBlocked(
                TeardownBlockedError { roles }.to_string(),
            ));
        }
        let ctx = self.prepare(input).await?;
        self.init(&ctx).await?;
        self.tofu(
            &ctx,
            args(
                &["destroy", COMMON[0], COMMON[1], "-auto-approve"],
                vec![
                    format!("-state={}", s(&ctx.state_path)),
                    format!("-var-file={}", s(&ctx.vars_path)),
                ],
            ),
            &[0],
        )
        .await?;
        let _ = tokio::fs::remove_file(&ctx.plan_path).await;
        Ok(())
    }

    async fn outputs(&self, sandbox_id: &str) -> PortResult<Option<SandboxOutputs>> {
        let _g = self.lock(sandbox_id).await?;
        let dir = self.work_dir(sandbox_id)?;
        let state_path = dir.join("terraform.tfstate");
        if tokio::fs::metadata(&state_path).await.is_err() {
            return Ok(None);
        }
        let vars_path = dir.join("vars.tfvars.json");
        // vars file missing or unreadable: region unknown
        let region = match tokio::fs::read_to_string(&vars_path).await {
            Ok(text) => serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("region").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        self.read_outputs(&Ctx {
            input_region: region,
            dir,
            vars_path,
            state_path,
            plan_path: PathBuf::new(),
        })
        .await
        .map_err(PortError::from)
    }
}

// ---- pure parsers -------------------------------------------------------------

#[derive(Deserialize)]
struct TfResourceChange {
    address: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(rename = "type")]
    resource_type: String,
    change: TfChange,
}

#[derive(Deserialize)]
struct TfChange {
    actions: Vec<String>,
}

#[derive(Deserialize)]
struct TfPlan {
    #[serde(default)]
    resource_changes: Vec<TfResourceChange>,
}

pub fn map_actions(actions: &[String]) -> PlanAction {
    let has = |a: &str| actions.iter().any(|x| x == a);
    if actions.len() == 2 && has("create") && has("delete") {
        return PlanAction::Replace;
    }
    if actions.len() == 1 {
        return match actions[0].as_str() {
            "create" => PlanAction::Create,
            "update" => PlanAction::Update,
            "delete" => PlanAction::Delete,
            _ => PlanAction::NoOp, // no-op, read
        };
    }
    PlanAction::Update
}

/// `tofu show -json <plan>` -> plan changes (managed resources only) and the one-line summary.
pub fn summarize_plan(show_json: &str) -> Result<SandboxPlan> {
    let doc: TfPlan = serde_json::from_str(show_json)?;
    let changes: Vec<PlanChange> = doc
        .resource_changes
        .into_iter()
        .filter(|r| r.mode.as_deref().unwrap_or("managed") == "managed")
        .map(|r| PlanChange {
            action: map_actions(&r.change.actions),
            address: r.address,
            resource_type: r.resource_type,
        })
        .collect();
    let n = |a: PlanAction| changes.iter().filter(|c| c.action == a).count();
    let replace = n(PlanAction::Replace);
    let summary = format!(
        "Plan: {} to add, {} to change, {} to destroy.",
        n(PlanAction::Create) + replace,
        n(PlanAction::Update),
        n(PlanAction::Delete) + replace
    );
    Ok(SandboxPlan { changes, summary })
}

fn missing(k: &str) -> Error {
    Error::other(format!("tofu output \"{k}\" missing (infra/contract.md)"))
}

fn identity(k: &str, v: &Value) -> Result<AwsIdentityRef> {
    let f = |name: &str| {
        v.get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| missing(&format!("{k}.{name}")))
    };
    Ok(AwsIdentityRef {
        role_arn: f("role_arn")?,
        role_name: f("role_name")?,
        instance_profile_arn: f("instance_profile_arn")?,
        instance_profile_name: f("instance_profile_name")?,
    })
}

/// `tofu output -json` -> contract outputs (infra/contract.md). `None` when nothing is applied.
pub fn parse_tofu_outputs(json: &str, region: Option<&str>) -> Result<Option<SandboxOutputs>> {
    let doc: Map<String, Value> = match serde_json::from_str::<Value>(json)? {
        Value::Object(m) => m,
        _ => return Err(Error::other("tofu output -json is not an object")),
    };
    if doc.is_empty() {
        return Ok(None);
    }
    let get = |k: &str| -> Result<&Value> {
        doc.get(k)
            .and_then(|o| o.get("value"))
            .ok_or_else(|| missing(k))
    };
    let string = |k: &str| -> Result<String> {
        get(k)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| missing(k))
    };
    let eval = get("eval_identity_ref")?;
    let confirm = get("confirm_identity_ref")?;
    // Optional: sandboxes applied before dependency snapshots have no such output.
    let deps = doc
        .get("deps_identity_ref")
        .and_then(|o| o.get("value"))
        .filter(|v| !v.is_null());
    let net = get("network_ref")?;
    let limits = get("limits")?;
    let net_str = |name: &str| net.get(name).and_then(Value::as_str).map(str::to_string);
    let net_field =
        |name: &str| net_str(name).ok_or_else(|| missing(&format!("network_ref.{name}")));
    let sg = |name: &str| {
        net.get("security_group_ids")
            .and_then(|g| g.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| missing(&format!("network_ref.security_group_ids.{name}")))
    };
    let eval_identity = identity("eval_identity_ref", eval)?;
    let max_instances = limits
        .get("max_instances_per_agent")
        .and_then(|v| {
            v.as_u64().or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
        })
        .ok_or_else(|| missing("limits.max_instances_per_agent"))?;
    let max_hours = limits
        .get("max_instance_lifetime_hours")
        .and_then(Value::as_f64)
        .ok_or_else(|| missing("limits.max_instance_lifetime_hours"))?;
    Ok(Some(SandboxOutputs {
        artifacts_bucket: string("artifacts_bucket")?,
        sealed_bucket: string("sealed_bucket")?,
        agent_boundary_ref: string("agent_boundary_ref")?,
        // The port carries a single string; the bench launcher needs the instance profile.
        eval_identity_ref: eval_identity.instance_profile_arn.clone(),
        agent_role_path: string("agent_role_path")?,
        network: SandboxNetwork {
            subnet_id: net_field("subnet_id")?,
            agent_runner_security_group_id: sg("agent_runner")?,
            bench_security_group_id: sg("bench")?,
            bench_subnet_id: net_field("bench_subnet_id")?,
            egress_proxy_url: net_str("egress_proxy_url"),
        },
        region: region.unwrap_or("").to_string(),
        vpc_id: net_field("vpc_id")?,
        eval_identity,
        confirm_identity: identity("confirm_identity_ref", confirm)?,
        deps_identity: deps.map(|d| identity("deps_identity_ref", d)).transpose()?,
        sealed_confirm_prefix: string("sealed_confirm_prefix")?,
        limits: SandboxLimits {
            max_instances_per_agent: max_instances as u32,
            max_instance_lifetime_hours: max_hours,
        },
        environment: None,
    }))
}
