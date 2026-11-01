//! Repos (GitHub list and inspect, local folders), templates, environments (sandboxes).

use ratchet_core::config::ConfigError;
use ratchet_core::enums::TemplateKind;
use ratchet_core::templates::TemplateInfo;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::vm::{ConfigSummary, TeardownReceiptVm};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoVm {
    /// `owner/name`
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
    #[schema(required = true)]
    pub description: Option<String>,
    #[schema(required = true)]
    pub pushed_at: Option<String>,
}

/// Where an optimization starts from: a GitHub repo of the signed-in account, or a local folder (this-machine runs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RepoSource {
    Github { repo: String },
    Local { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectRepoInput {
    pub source: RepoSource,
    /// Read the repo with this GitHub connector (default: the default one).
    pub connector_id: Option<String>,
}

/// What step 1 learned about the chosen repo (read from the default branch, never typed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoInspectVm {
    pub source: RepoSource,
    /// `owner/name` (also for a local folder, from its `origin`); `null` for a local folder with no GitHub remote.
    #[schema(required = true)]
    pub repo: Option<String>,
    pub base_ref: String,
    /// The ratchet.yaml text, read-only; `null` when the repo has none.
    #[schema(required = true)]
    pub config_yaml: Option<String>,
    /// Parsed summary; `null` when there is no ratchet.yaml or it does not parse.
    #[schema(required = true)]
    pub config: Option<ConfigSummary>,
    pub errors: Vec<ConfigError>,
    /// Offered when there is no ratchet.yaml: "Add via pull request".
    pub templates: Vec<TemplateInfo>,
    /// Why the repo cannot be used here (a local folder with no GitHub remote, not a git repo, ...).
    #[schema(required = true)]
    pub problem: Option<String>,
}

/// Filter of `GET /v1/templates`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TemplatesQuery {
    /// `template` (runnable evaluators) or `demo` ("Try a demo"); default both.
    pub kind: Option<TemplateKind>,
}

// ---- environments -------------------------------------------------------------------------------------------

/// Phase of an environment (sandbox) as seen from outside. `none`: the ledger has it, nothing reported a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentPhase {
    None,
    Creating,
    Ready,
    Idle,
    Destroying,
    Destroyed,
    Failed,
}

/// One automatic environment, as seen from outside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentVm {
    pub sandbox_id: String,
    #[schema(required = true)]
    pub region: Option<String>,
    pub phase: EnvironmentPhase,
    /// The last phase message ("environment ready").
    pub text: String,
    pub simulated: bool,
    pub active_runs: Vec<String>,
    #[schema(required = true)]
    pub instances_running: Option<u64>,
    /// Estimated spend of the runs that are active in it.
    pub spend_usd: f64,
    /// When the automatic destroy is due.
    #[schema(required = true)]
    pub destroy_at: Option<i64>,
    #[schema(required = true)]
    pub idle_since: Option<i64>,
    /// Ending it now is possible (no run is active).
    pub can_end: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentsVm {
    pub environments: Vec<EnvironmentVm>,
    /// The most recent teardown across environments.
    #[schema(required = true)]
    pub last_receipt: Option<TeardownReceiptVm>,
    /// Idle window before an unused environment is removed, minutes.
    pub idle_minutes: u32,
}
