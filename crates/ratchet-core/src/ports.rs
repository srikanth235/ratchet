//! Ports: every external system Ratchet talks to, as a trait.
//!
//! Each port has a real implementation (AWS, GitHub, agent CLIs, EC2 bench) in its own crate and a mock / local
//! one, so the whole product runs in `simulation` mode. See docs/REAL_ENV_TESTING.md for which mocks must be
//! re-tested for real. Port of `packages/core/src/ports/index.ts`.
//!
//! The traits use `async-trait` so that they stay object-safe: the engine holds them as `Arc<dyn Port>`.
//! Cancellation (`AbortSignal` in TypeScript) is a [`CancellationToken`]; callbacks (`onEvent`) are
//! [`EventSink`]s.

use std::collections::BTreeMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

pub use tokio_util::sync::CancellationToken;

use crate::bench::calibrate::Calibration;
use crate::config::{GuardrailResult, RatchetConfig};
use crate::enums::{
    AgentKind, AgentsWhere, CandidateVerdict, InstanceRole, Mode, SandboxProviderKind,
    SandboxStatus, SpendCategory,
};
use crate::ids::{AgentId, CandidateId, OptimizationId};
use crate::protected_set::EffectiveProtected;
use crate::templates::{RenderTemplateOptions, TemplateInfo};

/// What a port can fail with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    /// The provider refused a destroy before touching anything (`infra/README.md` teardown order): nothing
    /// changed. (`TeardownBlockedError` in TypeScript.)
    #[error("{0}")]
    TeardownBlocked(String),
    /// The port does not implement this optional operation.
    #[error("{0} is not supported by this implementation")]
    Unsupported(&'static str),
    /// The operation was cancelled through its [`CancellationToken`].
    #[error("cancelled")]
    Cancelled,
    /// A remote service (a code host's REST API, say) answered with an HTTP error status.
    #[error("{message}")]
    Http { status: u16, message: String },
    /// An agent CLI turn did not complete (non-zero exit, error result, missing session). `exit_code` and `stderr`
    /// are the process's, when there was one.
    #[error("{message}")]
    AgentFailed {
        message: String,
        exit_code: Option<i32>,
        stderr: Option<String>,
    },
    /// Anything else, with the message the underlying system gave.
    #[error("{0}")]
    Failed(String),
}

impl From<std::io::Error> for PortError {
    fn from(e: std::io::Error) -> Self {
        PortError::Failed(e.to_string())
    }
}

impl From<serde_json::Error> for PortError {
    fn from(e: serde_json::Error) -> Self {
        PortError::Failed(e.to_string())
    }
}

impl PortError {
    pub fn failed(msg: impl Into<String>) -> Self {
        PortError::Failed(msg.into())
    }

    /// An agent turn failed without a process exit to report.
    pub fn agent_failed(msg: impl Into<String>) -> Self {
        PortError::AgentFailed {
            message: msg.into(),
            exit_code: None,
            stderr: None,
        }
    }

    /// The HTTP status of a remote failure, if that is what this is (the TypeScript code read `err.status`).
    pub fn status(&self) -> Option<u16> {
        match self {
            PortError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// True for [`PortError::Cancelled`].
    pub fn is_cancelled(&self) -> bool {
        matches!(self, PortError::Cancelled)
    }
}

pub type PortResult<T> = Result<T, PortError>;

/// Where a stream of agent events goes (`onEvent` in TypeScript): a synchronous callback, so that events stay in
/// order and a sink can react to each one (budget tracking, artifact batching) before the adapter goes on. To
/// feed a channel use [`channel_sink`].
pub type EventSink<'a> = dyn Fn(AgentEvent) + Send + Sync + 'a;

/// An [`EventSink`] that can be shared and stored.
pub type SharedEventSink = std::sync::Arc<EventSink<'static>>;

/// A sink that sends every event into `tx` (events sent after the receiver is gone are dropped).
pub fn channel_sink(tx: mpsc::UnboundedSender<AgentEvent>) -> SharedEventSink {
    std::sync::Arc::new(move |event| {
        let _ = tx.send(event);
    })
}

// ---------------------------------------------------------------- clock

/// Time source of the engine. Injectable so tests control time.
#[async_trait]
pub trait Clock: Send + Sync {
    /// Epoch milliseconds.
    fn now(&self) -> i64;
    /// Sleep for `ms`, returning early (with `Err(Cancelled)`) when `cancel` fires.
    async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()>;
}

// ---------------------------------------------------------------- sandbox (OpenTofu)

/// The contract inputs of a sandbox (`infra/contract.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SandboxInput {
    pub sandbox_id: String,
    pub region: String,
    pub allowed_instance_types: Vec<String>,
    pub max_instances_per_agent: u32,
    /// A JSON number, not necessarily whole (`0.5` is half an hour). Whole values serialize without a fraction
    /// (`2`, not `2.0`), like JavaScript numbers.
    #[serde(serialize_with = "crate::numfmt::serialize_js_number")]
    pub max_instance_lifetime_hours: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<BTreeMap<String, String>>,
}

/// AWS-side identity of a role + instance profile pair (`eval_identity_ref`, `confirm_identity_ref`,
/// `deps_identity_ref` of the contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AwsIdentityRef {
    pub role_arn: String,
    pub role_name: String,
    pub instance_profile_arn: String,
    pub instance_profile_name: String,
}

/// The network part of the sandbox outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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

/// Limits echoed by the sandbox (the app reaper enforces them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SandboxLimits {
    pub max_instances_per_agent: u32,
    #[serde(serialize_with = "crate::numfmt::serialize_js_number")]
    pub max_instance_lifetime_hours: f64,
}

