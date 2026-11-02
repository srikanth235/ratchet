//! Overview: what was spent, what the verified wins are worth, and what is waiting on the user, over a range.
//!
//! The shapes are `packages/desktop/src/shared/overview.ts` (`OverviewVM` and its parts), one to one. Everything is
//! computed from the ledger (`crate::views::overview`); value uses the same model as the Result screen, so the
//! staircase, the hero number and the table's Value column always add up.

use ratchet_core::enums::ValueBasis;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::value::ValueModelVm;
use super::vm::UiStatus;

/// Window the Overview covers. `all` = everything in the ledger.
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

/// Dot colour of a status word: brass (running), amber (a person has something to do: ready to ship), mint (done), grey (nothing), rose (failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StatusTone {
    Run,
    Wait,
    Ok,
    Off,
    Bad,
}

/// One step of the "value locked in" staircase: a verified win with a value model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewWin {
    /// When the ratchet moved (epoch ms).
    pub at: i64,
    /// Cumulative value locked in after this win, USD per year.
    pub cumulative_usd_per_year: f64,
    /// What this win added, USD per year.
    pub usd_per_year: f64,
    pub optimization_id: String,
    pub optimization: String,
    /// Where the win stands: merged (delivered), in an open pull request (pending) or not shipped yet.
    pub state: OverviewValueState,
}

/// Whether the value of an optimization's wins counts as delivered yet. Only a merged pull request delivers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverviewValueState {
    /// The pull request is merged: the value is in the code.
    Delivered,
    /// A pull request is open: the value counts once it merges.
    Pending,
    /// Verified wins with no pull request yet ("Ready to ship").
    Ready,
    /// No verified win.
    None,
}

/// What the screen should say when there is nothing to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OverviewPhase {
    /// No optimization at all.
    Empty,
    /// Optimizations exist but none has a verified win yet.
    NoWins,
    /// At least one verified win.
    Wins,
}

/// The calm empty state: a title and one sentence. Present only when `phase` is not `wins`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewEmptyState {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverviewNeedKind {
    /// Verified wins that have no pull request yet.
    Ship,
    /// The run failed.
    Failed,
    /// A connector is signed out or its sign-in expired. Only a person can sign in again; runs carry on without it.
    Connector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewNeed {
    pub id: String,
    pub kind: OverviewNeedKind,
    pub optimization_id: String,
    pub title: String,
    pub sub: String,
    /// Button label.
    pub action: String,
}

/// `lower` = a smaller metric is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverviewDirection {
    Lower,
    Higher,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewRow {
    pub id: String,
    pub name: String,
    pub repo: String,
    /// Ledger status as the pills use it (`pr-open` once a pull request exists).
    pub status: UiStatus,
    /// The status word shown next to the dot ("Running", "Ready to ship", "No win").
    pub status_label: String,
    pub tone: StatusTone,
    /// Seconds left of the time limit while running, else `null`.
    #[schema(required = true)]
    pub remaining_s: Option<i64>,
    pub metric: String,
    pub unit: String,
    pub direction: OverviewDirection,
    #[schema(required = true)]
    pub baseline: Option<f64>,
    #[schema(required = true)]
    pub best: Option<f64>,
    /// Signed gain vs baseline in percent (positive = better); `null` without a baseline.
    #[schema(required = true)]
    pub gain_pct: Option<f64>,
    pub verified_wins: i64,
    /// Spend in the range (lifetime when the range is `all`).
    pub spent_usd: f64,
    pub budget_usd: f64,
    pub live: bool,
    /// Value of the wins in the range, USD per year; `null` without a value model.
    #[schema(required = true)]
    pub value_usd_per_year: Option<f64>,
    /// The pull request, once one exists. `merged` is what turns the value into delivered value.
    #[schema(required = true)]
    pub pull_request: Option<OverviewPullRequest>,
    /// Delivered (merged), pending (open pull request), ready (no pull request yet) or none.
    pub value_state: OverviewValueState,
    /// True while the value only counts once a pull request merges (`value_state` is `pending` or `ready`): the row shows
    /// its return dimmed with "pending".
    pub pending: bool,
    /// Yearly value per $1 spent on this optimization in the range; `null` without value or spend.
    #[serde(rename = "return")]
    #[schema(required = true)]
    pub return_x: Option<f64>,
    /// The value model behind `valueUsdPerYear` (the Value row); `null` when the optimization has none.
    #[schema(required = true)]
    pub value_model: Option<ValueModelVm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewPullRequest {
    pub url: String,
    pub merged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewSpent {
    /// Real dollars in the range: `awsUsd` + `agentsApiUsd`. Agents on the user's own plan cost nothing extra and are not in it.
    pub usd: f64,
    /// Budget of the optimizations listed in `rows`.
    pub budget_usd: f64,
    pub optimizations: i64,
    /// Environments, runners and storage on AWS.
    pub aws_usd: f64,
    /// Model tokens billed per token through a key.
    pub agents_api_usd: f64,
    /// Tokens used on the user's own plan ("included in your plan"): counted, never in `usd`.
    pub included_tokens: i64,
    /// "AWS $121 · agents $63", or "AWS $121 · agents included in your plan (2.4M tokens)".
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewValue {
    pub usd_per_year: f64,
    pub wins: i64,
    pub basis: ValueBasis,
}

/// Yearly value per $1 spent, and days until the spend is earned back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewReturn {
    pub ratio: f64,
    pub payback_days: f64,
}

/// Wins counted in dollars per year.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValueAmount {
    pub usd_per_year: f64,
    pub wins: i64,
}

/// Wins whose pull request is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingAmount {
    pub usd_per_year: f64,
    pub wins: i64,
    /// Open pull requests these wins are in ("$2.9k pending in 1 PR").
    pub pull_requests: i64,
}

