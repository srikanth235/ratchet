//! Overview: what was spent, what the verified wins are worth, and what is waiting on the user, over a range.
//!
//! The shapes are `packages/desktop/src/shared/overview.ts` (`OverviewVM` and its parts), one to one. Everything is
//! computed from the ledger (`crate::views::overview`); value uses the same model as the Result screen, so the
//! staircase, the hero number and the table's Value column always add up.

use ratchet_core::enums::ValueBasis;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

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

/// Dot colour of a status word: brass (running), amber (waiting on you), mint (done), grey (nothing), rose (failed).
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverviewNeedKind {
    /// Verified wins that have no pull request yet.
    Ship,
    /// The run failed.
    Failed,
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
    /// The status word shown next to the dot ("Running", "Waiting on you", "No win").
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewSpent {
    pub usd: f64,
    /// Budget of the optimizations listed in `rows`.
    pub budget_usd: f64,
    pub optimizations: i64,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewVm {
    pub range: OverviewRange,
    /// The window, epoch ms. `from` is the first activity for `all`.
    pub from: i64,
    pub to: i64,
    /// The ledger has no optimization at all (the Overview shows one calm empty state).
    pub empty: bool,
    pub spent: OverviewSpent,
    /// `null` when no optimization has a value model ("Set a value").
    #[schema(required = true)]
    pub value: Option<OverviewValue>,
    /// `null` without value or spend.
    #[schema(required = true)]
    pub return_on_spend: Option<OverviewReturn>,
    /// Staircase of cumulative value, oldest first; empty without a value model or wins in the range.
    pub wins: Vec<OverviewWin>,
    pub needs_you: Vec<OverviewNeed>,
    /// Optimizations with activity in the range (drafts created in it included), newest first.
    pub rows: Vec<OverviewRow>,
}
