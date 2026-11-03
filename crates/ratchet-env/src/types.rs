//! The data types of the dynamic-environment flow and the [`EnvironmentDrafter`] service (`types.ts`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use ratchet_core::enums::{AgentKind, EnvironmentScope};
use ratchet_core::ports::PortResult;
use serde::{Deserialize, Serialize};

/// What a machine of the environment does. Mirrors the `role` of the base module's `machines` map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentRole {
    App,
    Database,
    Cache,
    Load,
    Evaluator,
    Other,
}

impl EnvironmentRole {
    pub const ALL: [EnvironmentRole; 6] = [
        Self::App,
        Self::Database,
        Self::Cache,
        Self::Load,
        Self::Evaluator,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Database => "database",
            Self::Cache => "cache",
            Self::Load => "load",
            Self::Evaluator => "evaluator",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

impl fmt::Display for EnvironmentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of "What gets created".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentResource {
    /// "API", "Postgres 16", "Load generator".
    pub name: String,
    pub role: EnvironmentRole,
    /// Instance type: `c7i.2xlarge`.
    pub machine: String,
    /// One plain line: "shop-api in the repo's container".
    pub detail: String,
    pub usd_per_hour: f64,
}

/// A file of the draft, repo-relative POSIX path (`ratchet.yaml`, or under `infra/ratchet/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentFile {
    pub path: String,
    pub content: String,
}

impl EnvironmentFile {
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// One of Ratchet's checks on the draft ("isolated", "results sealed", "same hardware every time", "full
/// teardown", ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentCheck {
    pub id: String,
    /// Plain words for the UI.
    pub label: String,
    pub ok: bool,
    /// Why it passed or failed, one line (the list of offending lines when it failed).
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCost {
    /// Sum of the resources' hourly prices (excludes the sandbox's fixed private-network cost, ~$0.04/h, every run
    /// pays).
    pub usd_per_hour: f64,
    /// Minutes until the environment is ready to measure (sandbox boot + one per machine).
    pub startup_minutes: u32,
    /// `usd_per_hour x (run hours + start-up)` for the run's time limit.
    pub estimate_for_run_usd: f64,
}

/// What the setup agent proposed, as the "New optimization > Environment" step shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDraft {
    pub scope: EnvironmentScope,
    /// Why this scope, in a sentence or two.
    pub reason: String,
    pub resources: Vec<EnvironmentResource>,
    /// What would be committed: `ratchet.yaml` plus the files under `infra/ratchet/`.
    pub files: Vec<EnvironmentFile>,
    pub checks: Vec<EnvironmentCheck>,
    pub cost: EnvironmentCost,
    /// Which connector drafted it: `claude-code`, `codex` (or `mock`).
    pub agent: AgentKind,
    /// How long drafting took.
    pub duration_s: u64,
    /// What the setup agent cost in model tokens.
    pub usd_spent: f64,
}

/// True when every check passed: the draft may be approved.
pub fn draft_passed(draft: &EnvironmentDraft) -> bool {
    checks_passed(&draft.checks)
}

/// True when there is at least one check and all of them passed.
pub fn checks_passed(checks: &[EnvironmentCheck]) -> bool {
    !checks.is_empty() && checks.iter().all(|c| c.ok)
}

// ---------------------------------------------------------------- the drafter service

/// Called with progress while the agent works (for a spinner / live log).
pub type ProgressFn = Arc<dyn Fn(EnvironmentDraftProgress) + Send + Sync>;

#[derive(Clone)]
pub struct DraftEnvironmentInput {
    /// Repository URL the optimization runs on.
    pub repo: String,
    /// The protected ref the files are committed to / the PR targets. Default `main`.
    pub base_ref: Option<String>,
    /// What the user wants optimized, in their words ("make checkout p95 faster").
    pub goal: String,
    /// The user's agent connector that does the drafting (their own login, run locally).
    pub agent_connector: AgentKind,
    /// The AWS profile selected in Settings: `tofu plan` is checked against that account.
    pub aws_profile: Option<String>,
    /// Region of the sandbox (default: `environment.region` of ratchet.yaml, else `us-east-1`).
    pub region: Option<String>,
    /// The run's time limit, for the cost estimate (default 2 h).
    pub time_limit_seconds: Option<f64>,
    /// Path of the config inside the repo (default `ratchet.yaml`).
    pub config_path: Option<String>,
    /// Pick the draft's id instead of a generated one (a service that answers before the agent is done).
    pub id: Option<String>,
    /// Progress while the agent works.
    pub on_progress: Option<ProgressFn>,
}

impl DraftEnvironmentInput {
    pub fn new(
        repo: impl Into<String>,
        goal: impl Into<String>,
        agent_connector: AgentKind,
    ) -> Self {
        Self {
            repo: repo.into(),
            base_ref: None,
            goal: goal.into(),
            agent_connector,
            aws_profile: None,
            region: None,
            time_limit_seconds: None,
            config_path: None,
            id: None,
            on_progress: None,
        }
    }
}

impl fmt::Debug for DraftEnvironmentInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftEnvironmentInput")
            .field("repo", &self.repo)
            .field("base_ref", &self.base_ref)
            .field("goal", &self.goal)
            .field("agent_connector", &self.agent_connector)
            .field("aws_profile", &self.aws_profile)
            .field("region", &self.region)
            .field("time_limit_seconds", &self.time_limit_seconds)
            .field("config_path", &self.config_path)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentDraftPhase {
    Reading,
    Drafting,
    Validating,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDraftProgress {
    pub draft_id: String,
    pub phase: EnvironmentDraftPhase,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentDraftStatus {
    Drafting,
    Ready,
    Failed,
    Approved,
}

impl EnvironmentDraftStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Drafting => "drafting",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Approved => "approved",
        }
    }
}