/// The one value story Home and Overview both tell: what was spent, what is delivered, what is pending, what it returned.
///
/// `delivered` counts only wins in merged pull requests; `pending` those in open ones; `readyToShip` verified wins with no pull
/// request yet. `return` is `delivered` divided by `spentUsd` (real dollars only), `paybackDays` how long the spend takes to earn
/// back at that rate. Both are `null` until something is delivered and something was spent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValueStory {
    /// `measured` only when every counted win has a measured value model.
    pub basis: ValueBasis,
    pub delivered: ValueAmount,
    pub pending: PendingAmount,
    pub ready_to_ship: ValueAmount,
    /// Real dollars in the range (the same number as `spent.usd`).
    pub spent_usd: f64,
    /// Yearly value delivered per $1 spent ("68×"); `null` without delivered value or spend.
    #[serde(rename = "return")]
    #[schema(required = true)]
    pub return_x: Option<f64>,
    /// Days until the spend is earned back at the delivered rate.
    #[schema(required = true)]
    pub payback_days: Option<f64>,
    /// The sentence both screens show: "$12.5k delivered + $2.9k pending · 68×". Empty without a value model.
    pub line: String,
}

/// "Next 30 days": what the schedules will start and what that will cost, from what each scheduled optimization's last run cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewForecast {
    pub days: i64,
    /// Runs the schedules start in the window.
    pub scheduled_runs: i64,
    /// Scheduled optimizations whose runs cannot be counted ahead ("on every push").
    pub unforecast: i64,
    /// Expected real dollars: `expectedAwsUsd` + `expectedAgentsApiUsd`.
    pub expected_usd: f64,
    pub expected_aws_usd: f64,
    pub expected_agents_api_usd: f64,
    /// Tokens the plan will cover ("included in your plan").
    pub expected_included_tokens: i64,
    /// The ceiling: the budgets of the optimizations that run (`spent.budgetUsd`). Spend stops at it.
    #[schema(required = true)]
    pub hard_cap_usd: Option<f64>,
    /// "62 runs · ~$140 · cap $500".
    pub line: String,
}

/// One point of the spend line and the value staircase: cumulative real dollars and the yearly value counted by then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewChartPoint {
    pub at: i64,
    /// Real dollars spent from the start of the window up to `at`.
    pub spent_usd: f64,
    /// Yearly value of merged wins up to `at`.
    pub delivered_usd_per_year: f64,
    /// Yearly value of wins in open pull requests up to `at` (the dashed step).
    pub pending_usd_per_year: f64,
    /// Yearly value of wins with no pull request yet up to `at`.
    pub ready_usd_per_year: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewVm {
    pub range: OverviewRange,
    /// The window, epoch ms. `from` is the first activity for `all`.
    pub from: i64,
    pub to: i64,
    /// The ledger has no optimization at all (the Overview shows one calm empty state).
    pub empty: bool,
    /// `empty` (no optimization), `no-wins` (some, none proven yet) or `wins`.
    pub phase: OverviewPhase,
    /// What to say when `phase` is not `wins`: "No wins yet" and one sentence.
    #[schema(required = true)]
    pub empty_state: Option<OverviewEmptyState>,
    /// The value story, the same object `GET /v1/home` returns.
    pub story: ValueStory,
    pub forecast: OverviewForecast,
    /// Points of the "Value delivered vs spend" chart, oldest first, from the start of the window to now.
    pub chart: Vec<OverviewChartPoint>,
    pub spent: OverviewSpent,
    /// Every verified win in the range, merged or not (see `story` for what is delivered). `null` when no optimization has a
    /// value model ("Set a value").
    #[schema(required = true)]
    pub value: Option<OverviewValue>,
    /// `value` divided by real dollars spent; `story.return` is the one the screens show (delivered value only). `null` without
    /// value or spend.
    #[schema(required = true)]
    pub return_on_spend: Option<OverviewReturn>,
    /// Staircase of cumulative value, oldest first; empty without a value model or wins in the range.
    pub wins: Vec<OverviewWin>,
    pub needs_you: Vec<OverviewNeed>,
    /// Optimizations with activity in the range (drafts created in it included), newest first.
    pub rows: Vec<OverviewRow>,
}