/// Mirrors the `infra/contract.md` outputs. (The TypeScript has the base `SandboxOutputs` in core and the
/// `AwsSandboxOutputs` extension in `packages/aws`; a Rust port trait cannot return a subtype, and the engine only
/// ever stores and reads the full shape, so they are one type here.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
    /// Dependency-snapshot builder (`deps_identity_ref`). Absent in sandboxes applied before dependency snapshots
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deps_identity: Option<AwsIdentityRef>,
    /// Key prefix of the confirm traces in the sealed bucket.
    pub sealed_confirm_prefix: String,
    pub limits: SandboxLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum PlanAction {
    Create,
    Update,
    Delete,
    Replace,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct PlanChange {
    pub action: PlanAction,
    pub address: String,
    #[serde(rename = "type")]
    pub resource_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SandboxPlan {
    pub changes: Vec<PlanChange>,
    pub summary: String,
}

#[async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn plan(&self, input: &SandboxInput) -> PortResult<SandboxPlan>;
    async fn apply(&self, input: &SandboxInput) -> PortResult<SandboxOutputs>;
    async fn destroy(&self, input: &SandboxInput) -> PortResult<()>;
    async fn outputs(&self, sandbox_id: &str) -> PortResult<Option<SandboxOutputs>>;
}

// ---------------------------------------------------------------- cloud compute + identity

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentIdentity {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub role_ref: String,
    pub instance_profile_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SecretRef {
    /// e.g. `ANTHROPIC_API_KEY`
    pub name: String,
    /// e.g. the SSM parameter name `/ratchet/<opt>/<agent>/ANTHROPIC_API_KEY`
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    Pending,
    Running,
    Stopping,
    Stopped,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub id: String,
    pub role: InstanceRole,
    pub optimization_id: OptimizationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub instance_type: String,
    pub state: InstanceState,
    pub launched_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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

/// Why a bench instance is launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum BenchLaunchPurpose {
    Baseline,
    Bench,
    Confirm,
    /// The dependency-snapshot job: it runs on an instance that HAS egress (the agent subnet, through the proxy
    /// when enabled) under its own minimal identity, and never executes candidate code.
    Deps,
}

impl BenchLaunchPurpose {
    /// The wire / tag string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Bench => "bench",
            Self::Confirm => "confirm",
            Self::Deps => "deps",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BenchLaunchSpec {
    pub optimization_id: OptimizationId,
    pub instance_type: String,
    pub image_ref: String,
    pub purpose: BenchLaunchPurpose,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommand {
    /// Shell script run on the instance (SSM RunShellScript).
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl RemoteCommand {
    /// A command with no extra environment and the default timeout.
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            env: None,
            timeout_seconds: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait CloudCompute: Send + Sync {
    async fn create_agent_identity(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<AgentIdentity>;
    /// Kill switch: attach deny-all. Must take effect immediately.
    async fn revoke_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()>;
    async fn delete_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()>;
    async fn put_secret(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
        name: &str,
        value: &str,
    ) -> PortResult<SecretRef>;
    async fn delete_secrets(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<()>;
    async fn launch_runner(&self, spec: &RunnerLaunchSpec) -> PortResult<InstanceInfo>;
    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> PortResult<InstanceInfo>;
    async fn terminate(&self, instance_ids: &[String]) -> PortResult<()>;
    /// Every instance carrying the ratchet tags (optionally for one optimization).
    async fn list_tagged(
        &self,
        optimization_id: Option<&OptimizationId>,
    ) -> PortResult<Vec<InstanceInfo>>;
    async fn run_command(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> PortResult<RemoteCommandResult>;
}

#[async_trait]
pub trait PriceBook: Send + Sync {
    /// On-demand USD per hour for an instance type in the sandbox region.
    async fn hourly_usd(&self, instance_type: &str) -> PortResult<f64>;
}

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()>;
    /// `None` when the key does not exist.
    async fn get(&self, key: &str) -> PortResult<Option<String>>;
    /// Sorted keys starting with `prefix`.
    async fn list(&self, prefix: &str) -> PortResult<Vec<String>>;

    /// [`ArtifactStore::put`] for text.
    async fn put_str(&self, key: &str, body: &str) -> PortResult<()> {
        self.put(key, body.as_bytes()).await
    }
}

// ---------------------------------------------------------------- cost reconciliation

/// What to ask the billing system about: everything tagged to one optimization within a time window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CostActualsQuery {
    pub optimization_id: OptimizationId,
    /// Window start (epoch ms); implementations round down to their billing granularity (UTC day).
    pub start_ms: i64,
    /// Window end (epoch ms); implementations round up to their billing granularity.
    pub end_ms: i64,
}

/// Billed cost of the tagged resources, mapped to ledger categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CostActuals {
    /// USD per ledger category (a category missing from the billing data is absent, not 0).
    pub by_category: BTreeMap<SpendCategory, f64>,
    /// Number of non-empty billing groups that came back; 0 means no billing data at all (not available yet, or
    /// tags not activated).
    pub groups: u32,
    /// False while the billing system flags the window as still being estimated / incomplete.
    pub complete: bool,
    /// Why `complete` is false, or any caveat worth showing the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Billing truth for the cost meter's estimate (docs/V0_ARCHITECTURE.md section 7, "Reconcile"). The AWS
/// implementation queries Cost Explorer by cost-allocation tag (~24 h lag); the mock returns canned numbers.
/// [`reconcile_spend`](crate::cost::reconcile_spend) turns the difference into ledger correction rows.
#[async_trait]
pub trait CostReconciler: Send + Sync {
    async fn actuals(&self, query: &CostActualsQuery) -> PortResult<CostActuals>;
}

/// One category of a reconcile run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CostReconcileLine {
    pub category: SpendCategory,
    /// Ledger total for the category before this run (estimates plus earlier corrections).
    pub ledger_usd: f64,
    /// Billed amount for the category.
    pub actual_usd: f64,
    /// `actual_usd - ledger_usd`; the correction row's amount when `written`.
    pub delta_usd: f64,
    pub written: bool,
}

/// - `reconciled`: at least one correction row was written.
/// - `unchanged`: billing data is complete and already matches the ledger (re-runs land here).
/// - `pending`: billing data is incomplete or missing; nothing was written. Try again after ~24 h.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum CostReconcileStatus {
    Reconciled,
    Unchanged,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TimeWindow {
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CostReconcileReport {
    pub optimization_id: OptimizationId,
    pub at: i64,
    pub status: CostReconcileStatus,
    /// Set when `status` is `pending`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub window: TimeWindow,
    pub lines: Vec<CostReconcileLine>,
    /// Sum of the correction rows written by this run.
    pub written_usd: f64,
}

// ---------------------------------------------------------------- git

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CandidateCommit {
    pub sha: String,
    pub branch: String,
    pub message: String,
    pub committed_at: i64,
}

/// One file of a diff between two commits (renames are reported as a delete plus an add).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct FileDiff {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    /// True for binary files: no line counts, no patch.
    pub binary: bool,
    /// This file's unified diff (`@@` hunks only, no `diff --git` / index headers); empty for binary files.
    pub patch: String,
}

/// A file for `commit_files`. `path` is repo-relative POSIX; `executable` sets mode 100755.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CommitFile {
    pub path: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
}

impl CommitFile {
    /// A regular (mode 100644) file.
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            executable: None,
        }
    }

    /// The same file with mode 100755.
    pub fn executable(mut self) -> Self {
        self.executable = Some(true);
        self
    }

    /// Whether the file gets mode 100755.
    pub fn is_executable(&self) -> bool {
        self.executable.unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CommitFilesInput {
    /// Branch (or sha) the new commit is parented on and whose tree it extends.
    pub base_ref: String,
    /// The branch to create. It must not exist yet.
    pub branch: String,
    pub message: String,
    /// Written over the base tree (added, or replaced when the path exists).
    pub files: Vec<CommitFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PullRequestInput {
    pub base: String,
    pub head: String,
    pub title: String,
    pub body: String,
    pub shas: Vec<String>,
    /// Squash commit message; defaults to `title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct PullRequestRef {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CommitRef {
    pub sha: String,
}

#[async_trait]
pub trait GitHost: Send + Sync {
    /// Create `branch` at `from_ref` in the remote.
    async fn create_branch(&self, from_ref: &str, branch: &str) -> PortResult<()>;
    /// Commits on `branch` carrying the `Ratchet-Candidate: true` trailer, oldest first, after `since_sha`.
    async fn list_candidates(
        &self,
        branch: &str,
        since_sha: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>>;
    async fn changed_files(&self, base_ref: &str, sha: &str) -> PortResult<Vec<String>>;
    /// Per-file diff of `sha` against its merge base with `base_ref` (what a PR from `sha` into `base_ref`
    /// would show).
    async fn diff(&self, base_ref: &str, sha: &str) -> PortResult<Vec<FileDiff>>;
    /// Materialize `sha` (code) into `dir`; used by the bench runner.
    async fn checkout(&self, sha: &str, dir: &std::path::Path) -> PortResult<()>;
    /// Squash the verified `shas` (ratchet order; last one's tree wins) onto `base` as branch `head` and open
    /// a PR.
    async fn open_pull_request(&self, input: &PullRequestInput) -> PortResult<PullRequestRef>;
    /// Resolve a ref (branch, tag, sha) to a full commit sha.
    async fn resolve_ref(&self, git_ref: &str) -> PortResult<String>;
    fn remote_url(&self) -> String;
    /// Create `branch` in the remote as one new commit on top of `base_ref` that adds `files` (a template, say)
    /// and return that commit's sha. Fails when the branch already exists. Optional: the default says it is
    /// unsupported. To open the PR from the branch call `open_pull_request` with `shas: []`.
    async fn commit_files(&self, _input: &CommitFilesInput) -> PortResult<CommitRef> {
        Err(PortError::Unsupported("commit_files"))
    }
}

// ---------------------------------------------------------------- agents

/// Incremental usage (tokens / cost for one event only, not cumulative).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AgentEventType {
    Status,
    Message,
    Tool,
    Commit,
    Usage,
    Error,
    Verdict,
}

/// Conventions: `usage` is incremental. `status` events carry `data.state` in `running | paused | done | failed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AgentEvent {
    /// Epoch ms. Optional on read: older writers omitted it.
    #[serde(default)]
    pub at: i64,
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(value_type = Option<Object>))]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
}

impl AgentEvent {
    /// An event with no text, data or usage.
    pub fn new(at: i64, event_type: AgentEventType) -> Self {
        Self {
            at,
            event_type,
            text: None,
            data: None,
            usage: None,
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Attach `data` (an object). Top-level `null` entries are dropped, like `JSON.stringify` drops `undefined`
    /// fields; anything that is not an object is ignored.
    pub fn with_data(mut self, data: Value) -> Self {
        if let Value::Object(map) = data {
            self.data = Some(Value::Object(
                map.into_iter().filter(|(_, v)| !v.is_null()).collect(),
            ));
        }
        self
    }

    pub fn with_usage(mut self, usage: AgentUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// `data[key]` as a string.
    pub fn data_str(&self, key: &str) -> Option<&str> {
        self.data.as_ref()?.get(key)?.as_str()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    pub workdir: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AgentSession {
    pub id: String,
    pub kind: AgentKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Preflight {
    pub ok: bool,
    pub problems: Vec<String>,
}

/// Runs inside `ratchet runner` on an agent runner (or locally in simulation).
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn kind(&self) -> AgentKind;
    async fn preflight(&self) -> PortResult<Preflight>;
    /// Run one agent turn to completion, streaming events to `events`.
    async fn start(
        &self,
        spec: &AgentSpec,
        events: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession>;
    /// Continue the same session with a new message; returns when the turn ends.
    ///
    /// The session is `&mut` because a CLI may fork a new session id on resume; the adapter updates it.
    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        events: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<()>;
}

// ---------------------------------------------------------------- runner control (engine <-> runner loop)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RunnerMessage {
    Verdict {
        sha: String,
        verdict: CandidateVerdict,
        reasons: Vec<String>,
        current_best: f64,
    },
    /// The shared ratchet moved (possibly by another agent): rebase onto `sha` from `branch`.
    Ratchet {
        sha: String,
        branch: String,
        value: f64,
    },
    User {
        text: String,
    },
    Pause,
    Resume,
    Stop,
    /// A message with a `type` this version does not know (an engine newer than the runner). Never sent; the
    /// runner reports it and carries on.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunnerConfig {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub kind: AgentKind,
    pub repo_url: String,
    pub branch: String,
    pub base_ref: String,
    pub config: RatchetConfig,
    /// RATCHET.md contents.
    pub brief: String,
    pub baseline_value: f64,
    /// The win threshold in force (a fraction): ratchet.yaml's `min_gain`, or the one calibrated from the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_gain: Option<f64>,
    pub budget_usd: f64,
    pub deadline_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunnerHandle {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub instance_id: String,
}

/// Events since a cursor, and the next cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct PolledEvents {
    pub events: Vec<AgentEvent>,
    pub cursor: String,
}

#[async_trait]
pub trait RunnerControl: Send + Sync {
    async fn start(&self, handle: &RunnerHandle, config: &RunnerConfig) -> PortResult<()>;
    async fn send(&self, handle: &RunnerHandle, message: &RunnerMessage) -> PortResult<()>;
    /// Events since `cursor` (opaque); returns the next cursor.
    async fn poll(&self, handle: &RunnerHandle, cursor: Option<&str>) -> PortResult<PolledEvents>;
    async fn stop(&self, handle: &RunnerHandle) -> PortResult<()>;
    /// Free whatever the control holds on this machine for an optimization (local worktrees and clones). Called
    /// by the engine's `teardown`. Optional: the SSM control keeps nothing locally.
    async fn release(&self, _optimization_id: &OptimizationId) -> PortResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------- bench

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum BenchJobPhase {
    Baseline,
    Bench,
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BenchJob {
    pub optimization_id: OptimizationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<CandidateId>,
    pub sha: String,
    pub phase: BenchJobPhase,
    pub repeats: u64,
    /// For confirm: the hidden trace reference (S3 key or local path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BenchRun {
    /// The objective metric for this run.
    pub value: f64,
    /// Flattened numeric output, e.g. `http_req_duration.p95`, `error_rate`, `peak_rss_mb`.
    pub metrics: BTreeMap<String, f64>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BenchOutcome {
    pub runs: Vec<BenchRun>,
    /// `runs.map(|r| r.value)`
    pub samples: Vec<f64>,
    pub guardrails: Vec<GuardrailResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs_key: Option<String>,
    pub started_at: i64,
    pub finished_at: i64,
}

/// The baseline's metrics, for relative guardrails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BaselineMetrics {
    pub metrics: BTreeMap<String, f64>,
}

#[async_trait]
pub trait BenchRunner: Send + Sync {
    /// Check out `job.sha` (bench code from `config.evaluator.ref`), run command guardrails, then the benchmark
    /// `job.repeats` times, then threshold guardrails against `baseline`.
    async fn run(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome>;
}

// ---------------------------------------------------------------- engine API (app <-> engine)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EngineCreateOptimizationInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub repo_url: String,
    pub base_ref: String,
    /// Path inside the repo, default `ratchet.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    pub agents: Vec<AgentKind>,
    pub budget_usd: f64,
    pub time_limit_seconds: u64,
    pub plateau_evals: u32,
    /// The sandbox this optimization runs in. Live mode requires an applied sandbox and links it
    /// (`optimizations.sandbox_id`); in simulation an applied one is linked, otherwise the field stays null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    /// Where the agents run. Default: the engine's `agents_where` setting, else `local` when the engine has a
    /// local runner control, else `cloud`. The evaluator always runs in the cloud sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_where: Option<AgentsWhere>,
}

/// What the ledger knows about one sandbox, plus the optimizations linked to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SandboxState {
    pub sandbox_id: String,
    /// `absent` = never planned or applied.
    pub status: SandboxStateStatus,
    pub provider: Option<SandboxProviderKind>,
    pub region: Option<String>,
    pub account_id: Option<String>,
    /// Contract outputs, from the ledger (set on apply, cleared on destroy).
    pub outputs: Option<SandboxOutputs>,
    pub applied_at: Option<i64>,
    pub destroyed_at: Option<i64>,
    /// Why the last apply / destroy failed (this process only).
    pub last_error: Option<String>,
    pub optimizations: Vec<SandboxOptimizationRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SandboxOptimizationRef {
    pub id: OptimizationId,
    pub status: crate::enums::OptimizationStatus,
}

/// A [`SandboxStatus`] or `absent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum SandboxStateStatus {
    Absent,
    Planned,
    Applying,
    Applied,
    Destroying,
    Destroyed,
    Failed,
}

impl From<SandboxStatus> for SandboxStateStatus {
    fn from(s: SandboxStatus) -> Self {
        match s {
            SandboxStatus::Planned => Self::Planned,
            SandboxStatus::Applying => Self::Applying,
            SandboxStatus::Applied => Self::Applied,
            SandboxStatus::Destroying => Self::Destroying,
            SandboxStatus::Destroyed => Self::Destroyed,
            SandboxStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// `creating` -> `ready` -> (`idle` while no run is active) -> `destroying` -> `destroyed`; `failed` when a step
/// failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum SandboxPhase {
    Creating,
    Ready,
    Idle,
    Destroying,
    Destroyed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum EngineEvent {
    Optimization {
        optimization_id: OptimizationId,
        status: String,
    },
    AgentEvent {
        optimization_id: OptimizationId,
        agent_id: AgentId,
        event: AgentEvent,
    },
    Candidate {
        optimization_id: OptimizationId,
        candidate_id: CandidateId,
        verdict: CandidateVerdict,
    },
    Ratchet {
        optimization_id: OptimizationId,
        from: f64,
        to: f64,
    },
    Spend {
        optimization_id: OptimizationId,
        total_usd: f64,
    },
    Notice {
        optimization_id: OptimizationId,
        level: NoticeLevel,
        text: String,
    },
    /// True while any optimization is starting, baselining, running or stopping (false again once none is). The
    /// app turns this into a "prevent system sleep" assertion so the machine does not sleep under a live run.
    RunActive {
        active: bool,
        optimization_ids: Vec<OptimizationId>,
    },
    /// The automatic sandbox changing phase (docs/SIMPLIFIED_MODEL.md, "Sandbox lifecycle: invisible").
    Sandbox {
        sandbox_id: String,
        phase: SandboxPhase,
        region: Option<String>,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum TeardownTrigger {
    /// The idle window after the last run elapsed.
    Idle,
    /// `end_sandbox`.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TeardownRun {
    pub optimization_id: OptimizationId,
    pub status: String,
    pub spend_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TeardownRemoved {
    /// Instances terminated during teardown (agent runners, bench, experiment).
    pub instances: Vec<String>,
    #[serde(rename = "agentIdentities")]
    pub agent_identities: Vec<String>,
    pub secrets: Vec<String>,
    /// The sandbox's own resources, named from its outputs (buckets, network, roles).
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TeardownOrphan {
    pub id: String,
    pub role: String,
    pub state: String,
}

/// What the engine records when it tears the sandbox down (automatically after the idle window, or on demand):
/// what was removed, what it cost, and proof that nothing is left running. Shown on the Result screen and
/// Environments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SandboxTeardownReceipt {
    pub sandbox_id: String,
    pub region: String,
    pub trigger: TeardownTrigger,
    pub created_at: Option<i64>,
    pub destroyed_at: i64,
    pub runs: Vec<TeardownRun>,
    /// Estimated spend of every run that used the sandbox, split by category (tokens, compute, ...).
    pub spend_usd: f64,
    pub spend_by_category: BTreeMap<String, f64>,
    pub removed: TeardownRemoved,
    /// Verified by `list_tagged` after the destroy: nothing tagged is left, so nothing is billing.
    pub nothing_running: bool,
    pub orphans: Vec<TeardownOrphan>,
    pub errors: Vec<String>,
    /// One plain-words line for the UI.
    pub summary: String,
}

/// The invisible sandbox seen from outside: what is running, what it costs, when it goes away, the last receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SandboxActivity {
    pub sandbox_id: String,
    pub status: SandboxStateStatus,
    pub region: Option<String>,
    /// True when the engine created it itself and will destroy it after the idle window.
    pub auto: bool,
    pub active_runs: Vec<OptimizationId>,
    /// Tagged instances alive now (`None`: could not be listed).
    pub instances_running: Option<u64>,
    /// Estimated spend of the runs currently active in it.
    pub spend_usd: f64,
    pub idle_since: Option<i64>,
    /// When the automatic destroy is due; `None` while runs are active or the sandbox is not auto-managed.
    pub destroy_at: Option<i64>,
    pub last_receipt: Option<SandboxTeardownReceipt>,
}

/// What `EngineApi::propose_template` needs: a repository, the branch the PR targets, and the template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProposeTemplateInput {
    pub repo_url: String,
    /// The branch the PR targets (and the files are added on top of).
    pub base_ref: String,
    /// A template id from `list_templates()` (a legacy `--pack` name is accepted). Demos cannot be proposed.
    pub template_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<crate::enums::Direction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_usd: Option<f64>,
    /// Add the files even when some already exist on `base_ref` (they are replaced). Default false: refuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
}

impl ProposeTemplateInput {
    /// The render options carried by this input.
    pub fn render_options(&self) -> RenderTemplateOptions {
        RenderTemplateOptions {
            metric: self.metric.clone(),
            direction: self.direction,
            value_usd: self.value_usd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProposeTemplateResult {
    pub pr_url: String,
    /// The branch the PR was opened from.
    pub branch: String,
    /// The commit on `branch`.
    pub sha: String,
    pub template_id: String,
    /// Repo paths added by the commit.
    pub files: Vec<String>,
}

#[async_trait]
pub trait EngineApi: Send + Sync {
    fn mode(&self) -> Mode;
    async fn create_optimization(
        &self,
        input: &EngineCreateOptimizationInput,
    ) -> PortResult<OptimizationId>;
    async fn start(&self, id: &OptimizationId) -> PortResult<()>;
    /// Kill switch: revoke identities, stop agents, terminate instances.
    async fn stop(&self, id: &OptimizationId, reason: Option<&str>) -> PortResult<()>;
    async fn pause_agent(&self, agent_id: &AgentId) -> PortResult<()>;
    async fn resume_agent(&self, agent_id: &AgentId) -> PortResult<()>;
    async fn message_agent(&self, agent_id: &AgentId, text: &str) -> PortResult<()>;
    async fn open_pull_request(&self, id: &OptimizationId) -> PortResult<PullRequestRef>;
    async fn teardown(&self, id: &OptimizationId) -> PortResult<()>;
    /// Compare the ledger's estimated infrastructure spend with the billing data (Cost Explorer) and record
    /// correction rows. Idempotent; reports `pending` while billing data lags. Optional.
    async fn reconcile_costs(&self, _id: &OptimizationId) -> PortResult<CostReconcileReport> {
        Err(PortError::Unsupported("reconcile_costs"))
    }
    /// The auto-calibrated statistics of an optimization (docs/SIMPLIFIED_MODEL.md): how many runs each
    /// candidate gets, the gain a win must beat, and `explanation`, one plain-words sentence for the UI. Derived
    /// from the baseline, so `None` until the baseline has run. Optional.
    async fn calibration(&self, _id: &OptimizationId) -> PortResult<Option<Calibration>> {
        Err(PortError::Unsupported("calibration"))
    }
    /// Every path protected for this optimization, with why: the automatic set plus the user's `protected:`
    /// list. Optional.
    async fn protected_paths(&self, _id: &OptimizationId) -> PortResult<EffectiveProtected> {
        Err(PortError::Unsupported("protected_paths"))
    }
    /// The evaluator template gallery: runnable templates, then the "Try a demo" entries. Optional.
    async fn list_templates(&self) -> PortResult<Vec<TemplateInfo>> {
        Ok(crate::templates::list_templates(Default::default()))
    }
    /// "Use template": commit the template's files (ratchet.yaml, RATCHET.md, the evaluator) on a new branch of
    /// the repo and open a PR whose description says what was added and how to run a baseline. Refuses when the
    /// files already exist (unless `overwrite`) or the git host cannot commit files. Optional.
    async fn propose_template(
        &self,
        _input: &ProposeTemplateInput,
    ) -> PortResult<ProposeTemplateResult> {
        Err(PortError::Unsupported("propose_template"))
    }
    /// Sandbox lifecycle: planned -> applying -> applied -> destroying -> destroyed / failed, recorded in the
    /// ledger.
    async fn sandbox_state(&self, sandbox_id: &str) -> PortResult<SandboxState>;
    async fn plan_sandbox(&self, input: &SandboxInput) -> PortResult<SandboxPlan>;
    async fn apply_sandbox(&self, input: &SandboxInput) -> PortResult<SandboxOutputs>;
    /// Refuses while an optimization in the sandbox is running, or while tagged instances / agent identities
    /// remain.
    async fn destroy_sandbox(&self, sandbox_id: &str) -> PortResult<()>;
    /// A receiver of engine events (the Rust form of `subscribe(listener)`; drop it to unsubscribe).
    fn subscribe(&self) -> broadcast::Receiver<EngineEvent>;
    /// The receipt of the sandbox teardown that covered this optimization (`None` while the sandbox is still
    /// alive or the run never used one). Optional.
    async fn teardown_receipt(
        &self,
        _optimization_id: &OptimizationId,
    ) -> PortResult<Option<SandboxTeardownReceipt>> {
        Err(PortError::Unsupported("teardown_receipt"))
    }
    /// Live view of the automatic sandbox (running instances, spend, destroy time, last receipt). Optional.
    async fn sandbox_activity(&self, _sandbox_id: &str) -> PortResult<SandboxActivity> {
        Err(PortError::Unsupported("sandbox_activity"))
    }
    /// "End the environment now": tear the sandbox down without waiting for the idle window. Refuses while a
    /// run is active. Optional.
    async fn end_sandbox(&self, _sandbox_id: &str) -> PortResult<SandboxTeardownReceipt> {
        Err(PortError::Unsupported("end_sandbox"))
    }
    /// True while any run is active (the same signal as the `run-active` event), for the sleep-prevention hook.
    fn run_active(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------- runner loop (`ratchet runner`)

/// The inbox the runner loop reads engine messages from (SSM/S3 in live mode, in-memory in simulation).
#[async_trait]
pub trait RunnerInbox: Send + Sync {
    /// Resolves with the next message, or `None` when `cancel` fires.
    async fn next(&self, cancel: Option<&CancellationToken>) -> PortResult<Option<RunnerMessage>>;
}

// ---------------------------------------------------------------- in-process implementations

/// Real clock and simple in-memory mocks of the ports that `ratchet-core` itself exercises in tests. The mocks of
/// the AWS / GitHub / agent ports live with their crates.
pub mod mock {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    /// Wall-clock time.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct SystemClock;

    #[async_trait]
    impl Clock for SystemClock {
        fn now(&self) -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        }

        async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
            let sleep = tokio::time::sleep(std::time::Duration::from_millis(ms));
            match cancel {
                None => {
                    sleep.await;
                    Ok(())
                }
                Some(c) => tokio::select! {
                    () = sleep => Ok(()),
                    () = c.cancelled() => Err(PortError::Cancelled),
                },
            }
        }
    }

    /// A clock that only moves when told to: `sleep` advances it instead of waiting.
    #[derive(Debug, Default)]
    pub struct ManualClock {
        now: AtomicI64,
    }

    impl ManualClock {
        pub fn new(start_ms: i64) -> Self {
            Self {
                now: AtomicI64::new(start_ms),
            }
        }

        pub fn advance(&self, ms: i64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }

        pub fn set(&self, ms: i64) {
            self.now.store(ms, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Clock for ManualClock {
        fn now(&self) -> i64 {
            self.now.load(Ordering::SeqCst)
        }

        async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(PortError::Cancelled);
            }
            self.advance(ms as i64);
            Ok(())
        }
    }

    /// Contract outputs for tests and simulation: well-formed ARNs and ids, account `123456789012`, no egress
    /// proxy, no dependency identity.
    pub fn sample_sandbox_outputs(sandbox_id: &str) -> SandboxOutputs {
        let identity = |name: &str| AwsIdentityRef {
            role_arn: format!("arn:aws:iam::123456789012:role/ratchet/{name}-{sandbox_id}"),
            role_name: format!("{name}-{sandbox_id}"),
            instance_profile_arn: format!(
                "arn:aws:iam::123456789012:instance-profile/ratchet/{name}-{sandbox_id}"
            ),
            instance_profile_name: format!("{name}-{sandbox_id}"),
        };
        let eval_identity = identity("eval");
        SandboxOutputs {
            artifacts_bucket: format!("ratchet-artifacts-{sandbox_id}"),
            sealed_bucket: format!("ratchet-sealed-{sandbox_id}"),
            agent_boundary_ref: format!(
                "arn:aws:iam::123456789012:policy/ratchet-agent-boundary-{sandbox_id}"
            ),
            eval_identity_ref: eval_identity.instance_profile_arn.clone(),
            agent_role_path: "/ratchet/agent/".into(),
            network: SandboxNetwork {
                subnet_id: "subnet-0a1".into(),
                agent_runner_security_group_id: "sg-0a1".into(),
                bench_security_group_id: "sg-0b1".into(),
                bench_subnet_id: "subnet-0b1".into(),
                egress_proxy_url: None,
            },
            region: "us-east-1".into(),
            vpc_id: "vpc-0a1".into(),
            eval_identity,
            confirm_identity: identity("confirm"),
            deps_identity: None,
            sealed_confirm_prefix: "confirm/".into(),
            limits: SandboxLimits {
                max_instances_per_agent: 4,
                max_instance_lifetime_hours: 2.0,
            },
        }
    }

    /// A [`CostReconciler`] returning canned numbers and recording the queries it got.
    pub struct MockCostReconciler {
        response: Mutex<Result<CostActuals, PortError>>,
        calls: Mutex<Vec<CostActualsQuery>>,
    }

    impl MockCostReconciler {
        pub fn new(actuals: CostActuals) -> Self {
            Self {
                response: Mutex::new(Ok(actuals)),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Complete billing data with the given per-category amounts.
        pub fn complete(by_category: &[(SpendCategory, f64)]) -> Self {
            Self::new(CostActuals {
                by_category: by_category.iter().copied().collect(),
                groups: by_category.len() as u32,
                complete: true,
                note: None,
            })
        }

        pub fn failing(message: &str) -> Self {
            Self {
                response: Mutex::new(Err(PortError::Failed(message.to_owned()))),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn set(&self, actuals: CostActuals) {
            if let Ok(mut r) = self.response.lock() {
                *r = Ok(actuals);
            }
        }

        pub fn calls(&self) -> Vec<CostActualsQuery> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    #[async_trait]
    impl CostReconciler for MockCostReconciler {
        async fn actuals(&self, query: &CostActualsQuery) -> PortResult<CostActuals> {
            if let Ok(mut c) = self.calls.lock() {
                c.push(query.clone());
            }
            self.response
                .lock()
                .map_err(|_| PortError::failed("poisoned"))?
                .clone()
        }
    }

    /// An in-memory [`ArtifactStore`].
    #[derive(Default)]
    pub struct MemoryArtifactStore {
        objects: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    #[async_trait]
    impl ArtifactStore for MemoryArtifactStore {
        async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
            self.objects
                .lock()
                .map_err(|_| PortError::failed("poisoned"))?
                .insert(key.to_owned(), body.to_vec());
            Ok(())
        }

        async fn get(&self, key: &str) -> PortResult<Option<String>> {
            let g = self
                .objects
                .lock()
                .map_err(|_| PortError::failed("poisoned"))?;
            Ok(g.get(key).map(|b| String::from_utf8_lossy(b).into_owned()))
        }

        async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
            let g = self
                .objects
                .lock()
                .map_err(|_| PortError::failed("poisoned"))?;
            Ok(g.keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect())
        }
    }

    /// A [`PriceBook`] with a fixed table.
    #[derive(Default)]
    pub struct StaticPriceBook {
        prices: HashMap<String, f64>,
    }

    impl StaticPriceBook {
        pub fn new(prices: &[(&str, f64)]) -> Self {
            Self {
                prices: prices.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            }
        }
    }

    #[async_trait]
    impl PriceBook for StaticPriceBook {
        async fn hourly_usd(&self, instance_type: &str) -> PortResult<f64> {
            self.prices
                .get(instance_type)
                .copied()
                .ok_or_else(|| PortError::failed(format!("no price for {instance_type}")))
        }
    }

    /// A [`SandboxProvider`] that records its calls and can be told to fail. The simulation's provider.
    pub struct MockSandboxProvider {
        pub outputs: SandboxOutputs,
        state: Mutex<MockSandboxState>,
    }

    #[derive(Default)]
    struct MockSandboxState {
        calls: Vec<String>,
        inputs: Vec<SandboxInput>,
        fail_apply: Option<PortError>,
        fail_destroy: Option<PortError>,
    }

    impl MockSandboxProvider {
        pub fn new(outputs: SandboxOutputs) -> Self {
            Self {
                outputs,
                state: Mutex::new(MockSandboxState::default()),
            }
        }

        fn with<T>(&self, f: impl FnOnce(&mut MockSandboxState) -> T) -> T {
            let mut g = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            f(&mut g)
        }

        pub fn fail_apply(&self, e: Option<PortError>) {
            self.with(|s| s.fail_apply = e);
        }

        pub fn fail_destroy(&self, e: Option<PortError>) {
            self.with(|s| s.fail_destroy = e);
        }

        /// `plan`, `apply`, `destroy` in call order.
        pub fn calls(&self) -> Vec<String> {
            self.with(|s| s.calls.clone())
        }

        /// The input of every apply / destroy call, in order.
        pub fn inputs(&self) -> Vec<SandboxInput> {
            self.with(|s| s.inputs.clone())
        }
    }

    #[async_trait]
    impl SandboxProvider for MockSandboxProvider {
        async fn plan(&self, _input: &SandboxInput) -> PortResult<SandboxPlan> {
            self.with(|s| s.calls.push("plan".into()));
            Ok(SandboxPlan {
                changes: vec![PlanChange {
                    action: PlanAction::Create,
                    address: "aws_s3_bucket.a".into(),
                    resource_type: "aws_s3_bucket".into(),
                }],
                summary: "Plan: 1 to add, 0 to change, 0 to destroy.".into(),
            })
        }

        async fn apply(&self, input: &SandboxInput) -> PortResult<SandboxOutputs> {
            self.with(|s| {
                s.calls.push("apply".into());
                s.inputs.push(input.clone());
                s.fail_apply.clone()
            })
            .map_or_else(|| Ok(self.outputs.clone()), Err)
        }

        async fn destroy(&self, input: &SandboxInput) -> PortResult<()> {
            self.with(|s| {
                s.calls.push("destroy".into());
                s.inputs.push(input.clone());
                s.fail_destroy.clone()
            })
            .map_or(Ok(()), Err)
        }

        async fn outputs(&self, _sandbox_id: &str) -> PortResult<Option<SandboxOutputs>> {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;

    #[tokio::test]
    async fn manual_clock_sleep_advances_time() {
        let c = ManualClock::new(1_000);
        c.sleep(500, None).await.unwrap();
        assert_eq!(c.now(), 1_500);
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(c.sleep(1, Some(&token)).await, Err(PortError::Cancelled));
    }

    #[tokio::test]
    async fn system_clock_sleep_is_cancellable() {
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move { t2.cancel() });
        assert_eq!(
            SystemClock.sleep(60_000, Some(&token)).await,
            Err(PortError::Cancelled)
        );
    }

    #[tokio::test]
    async fn memory_artifact_store_round_trips() {
        let s = MemoryArtifactStore::default();
        s.put("a/1", b"x").await.unwrap();
        s.put("b/1", b"y").await.unwrap();
        assert_eq!(s.get("a/1").await.unwrap().as_deref(), Some("x"));
        assert_eq!(s.get("zzz").await.unwrap(), None);
        assert_eq!(s.list("a/").await.unwrap(), ["a/1"]);
    }

    #[test]
    fn runner_messages_use_the_typescript_wire_shape() {
        let m = RunnerMessage::Verdict {
            sha: "abc".into(),
            verdict: CandidateVerdict::FailedConfirm,
            reasons: vec!["r".into()],
            current_best: 90.0,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "verdict", "sha": "abc", "verdict": "failed_confirm", "reasons": ["r"], "currentBest": 90.0})
        );
        assert_eq!(
            serde_json::to_value(RunnerMessage::Pause).unwrap(),
            serde_json::json!({"type": "pause"})
        );
        let back: RunnerMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn engine_events_use_kebab_case_tags() {
        let e = EngineEvent::RunActive {
            active: true,
            optimization_ids: vec!["o".into()],
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "run-active", "active": true, "optimizationIds": ["o"]})
        );
        let e = EngineEvent::AgentEvent {
            optimization_id: "o".into(),
            agent_id: "a".into(),
            event: AgentEvent {
                at: 1,
                event_type: AgentEventType::Message,
                text: Some("hi".into()),
                data: None,
                usage: None,
            },
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "agent-event");
        assert_eq!(v["event"]["type"], "message");
    }

    #[test]
    fn sandbox_outputs_match_the_contract_names() {
        let o = sample_sandbox_outputs("sbx");
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v["agentRolePath"], "/ratchet/agent/");
        assert_eq!(v["network"]["agentRunnerSecurityGroupId"], "sg-0a1");
        assert_eq!(v["network"]["egressProxyUrl"], serde_json::Value::Null);
        assert_eq!(v["evalIdentity"]["roleName"], "eval-sbx");
        assert!(v.get("depsIdentity").is_none());
        // whole numbers print like JavaScript numbers
        assert_eq!(v["limits"]["maxInstanceLifetimeHours"].to_string(), "2");
        let back: SandboxOutputs = serde_json::from_value(v).unwrap();
        assert_eq!(back, o);
    }

    #[test]
    fn sandbox_input_hours_keep_their_fraction() {
        let i: SandboxInput = serde_json::from_value(serde_json::json!({
            "sandboxId": "s", "region": "r", "allowedInstanceTypes": [],
            "maxInstancesPerAgent": 4, "maxInstanceLifetimeHours": 0.5
        }))
        .unwrap();
        assert_eq!(i.max_instance_lifetime_hours, 0.5);
        assert_eq!(
            serde_json::to_value(&i).unwrap()["maxInstanceLifetimeHours"].to_string(),
            "0.5"
        );
    }

    #[test]
    fn unknown_runner_messages_decode_instead_of_failing() {
        let m: RunnerMessage = serde_json::from_str(r#"{"type":"from-the-future","x":1}"#).unwrap();
        assert_eq!(m, RunnerMessage::Unknown);
    }

    #[test]
    fn agent_events_parse_without_at_and_drop_null_data() {
        let e: AgentEvent = serde_json::from_str(r#"{"type":"status","text":"one"}"#).unwrap();
        assert_eq!((e.at, e.text.as_deref()), (0, Some("one")));
        let e = AgentEvent::new(5, AgentEventType::Tool)
            .with_data(serde_json::json!({"name": "x", "exitCode": null}));
        assert_eq!(e.data_str("name"), Some("x"));
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"at":5,"type":"tool","data":{"name":"x"}}"#
        );
    }
}