impl fmt::Display for EnvironmentDraftStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageFrom {
    User,
    Agent,
}

/// One turn of the revise loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentDraftMessage {
    pub from: MessageFrom,
    pub text: String,
    /// Epoch ms.
    pub at: i64,
}

/// A draft as the service keeps it: the draft itself plus where it stands in the draft / revise / approve loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDraftRecord {
    pub id: String,
    pub repo: String,
    pub base_ref: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_profile: Option<String>,
    pub region: String,
    pub status: EnvironmentDraftStatus,
    /// 1 for the first draft, +1 for every `revise` (0 while the first draft is being made).
    pub revision: u32,
    /// Set unless `status` is `failed` before the first draft.
    pub draft: Option<EnvironmentDraft>,
    /// The user's change requests and the agent's replies.
    pub messages: Vec<EnvironmentDraftMessage>,
    /// Why drafting failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Set once approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<EnvironmentApproval>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentApprovalMode {
    /// Straight onto `base_ref`.
    Commit,
    /// A branch and a pull request the user merges (the default).
    #[default]
    Pr,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApproveEnvironmentOptions {
    /// `commit`: straight onto `base_ref`. `pr`: a branch and a pull request the user merges. Default `pr`.
    pub mode: Option<EnvironmentApprovalMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentApproval {
    pub mode: EnvironmentApprovalMode,
    /// The commit holding the files (on `base_ref` for `commit`, on `branch` for `pr`).
    pub sha: String,
    /// `pr` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `pr` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// Repo paths written.
    pub files: Vec<String>,
    /// The environment is locked from here (for `pr`: once the PR is merged): agents' candidates touching it are
    /// blocked.
    pub locked: bool,
    pub at: i64,
}

/// Drafts, revises and approves an optimization's environment. Real implementation:
/// [`crate::AgentEnvironmentDrafter`] (a setup agent, the user's Claude Code / Codex, local, writes the files;
/// Ratchet validates and costs them); mock: [`crate::MockEnvironmentDrafter`] (deterministic drafts per scope for
/// simulation, demos and tests). Implementations never apply anything: applying is the engine's job, at run
/// start, from the protected ref.
///
/// `draft` and `revise` are `begin_*` (synchronous: validate the request, record it as `drafting`) plus `run_*`
/// (the agent's turn); a service that answers before the agent is done calls the halves itself.
#[async_trait]
pub trait EnvironmentDrafter: Send + Sync {
    /// Validate the request and record a draft in status `drafting` (revision 0). Nothing runs yet.
    fn begin_draft(&self, input: DraftEnvironmentInput)
    -> crate::EnvResult<EnvironmentDraftRecord>;
    /// Do the work of a draft begun with [`Self::begin_draft`] and return its final record (status `ready`, or
    /// `failed` with the reason in `error`).
    async fn run_draft(&self, draft_id: &str) -> crate::EnvResult<EnvironmentDraftRecord>;
    /// Record a change request ("use a smaller setup", "use RDS instead of a Postgres container") and set the draft
    /// back to `drafting`. Refuses an approved, failed or already-drafting draft.
    fn begin_revise(
        &self,
        draft_id: &str,
        message: &str,
    ) -> crate::EnvResult<EnvironmentDraftRecord>;
    /// Ask the same agent for the change; re-validates and re-costs. A failed revision restores the earlier draft
    /// (`status` back to what it was, the reason in `error` and in the messages).
    async fn run_revise(&self, draft_id: &str) -> crate::EnvResult<EnvironmentDraftRecord>;
    /// Commit the files to the repo's protected ref (directly or as a PR). Refuses a draft whose checks did not
    /// all pass.
    async fn approve(
        &self,
        draft_id: &str,
        options: ApproveEnvironmentOptions,
    ) -> crate::EnvResult<EnvironmentDraftRecord>;
    fn get(&self, draft_id: &str) -> Option<EnvironmentDraftRecord>;
    /// Newest first.
    fn list(&self) -> Vec<EnvironmentDraftRecord>;
    /// Forget a draft that was not approved ("Back").
    fn discard(&self, draft_id: &str) -> crate::EnvResult<()>;

    async fn draft(
        &self,
        input: DraftEnvironmentInput,
    ) -> crate::EnvResult<EnvironmentDraftRecord> {
        let rec = self.begin_draft(input)?;
        self.run_draft(&rec.id).await
    }

    async fn revise(
        &self,
        draft_id: &str,
        message: &str,
    ) -> crate::EnvResult<EnvironmentDraftRecord> {
        self.begin_revise(draft_id, message)?;
        self.run_revise(draft_id).await
    }
}

/// What [`EnvironmentChecker::plan`] is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPlanInput {
    pub scope: EnvironmentScope,
    /// Repo paths under `infra/ratchet/`.
    pub files: Vec<EnvironmentFile>,
    pub region: String,
    pub aws_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPlanOutcome {
    pub ok: bool,
    pub detail: String,
}

/// Runs `tofu validate` and `tofu plan` on a drafted module in the user's AWS account (the "plans cleanly in your
/// account" check). Real implementation: `TofuEnvironmentChecker` in `ratchet-sandbox`. Without one, a draft with
/// OpenTofu cannot pass the checks: Ratchet does not claim a plan it did not run.
#[async_trait]
pub trait EnvironmentChecker: Send + Sync {
    async fn plan(&self, input: &EnvironmentPlanInput) -> PortResult<EnvironmentPlanOutcome>;
}
