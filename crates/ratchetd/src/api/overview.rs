//! Overview (PROVISIONAL): spent / value locked in / return, the value staircase, "needs you", the table.
//!
//! Shapes come from `design/v02/1-overview.png` and `design/v02.html`. The TypeScript feature is still in flight
//! on another branch; these types may change before it lands (routes are tagged `overview (provisional)`).

use ratchet_core::enums::ValueBasis;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub enum OverviewRange {
    #[serde(rename = "30d")]
    Days30,
    #[default]
    #[serde(rename = "90d")]
    Days90,
    #[serde(rename = "all")]
    All,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OverviewQuery {
    /// Window of the hero numbers and the staircase; default `90d`.
    pub range: Option<OverviewRange>,
}

/// "Spent $184 of $500 across AWS and agent keys".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpentHero {
    pub usd: f64,
    /// The total monthly caps of the connected AWS accounts and keys, when any is set.
    #[schema(required = true)]
    pub cap_usd: Option<f64>,
    /// 0..=1, for the thin bar; `null` without a cap.
    #[schema(required = true)]
    pub fraction_of_cap: Option<f64>,
    /// "across AWS and agent keys"
    pub note: String,
}

/// "Value locked in $15.4k / yr from 9 verified wins (estimated)".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValueHero {
    pub usd_per_year: f64,
    pub wins: i64,
    pub basis: ValueBasis,
    pub note: String,
}

/// "Return 84x: yearly value for every $1 spent, paid back in 4 days".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReturnHero {
    /// `null` when nothing was spent.
    #[schema(required = true)]
    pub ratio: Option<f64>,
    #[schema(required = true)]
    pub payback_days: Option<f64>,
    pub note: String,
}

/// One verified win on the staircase. `usdPerYear` is cumulative after this win.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StaircasePoint {
    pub at: i64,
    pub usd_per_year: f64,
    pub optimization_id: String,
    pub optimization_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Staircase {
    pub from: i64,
    pub to: i64,
    pub points: Vec<StaircasePoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NeedsYouKind {
    ApproveEnvironment,
    MergeWins,
    SignIn,
    BudgetLimit,
}

/// One item of "Needs you". The button opens `optimizationId` (or the draft / connector) in the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NeedsYouItem {
    pub id: String,
    pub kind: NeedsYouKind,
    pub title: String,
    pub subtitle: String,
    /// The button: "Review", "Open PR".
    pub action_label: String,
    #[schema(required = true)]
    pub optimization_id: Option<String>,
    #[schema(required = true)]
    pub environment_draft_id: Option<String>,
    #[schema(required = true)]
    pub connector_id: Option<String>,
    /// For `merge-wins`: the pull request to open.
    #[schema(required = true)]
    pub url: Option<String>,
}

/// The state dot of an Overview row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OverviewState {
    Running,
    WaitingOnYou,
    Merged,
    Done,
    NoWin,
    Draft,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewRow {
    pub optimization_id: String,
    pub name: String,
    pub repo: String,
    pub state: OverviewState,
    /// "Running", "Waiting on you", "Merged", "Done", "No win".
    pub state_text: String,
    /// Under the state: "1 h 12 m left".
    #[schema(required = true)]
    pub state_note: Option<String>,
    /// Signed percent change of the metric, positive = better.
    #[schema(required = true)]
    pub result_pct: Option<f64>,
    /// "412 -> 288 ms"
    pub result_text: String,
    pub spent_usd: f64,
    #[schema(required = true)]
    pub budget_usd: Option<f64>,
    #[schema(required = true)]
    pub value_usd_per_year: Option<f64>,
}

/// The sandbox line at the bottom of the sidebar ("Sandbox on · acme-prod, $1.20 so far · off at 15:40").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxChip {
    pub on: bool,
    #[schema(required = true)]
    pub name: Option<String>,
    pub spent_usd: f64,
    #[schema(required = true)]
    pub off_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewVm {
    pub range: OverviewRange,
    pub spent: SpentHero,
    pub value_locked_in: ValueHero,
    pub return_on_spend: ReturnHero,
    pub staircase: Staircase,
    pub needs_you: Vec<NeedsYouItem>,
    pub rows: Vec<OverviewRow>,
    pub sandbox: SandboxChip,
}
