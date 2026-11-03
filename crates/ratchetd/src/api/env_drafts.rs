//! Dynamic environment drafts: a setup agent drafts the environment an optimization needs, the user reads it, asks
//! for changes, and approves (docs/SIMPLIFIED_MODEL.md, "Dynamic environments").
//!
//! The wire shapes are the TypeScript ones (`EnvironmentDraftRecord` and friends of `packages/core/src/environment`):
//! `packages/contract/src/contract.check.ts` compares them field by field. The Rust side of the flow is the
//! `ratchet-env` crate; these types are its JSON, described for OpenAPI.
//!
//! Life of a draft: `drafting` -> `ready` -> (`drafting` -> `ready`)* -> `approved`, or `failed`. A draft is
//! `drafting` while the agent works (the first draft has `revision` 0 and no `draft` yet; a revision keeps the
//! earlier one). `approved` is final: the files are on the protected ref (or in a pull request) and locked, agents
//! change code only, never the environment.

use ratchet_core::enums::{AgentKind, EnvironmentScope};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentDraftStatus {
    Drafting,
    Ready,
    Failed,
    Approved,
}

/// What a machine of the environment does (the `role` of the base module's `machines` map).
/// Which step an environment draft is in while its agent works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentDraftPhase {
    Reading,
    Drafting,
    Validating,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentRole {
    App,
    Database,
    Cache,
    Load,
    Evaluator,
    Other,
}

/// One row of "What gets created".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentResourceVm {
    /// "API", "Postgres 16", "Load generator".
    pub name: String,
    pub role: EnvironmentRole,
    /// Instance type, "c7i.2xlarge".
    pub machine: String,
    /// One plain line: "shop-api in the repo's container".
    pub detail: String,
    pub usd_per_hour: f64,
}

/// A file of the draft: `ratchet.yaml`, or a file under `infra/ratchet/` (OpenTofu, Dockerfile, seed script).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EnvironmentFileVm {
    /// Repo-relative POSIX path.
    pub path: String,
    pub content: String,
}

/// One of Ratchet's checks on a draft: isolated, results sealed, same hardware every time, full teardown, ...
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EnvironmentCheckVm {
    pub id: String,
    /// Plain words for the UI.
    pub label: String,
    pub ok: bool,
    /// Why it passed or failed, one line.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCostVm {
    /// Sum of the resources' hourly prices (excludes the sandbox's fixed private-network cost, ~$0.04/h).
    pub usd_per_hour: f64,
    /// Minutes until the environment is ready to measure.
    pub startup_minutes: u32,
    /// `usd_per_hour x (run hours + start-up)` for the run's time limit.
    pub estimate_for_run_usd: f64,
}

/// What the setup agent proposed, as "New optimization > Environment" shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentProposalVm {
    pub scope: EnvironmentScope,
    /// Why this scope, in a sentence or two.
    pub reason: String,
    pub resources: Vec<EnvironmentResourceVm>,
    /// What would be committed: `ratchet.yaml` plus the files under `infra/ratchet/`, with their content ("View files").
    pub files: Vec<EnvironmentFileVm>,
    pub checks: Vec<EnvironmentCheckVm>,
    pub cost: EnvironmentCostVm,
    /// Which connector drafted it.
    pub agent: AgentKind,
    /// How long drafting took, seconds (all revisions).
    pub duration_s: u64,
    /// What the setup agent cost in model tokens, USD (all revisions).
    pub usd_spent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentMessageFrom {
    User,
    Agent,
}

/// One turn of the revise loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EnvironmentMessageVm {
    pub from: EnvironmentMessageFrom,
    pub text: String,
    /// Epoch ms.
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentApprovalMode {
    /// Straight onto the base ref.
    Commit,
    /// A branch and a pull request the user merges (the default).
    Pr,
}

/// Where the approved files went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentApprovalVm {
    pub mode: EnvironmentApprovalMode,
    /// The commit holding the files (on the base ref for `commit`, on `branch` for `pr`).
    pub sha: String,
    /// `pr` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub branch: Option<String>,
    /// `pr` only: "Open as PR".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub pr_url: Option<String>,
    /// Repo paths written.
    pub files: Vec<String>,
    /// Always true: the environment is locked from here (for `pr`: once the PR is merged), a candidate that touches it
    /// is blocked.
    pub locked: bool,
    pub at: i64,
}

/// A draft and where it stands in the draft / revise / approve loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDraftVm {
    pub id: String,
    pub repo: String,
    pub base_ref: String,
    /// The goal the user typed ("Make checkout p95 faster").
    pub goal: String,
    /// The AWS profile `tofu plan` is checked against.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub aws_profile: Option<String>,
    pub region: String,
    pub status: EnvironmentDraftStatus,
    /// 1 for the first draft, +1 for every revision; 0 while the first draft is being made.
    pub revision: u32,
    /// `null` until the first draft is ready (and when it failed).
    #[schema(required = true)]
    pub draft: Option<EnvironmentProposalVm>,
    /// The user's change requests and the agent's replies.
    pub messages: Vec<EnvironmentMessageVm>,
    /// Why drafting (or the last revision) failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
    /// Set once approved.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub approval: Option<EnvironmentApprovalVm>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `GET /v1/environment-drafts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EnvironmentDraftsVm {
    /// Newest first.
    pub drafts: Vec<EnvironmentDraftVm>,
}

/// `POST /v1/environment-drafts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateEnvironmentDraftInput {
    /// The repository: `owner/name` or its GitHub URL.
    pub repo: String,
    /// The protected ref the files are committed to / the PR targets. Default `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    /// What the user wants optimized, in their words.
    pub goal: String,
    /// The user's agent connector that does the drafting (their own login, run on this machine).
    pub agent_connector: AgentKind,
    /// The AWS profile selected in Settings: `tofu plan` is checked against that account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_profile: Option<String>,
    /// Region of the sandbox (default: `environment.region` of ratchet.yaml, else the default region).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// The run's time limit, for the cost estimate (default 2 h).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_seconds: Option<f64>,
    /// Path of the config inside the repo (default `ratchet.yaml`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
}

/// `POST /v1/environment-drafts/{id}/revisions`: "Ask for a change", or "Use a smaller setup".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReviseEnvironmentDraftInput {
    /// Plain words: "use a smaller setup", "use RDS instead of a Postgres container".
    pub message: String,
}

/// `POST /v1/environment-drafts/{id}/approval`. The body is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApproveEnvironmentDraftInput {
    /// `pr` (default): a branch and a pull request the user merges. `commit`: straight onto the base ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<EnvironmentApprovalMode>,
}
