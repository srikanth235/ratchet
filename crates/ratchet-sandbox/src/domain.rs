//! Domain types this crate shares with the rest of Ratchet.
//!
//! TODO(core): replace with ratchet-core. Everything here is a minimal local copy of the types in
//! `packages/core/src/ports/index.ts` and `packages/core/src/ledger/schema.ts`, with serde attributes chosen so the
//! JSON is byte-compatible with the TypeScript shapes (camelCase fields, the same string enums). Unifying with
//! ratchet-core later is then a mechanical swap of the `use` paths.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub type OptimizationId = String;
pub type AgentId = String;

// ---------------------------------------------------------------- clock

/// Time source and sleeper, injected so tests and simulation never wait for real time.
/// (TS `Clock`; the abort signal is dropped: cancel by dropping the future.)
#[async_trait]
pub trait Clock: Send + Sync {
    /// Epoch milliseconds.
    fn now_ms(&self) -> i64;
    async fn sleep(&self, ms: u64);
}

// ---------------------------------------------------------------- ledger enums

/// `AGENT_KINDS` of the ledger schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "mock")]
    Mock,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Mock => "mock",
        }
    }
}

/// `SPEND_CATEGORIES` of the ledger schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendCategory {
    AgentRunner,
    Experiment,
    BenchRunner,
    StorageEgress,
    Tokens,
}

/// `SANDBOX_STATUSES` of the ledger schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxStatus {
    Planned,
    Applying,
    Applied,
    Destroying,
    Destroyed,
    Failed,
}

/// `SANDBOX_PROVIDERS` of the ledger schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxProviderKind {
    Aws,
    Mock,
}

/// `creating` -> `ready` -> (`idle` while no run is active) -> `destroying` -> `destroyed`; `failed` when a step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxPhase {
    Creating,
    Ready,
    Idle,
    Destroying,
    Destroyed,
    Failed,
}

// ---------------------------------------------------------------- sandbox (OpenTofu)

/// Whole `f64`s serialize as JSON integers (`2`, not `2.0`) like JavaScript numbers do.
pub(crate) fn ser_js_number<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

pub(crate) fn de_js_number<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    f64::deserialize(d)
}

/// A JSON number the way JavaScript would print `v`.
pub(crate) fn js_number(v: f64) -> serde_json::Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        serde_json::Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

/// The contract inputs of a sandbox (infra/contract.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxInput {
    pub sandbox_id: String,
    pub region: String,
    pub allowed_instance_types: Vec<String>,
    pub max_instances_per_agent: u32,
    #[serde(serialize_with = "ser_js_number", deserialize_with = "de_js_number")]
    pub max_instance_lifetime_hours: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<BTreeMap<String, String>>,
}

/// AWS-side identity of a role + instance profile pair (`eval_identity_ref`, `confirm_identity_ref`, `deps_identity_ref`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsIdentityRef {
    pub role_arn: String,
    pub role_name: String,
    pub instance_profile_arn: String,
    pub instance_profile_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxNetwork {
    pub subnet_id: String,
    pub agent_runner_security_group_id: String,
    pub bench_security_group_id: String,
    /// Private bench subnet: no route to the internet.
    pub bench_subnet_id: String,
    /// `None` when the egress proxy is disabled.
    pub egress_proxy_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxLimits {
    pub max_instances_per_agent: u32,
    #[serde(serialize_with = "ser_js_number", deserialize_with = "de_js_number")]
    pub max_instance_lifetime_hours: f64,
}

/// Mirrors infra/contract.md outputs. (TS: core's `SandboxOutputs` plus the `AwsSandboxOutputs` extension, merged:
/// the engine only ever stores and reads the AWS shape.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxOutputs {
    pub artifacts_bucket: String,
    pub sealed_bucket: String,
    pub agent_boundary_ref: String,
    /// The bench instance profile ARN (the TS port carried a single string for `eval_identity_ref`).
    pub eval_identity_ref: String,
    pub agent_role_path: String,
    pub network: SandboxNetwork,
    pub region: String,
    pub vpc_id: String,
    /// Bench runner for baseline/bench phases (`eval_identity_ref`): NO sealed access.
    pub eval_identity: AwsIdentityRef,
    /// Confirm runner (`confirm_identity_ref`): the only identity that reads the sealed bucket. Confirm runs only.
    pub confirm_identity: AwsIdentityRef,
    /// Dependency-snapshot builder (`deps_identity_ref`). Absent in sandboxes applied before dependency snapshots existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deps_identity: Option<AwsIdentityRef>,
    /// Key prefix of the confirm traces in the sealed bucket.
    pub sealed_confirm_prefix: String,
    pub limits: SandboxLimits,
}

/// TS name of [`SandboxOutputs`] in `packages/aws`.
pub type AwsSandboxOutputs = SandboxOutputs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanAction {
    Create,
    Update,
    Delete,
    Replace,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanChange {
    pub action: PlanAction,
    pub address: String,
    #[serde(rename = "type")]
    pub resource_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPlan {
    pub changes: Vec<PlanChange>,
    pub summary: String,
}

// ---------------------------------------------------------------- cloud compute + identity

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdentity {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub role_ref: String,
    pub instance_profile_ref: String,
}

/// An SSM parameter reference for a secret (`name` e.g. `ANTHROPIC_API_KEY`, `ref` e.g. `/ratchet/<opt>/<agent>/NAME`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    pub name: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceRole {
    AgentRunner,
    Experiment,
    Bench,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    Pending,
    Running,
    Stopping,
    Stopped,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub id: String,
    pub role: InstanceRole,
    pub optimization_id: OptimizationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub instance_type: String,
    pub state: InstanceState,
    /// Epoch ms.
    pub launched_at: i64,
    /// Epoch ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerLaunchSpec {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub kind: AgentKind,
    pub instance_type: String,
    pub image_ref: String,
    pub identity: AgentIdentity,
    pub secrets: Vec<SecretRef>,
    pub repo_url: String,
    pub branch: String,
}

/// `deps` is the dependency-snapshot job: it runs on an instance that HAS egress (the agent subnet, through the proxy
/// when enabled) under its own minimal identity, and never executes candidate code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BenchPurpose {
    Baseline,
    Bench,
    Confirm,
    Deps,
}

impl BenchPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Bench => "bench",
            Self::Confirm => "confirm",
            Self::Deps => "deps",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchLaunchSpec {
    pub optimization_id: OptimizationId,
    pub instance_type: String,
    pub image_ref: String,
    pub purpose: BenchPurpose,
}

/// A shell script run on an instance (SSM `AWS-RunShellScript`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommand {
    pub script: String,
    /// Exported before the script; names must be shell identifiers. (Sorted by name, unlike the TS insertion order.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl RemoteCommand {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            env: None,
            timeout_seconds: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

// ---------------------------------------------------------------- cost reconciliation

/// What to ask the billing system about: everything tagged to one optimization within a time window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostActualsQuery {
    pub optimization_id: OptimizationId,
    /// Window start (epoch ms); implementations round down to their billing granularity (UTC day).
    pub start_ms: i64,
    /// Window end (epoch ms); implementations round up to their billing granularity.
    pub end_ms: i64,
}

/// Billed cost of the tagged resources, mapped to ledger categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostActuals {
    /// USD per ledger category (a category missing from the billing data is absent, not 0).
    pub by_category: BTreeMap<SpendCategory, f64>,
    /// Number of non-empty billing groups that came back; 0 means no billing data at all.
    pub groups: u32,
    /// False while the billing system flags the window as still being estimated / incomplete.
    pub complete: bool,
    /// Why `complete` is false, or any caveat worth showing the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
