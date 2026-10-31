//! Dynamic environment drafts (PROVISIONAL): an agent drafts the environment an optimization needs, the user reads
//! it, asks for changes, and approves.
//!
//! Shapes come from `design/v02/4-new-environment.png` and `docs/NATIVE_ARCHITECTURE.md` (`ratchet-env`: scope
//! none | single | full, validator, cost estimate, approve and lock). The TypeScript feature is still in flight on
//! another branch; these types may change before it lands (routes are tagged `environment drafts (provisional)`).
//!
//! Life of a draft: `drafting` -> `ready` -> (`revising` -> `ready`)* -> `approved`. Approving locks it: agents
//! change code only, never the environment.

use ratchet_core::enums::AgentKind;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How much environment the task needs. `none`: the evaluator runs on the host as is; `single`: one machine;
/// `full`: several services (API + database + cache + load generator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvScope {
    None,
    Single,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentDraftStatus {
    Drafting,
    Ready,
    Revising,
    Approved,
    Failed,
}

/// One thing the environment creates (a row of "What gets created").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvResource {
    pub id: String,
    /// "API", "Postgres 16".
    pub name: String,
    /// "shop-api in the repo's container".
    pub description: String,
    /// Instance type, "c7i.2xlarge".
    pub machine: String,
    pub hourly_usd: f64,
}

/// A file the draft would add (Terraform, Dockerfile, seed script). The content is for "View files".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Ok,
    Warn,
    Fail,
}

/// One of Ratchet's checks on a draft: isolated, results sealed, same hardware every time, full teardown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvCheck {
    pub id: String,
    pub label: String,
    pub state: CheckState,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvCost {
    pub hourly_usd: f64,
    /// "ready in ~6 min"
    pub ready_minutes: u32,
    /// "about $1.18 for a 2 h run"
    pub run_hours: f64,
    pub estimated_run_usd: f64,
}

/// Who drafted it and what that cost ("Drafted by Claude Code from the repo and your goal, 2 min, $0.31").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftProvenance {
    pub agent: AgentKind,
    pub duration_s: u32,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDraftVm {
    pub id: String,
    pub status: EnvironmentDraftStatus,
    pub repo: String,
    pub base_ref: String,
    /// The goal the user typed ("Make checkout p95 faster").
    pub goal: String,
    pub scope: EnvScope,
    /// The serif quote: "This task needs the full stack: the API, Postgres with seeded orders, ...".
    pub summary: String,
    /// The note under it: why this much environment, and the "Use a smaller setup" hint.
    pub rationale: String,
    pub provenance: DraftProvenance,
    pub resources: Vec<EnvResource>,
    /// File paths only; read them with `GET /v1/environment-drafts/{id}/files`.
    pub file_paths: Vec<String>,
    pub cost: EnvCost,
    pub checks: Vec<EnvCheck>,
    /// All checks passed ("Passed Ratchet's checks"): required to approve.
    pub checks_passed: bool,
    /// Starts at 1, +1 per revision.
    pub revision: u32,
    /// Set once approved (the environment is locked from then on).
    #[schema(required = true)]
    pub approved_at: Option<i64>,
    /// "Open as PR": the pull request that adds the environment files to the repo.
    #[schema(required = true)]
    pub pr_url: Option<String>,
    pub created_at: i64,
}

/// `POST /v1/environment-drafts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateEnvironmentDraftInput {
    pub repo: String,
    #[serde(default = "default_base_ref")]
    pub base_ref: String,
    pub goal: String,
    /// Which agent drafts it; default the first signed-in one.
    pub agent: Option<AgentKind>,
    /// Skip the agent's judgement of how much environment is needed.
    pub scope: Option<EnvScope>,
}

fn default_base_ref() -> String {
    "main".into()
}

/// `POST /v1/environment-drafts/{id}/revisions`: "Ask for a change", or "Use a smaller setup".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviseEnvironmentDraftInput {
    /// Plain words: "use RDS instead of a Postgres container".
    pub request: String,
    pub scope: Option<EnvScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvFilesVm {
    pub files: Vec<EnvFile>,
}
