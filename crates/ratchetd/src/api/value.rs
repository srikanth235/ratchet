//! The value model and the schedule of one optimization, and the Home header.
//!
//! The **value model** is what a metric improvement is worth: `usdPerUnit` dollars per one unit of improvement in the metric, per
//! `period`, with a plain-words `note` ("ms saved x checkout volume") that says where the number comes from. It is the "Value" row of
//! the optimization's settings, and the footnote of the Overview ("Estimated from each optimization's Value row") links to it.
//! Changing it re-values every win already in the ledger: a win's value is computed from the model, never stored.

use ratchet_core::enums::{Direction, ScheduleKind, ValueBasis, ValuePeriod};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::overview::{OverviewNeed, OverviewRange, ValueStory};

/// `GET /v1/optimizations/{id}/value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValueModelVm {
    /// False until a model is set: the optimization's wins then have no dollar value ("Set a value").
    pub set: bool,
    /// The metric the model prices, as the optimization measures it.
    pub metric: String,
    /// Which way the metric improves.
    pub direction: Direction,
    /// Dollars per one unit of improvement per `period`; `null` while `set` is false.
    #[schema(required = true)]
    pub usd_per_unit: Option<f64>,
    #[schema(required = true)]
    pub period: Option<ValuePeriod>,
    /// What one unit is called ("ms", "k/s").
    #[schema(required = true)]
    pub unit: Option<String>,
    /// `estimated` until it has been measured in production.
    #[schema(required = true)]
    pub basis: Option<ValueBasis>,
    /// The model in plain words, e.g. "ms saved x checkout volume".
    #[schema(required = true)]
    pub note: Option<String>,
    /// One line for the row: "$1,200 per ms a year · estimated · ms saved x checkout volume", or "Not set".
    pub summary: String,
}

/// `PUT /v1/optimizations/{id}/value`: replaces the whole model. Fields left out take their defaults (`period` year, `basis`
/// estimated, no unit, no note); read the current model first to change one field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetValueModelInput {
    /// Dollars per one unit of improvement per `period`; must be greater than 0.
    pub usd_per_unit: f64,
    pub period: Option<ValuePeriod>,
    pub basis: Option<ValueBasis>,
    /// What one unit is called; at most 24 characters.
    pub unit: Option<String>,
    /// The model in plain words; at most 200 characters.
    pub note: Option<String>,
}

/// `GET /v1/optimizations/{id}/schedule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleVm {
    /// `null`: not scheduled, runs when started by hand.
    #[schema(required = true)]
    pub schedule: Option<ScheduleKind>,
    /// "Every night", "Once a week", "Not scheduled".
    pub summary: String,
    /// Runs this schedule starts in the next 30 days; `null` when it cannot be known ("on push").
    #[schema(required = true)]
    pub runs_next_30_days: Option<i64>,
}

/// `PUT /v1/optimizations/{id}/schedule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetScheduleInput {
    /// `null` clears the schedule.
    #[schema(required = true)]
    pub schedule: Option<ScheduleKind>,
}

/// `GET /v1/home`'s query.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HomeQuery {
    /// Window of the numbers; default `90d`. The same ranges the Overview takes.
    pub range: Option<OverviewRange>,
}

/// Pull requests behind the numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HomePullRequests {
    /// Opened and waiting for a merge.
    pub open: i64,
    pub merged: i64,
}

/// `GET /v1/home`: the line above the composer. Its `story` is the Overview's `story`, computed by the same function, so the two
/// screens can never disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HomeVm {
    pub range: OverviewRange,
    /// No optimization yet: show the composer and no numbers.
    pub empty: bool,
    pub story: ValueStory,
    /// Optimizations in the range (drafts left out).
    pub optimizations: i64,
    pub pull_requests: HomePullRequests,
    /// The most recent failed run, if one still needs a look ("1 run failed: CLI cold start").
    #[schema(required = true)]
    pub failed: Option<OverviewNeed>,
}

/// "Every night", "Weekdays at night", "Once a week", "On every push", "Once", "Not scheduled".
pub fn schedule_summary(schedule: Option<ScheduleKind>) -> &'static str {
    match schedule {
        None => "Not scheduled",
        Some(ScheduleKind::Once) => "Once",
        Some(ScheduleKind::Nightly) => "Every night",
        Some(ScheduleKind::Weekdays) => "Weekdays at night",
        Some(ScheduleKind::Weekly) => "Once a week",
        Some(ScheduleKind::OnPush) => "On every push",
    }
}

impl ScheduleVm {
    pub fn of(schedule: Option<ScheduleKind>) -> Self {
        Self {
            schedule,
            summary: schedule_summary(schedule).to_owned(),
            runs_next_30_days: schedule
                .and_then(ScheduleKind::runs_in_30_days)
                .map(i64::from),
        }
    }
}
