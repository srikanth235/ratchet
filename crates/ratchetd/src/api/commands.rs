//! Request bodies and query parameters of the commands and reads.

use ratchet_core::enums::{AgentKind, AgentsWhere};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::connectors::ConnectorSelectionInput;
use super::vm::KpiRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum DemoId {
    #[serde(rename = "demo-search")]
    Search,
    #[serde(rename = "demo-cli")]
    Cli,
    #[serde(rename = "demo-forecast")]
    Forecast,
}

impl DemoId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "demo-search",
            Self::Cli => "demo-cli",
            Self::Forecast => "demo-forecast",
        }
    }
}

/// `POST /v1/optimizations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateOptimizationInput {
    pub name: Option<String>,
    /// A GitHub repo (`owner/name` or a GitHub URL). Ignored for a demo.
    #[serde(default)]
    pub repo_url: String,
    #[serde(default = "default_base_ref")]
    pub base_ref: String,
    /// Run a demo template in simulation (mock agents, no setup) instead of a repo.
    pub demo: Option<DemoId>,
    pub agents: Vec<AgentKind>,
    pub agents_where: Option<AgentsWhere>,
    pub budget_usd: f64,
    pub time_limit_seconds: u64,
    pub plateau_evals: u32,
    /// Provisional (connectors): which saved connectors this run uses.
    pub connectors: Option<ConnectorSelectionInput>,
    /// Provisional (environment drafts): the approved draft whose environment this run uses.
    pub environment_draft_id: Option<String>,
}

fn default_base_ref() -> String {
    "main".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedOptimization {
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StopOptimizationInput {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageAgentInput {
    pub text: String,
}

/// `POST /v1/preflight`: "what's missing" for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PreflightSource {
    /// A demo needs nothing.
    Demo,
    /// `github` / `local` need GitHub, AWS and agents.
    Github,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreflightInput {
    pub agents: Vec<AgentKind>,
    pub agents_where: AgentsWhere,
    pub source: PreflightSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateConfigInput {
    /// The ratchet.yaml text.
    pub yaml: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestVm {
    pub url: String,
}

/// The markdown report of an optimization (the app offers it in a save dialog, the CLI writes it to a file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportVm {
    pub file_name: String,
    pub markdown: String,
}

// ---- query parameters -----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct RangeQuery {
    /// KPI window; default `30d`.
    pub range: Option<KpiRange>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct MetricQuery {
    /// One of the series' `options` keys; default the objective.
    pub metric: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuery {
    pub optimization_id: Option<String>,
    /// 1..=200, default 20.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventsQuery {
    /// Return only events newer than this id. Without it: the newest `limit` events.
    pub cursor: Option<i64>,
    /// 1..=1000, default 200.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ReposQuery {
    /// Substring of `owner/name`.
    pub query: Option<String>,
}
