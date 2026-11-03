//! Typed arguments of the session tools. They derive `JsonSchema`, so `tools/list` shows exactly what the tool accepts and the
//! daemon rejects anything else. Enums come from `ratchet-core`.

use ratchet_core::enums::{AgentKind, TemplateKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The window of a KPI summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Range {
    #[serde(rename = "7d")]
    Days7,
    #[serde(rename = "30d")]
    Days30,
    #[serde(rename = "90d")]
    Days90,
}

impl Range {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::Days90 => "90d",
        }
    }
}

/// The overview window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum OverviewRange {
    #[serde(rename = "30d")]
    Days30,
    #[serde(rename = "90d")]
    Days90,
    #[serde(rename = "all")]
    All,
}

impl OverviewRange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Days30 => "30d",
            Self::Days90 => "90d",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListOptimizationsArgs {
    /// KPI window for the summary (default 30d).
    pub range: Option<Range>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizationArgs {
    /// Optimization id, from `list_optimizations`.
    pub optimization: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunArgs {
    /// Run id. A run is identified by its optimization id.
    pub run: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverviewArgs {
    /// Window (default 90d).
    pub range: Option<OverviewRange>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTemplatesArgs {
    /// Only evaluator templates, or only demos.
    pub kind: Option<TemplateKind>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListReposArgs {
    /// Substring of owner/name.
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftOptimizationArgs {
    /// GitHub repo, `owner/name`.
    pub repo: String,
    /// What to improve, in the user's words. Becomes the brief the agents read (RATCHET.md).
    pub goal: String,
    /// Evaluator template id from `list_templates` (default: `test-suite-time`).
    pub template: Option<String>,
    /// Objective metric, a dotted path into the evaluator's JSON output (default: the template's).
    pub metric: Option<String>,
    /// When to run again, in plain words ("nightly"). Kept on the draft card.
    pub schedule: Option<String>,
    /// Budget in USD (default 50).
    pub budget: Option<f64>,
    /// Agents to run (default: claude-code).
    pub agents: Option<Vec<AgentKind>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOptimizationArgs {
    /// Draft id from `draft_optimization`.
    pub draft_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteeringNoteArgs {
    /// Optimization id.
    pub optimization: String,
    /// The note. The agents read it at the start of their next turn.
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenPrArgs {
    /// Optimization id.
    pub optimization: String,
    /// A candidate sha from `get_run`; checked to be part of the run.
    pub candidate: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryArgs {
    /// Most recent entries to return (default 20).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorQueryArgs {
    /// Connector id from the optimization's brief.
    pub connector: String,
    /// A read-only query in the connector's language.
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitCandidateArgs {
    /// Commit sha on this agent's branch.
    pub sha: String,
    /// One line on what changed and why.
    pub note: Option<String>,
}
