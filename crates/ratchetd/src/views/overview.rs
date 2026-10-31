//! The Overview: what was spent, what the verified wins are worth, and what is waiting on the user, over a range.
//! Port of `packages/desktop/src/main/overview.ts` (`buildOverview`).
//!
//! Everything comes from the ledger. Value uses the same model as the Result screen (`objective.value`, via
//! [`super::value::value_of_move`]), so the staircase, the hero number and the table's Value column always add up.

use std::collections::{BTreeMap, BTreeSet};

use ratchet_core::Direction;
use ratchet_core::enums::{OptimizationStatus, ValueBasis};
use ratchet_core::numfmt::to_fixed;
use ratchet_core::rows::{Optimization, OptimizationSummary};

use super::format::{DAY_MS, unit_for_metric};
use super::kpis::{KpiInput, Move, SpendAt, moves_of};
use super::ledger_views::LedgerViews;
use crate::api::{
    OverviewDirection, OverviewNeed, OverviewNeedKind, OverviewRange, OverviewReturn, OverviewRow,
    OverviewSpent, OverviewValue, OverviewVm, OverviewWin, StatusTone, UiStatus,
};
use crate::service::ServiceResult;

/// A failed run stops asking for attention after this long.
const FAILED_FOR_DAYS: i64 = 14;

/// Left fold from zero, in list order (what `xs.reduce((a, b) => a + b, 0)` does).
fn sum(xs: impl IntoIterator<Item = f64>) -> f64 {
    xs.into_iter().fold(0.0, |a, b| a + b)
}

/// `\u{2212}39%` for a lower-is-better win, `+5.9%` for a higher-is-better one (same wording as the renderer's
/// `gainLabel`).
fn signed_gain(direction: Direction, gain_pct: Option<f64>) -> Option<String> {
    let g = gain_pct?;
    let sign = if g == 0.0 {
        ""
    } else {
        // minimize: a positive gain reads minus (the metric went down); maximize: a positive gain reads plus
        let positive_is_minus = direction == Direction::Minimize;
        match (g > 0.0, positive_is_minus) {
            (true, true) | (false, false) => "\u{2212}",
            _ => "+",
        }
    };
    let a = g.abs();
    let digits = if a >= 10.0 { 0 } else { 1 };
    Some(format!("{sign}{}%", to_fixed(a, digits)))
}

fn plural(n: i64, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn name_of(s: &OptimizationSummary) -> String {
    s.name
        .clone()
        .unwrap_or_else(|| format!("{} {}", s.repo, s.metric))
}

fn is_live(s: &OptimizationSummary) -> bool {
    matches!(
        s.status,
        OptimizationStatus::Running | OptimizationStatus::Baselining | OptimizationStatus::Stopping
    )
}

fn activity_at(s: &OptimizationSummary) -> i64 {
    s.ended_at.or(s.started_at).unwrap_or(s.created_at)
}

fn status_word(s: &OptimizationSummary, ui: UiStatus, waiting: bool) -> (&'static str, StatusTone) {
    match s.status {
        OptimizationStatus::Running => ("Running", StatusTone::Run),
        OptimizationStatus::Baselining => ("Baselining", StatusTone::Run),
        OptimizationStatus::Stopping => ("Stopping", StatusTone::Run),
        OptimizationStatus::Draft => ("Draft", StatusTone::Off),
        OptimizationStatus::Failed => ("Failed", StatusTone::Bad),
        OptimizationStatus::Completed | OptimizationStatus::Stopped => {
            if waiting {
                ("Waiting on you", StatusTone::Wait)
            } else if ui == UiStatus::PrOpen {
                ("PR open", StatusTone::Ok)
            } else if s.verified_wins == 0 {
                ("No win", StatusTone::Off)
            } else if s.status == OptimizationStatus::Stopped {
                ("Stopped", StatusTone::Ok)
            } else {
                ("Done", StatusTone::Ok)
            }
        }
    }
}

impl LedgerViews {
    /// The ledger status as the pills use it: `pr-open` once a completed run has a pull request.
    fn ui_status(&self, s: &OptimizationSummary) -> UiStatus {
        if s.status == OptimizationStatus::Completed && (self.pr_url)(s.id.as_str()).is_some() {
            UiStatus::PrOpen
        } else {
            s.status.into()
        }
    }

    /// What waits on the user, whatever the range: verified wins nobody has opened a pull request for, and recent
    /// failed runs. (Environment approvals and expired connectors do not exist in the data model yet.)
    fn needs_you(&self, summaries: &[&OptimizationSummary], now: i64) -> Vec<OverviewNeed> {
        let mut ship = Vec::new();
        let mut failed = Vec::new();
        let mut sorted: Vec<&OptimizationSummary> = summaries.to_vec();
        sorted.sort_by_key(|s| std::cmp::Reverse(s.ended_at.unwrap_or(s.created_at)));
        for s in sorted {
            let name = name_of(s);
            let id = s.id.as_str();
            if matches!(
                s.status,
                OptimizationStatus::Completed | OptimizationStatus::Stopped
            ) && s.verified_wins > 0
                && self.ui_status(s) != UiStatus::PrOpen
            {
                let sub = match signed_gain(s.direction, s.gain_pct) {
                    Some(g) => format!("{name} \u{b7} {g}"),
                    None => name,
                };
                ship.push(OverviewNeed {
                    id: format!("ship:{id}"),
                    kind: OverviewNeedKind::Ship,
                    optimization_id: id.to_owned(),
                    title: format!(
                        "Open a pull request for {}",
                        plural(s.verified_wins, "verified win")
                    ),
                    sub,
                    action: "Review".into(),
                });
            } else if s.status == OptimizationStatus::Failed
                && now - activity_at(s) < FAILED_FOR_DAYS * DAY_MS
            {
                failed.push(OverviewNeed {
                    id: format!("failed:{id}"),
                    kind: OverviewNeedKind::Failed,
                    optimization_id: id.to_owned(),
                    title: "A run failed".into(),
                    sub: name,
                    action: "View".into(),
                });
            }
        }
        ship.extend(failed);
        ship
    }

    /// The Overview over `range`, at the clock's now.
    pub async fn overview(&self, range: OverviewRange) -> ServiceResult<OverviewVm> {
        let now = self.now();
        let rows = self.rows().await?;
        let mut histories = Vec::with_capacity(rows.len());
        for (s, _) in &rows {
            histories.push(self.ledger.ratchet_history(&s.id).await?);
        }
        let inputs: Vec<KpiInput<'_>> = rows
            .iter()
            .zip(&histories)
            .map(|((s, r), h)| KpiInput {
                summary: s,
                row: r,
                history: h,
            })
            .collect();
        let moves: Vec<Move> = moves_of(&inputs);
        let ids: BTreeSet<&str> = rows.iter().map(|(s, _)| s.id.as_str()).collect();
        let spend: Vec<SpendAt> = self
            .all_spend()
            .await?
            .into_iter()
            .filter(|e| ids.contains(e.opt.as_str()))
            .collect();
        let row_of: BTreeMap<&str, &Optimization> =
            rows.iter().map(|(s, r)| (s.id.as_str(), r)).collect();
        let summaries: Vec<&OptimizationSummary> = rows.iter().map(|(s, _)| s).collect();

        let first_activity = summaries
            .iter()
            .filter(|s| s.status != OptimizationStatus::Draft)
            .map(|s| s.started_at.unwrap_or(s.created_at))
            .chain(moves.iter().map(|m| m.at))
            .chain(spend.iter().map(|e| e.at))
            .fold(now, i64::min);
        let from = match range {
            OverviewRange::All => {
                if summaries.is_empty() {
                    now - 30 * DAY_MS
                } else {
                    first_activity
                }
            }
            OverviewRange::Days30 => now - 30 * DAY_MS,
            OverviewRange::Days90 => now - 90 * DAY_MS,
        };
        let inside = |at: i64| at >= from && at <= now;

        let has_value_model = rows.iter().any(|(_, r)| r.value_usd_per_unit.is_some());
        let needs = self.needs_you(&summaries, now);
        let waiting: BTreeSet<&str> = needs
            .iter()
            .filter(|n| n.kind == OverviewNeedKind::Ship)
            .map(|n| n.optimization_id.as_str())
            .collect();

        let in_range = |s: &OptimizationSummary| -> bool {
            if range == OverviewRange::All {
                return true;
            }
            if s.status == OptimizationStatus::Draft {
                return inside(s.created_at);
            }
            if is_live(s) {
                return true;
            }
            let id = s.id.as_str();
            inside(s.started_at.unwrap_or(s.created_at))
                || s.ended_at.is_some_and(inside)
                || spend.iter().any(|e| e.opt == id && inside(e.at))
                || moves.iter().any(|m| m.opt == id && inside(m.at))
        };

        let mut out_rows: Vec<OverviewRow> = summaries
            .iter()
            .filter(|s| in_range(s))
            .map(|s| {
                let id = s.id.as_str();
                let row = row_of[id];
                let ui = self.ui_status(s);
                let live = is_live(s);
                let (label, tone) = status_word(s, ui, waiting.contains(id));
                let valued = row.value_usd_per_unit.is_some();
                let remaining = match (live, s.started_at) {
                    (true, Some(started)) => {
                        Some((row.time_limit_s as f64 - (now - started) as f64 / 1000.0).max(0.0))
                    }
                    _ => None,
                };
                OverviewRow {
                    id: id.to_owned(),
                    name: name_of(s),
                    repo: s.repo.clone(),
                    status: ui,
                    status_label: label.to_owned(),
                    tone,
                    remaining_s: remaining.map(|r| r.round() as i64),
                    metric: s.metric.clone(),
                    unit: unit_for_metric(&s.metric).to_owned(),
                    direction: if s.direction == Direction::Minimize {
                        OverviewDirection::Lower
                    } else {
                        OverviewDirection::Higher
                    },
                    baseline: s.baseline_value,
                    best: s.best_value,
                    gain_pct: s.gain_pct,
                    verified_wins: s.verified_wins,
                    spent_usd: sum(spend
                        .iter()
                        .filter(|e| e.opt == id && inside(e.at))
                        .map(|e| e.usd)),
                    budget_usd: s.budget_usd,
                    live,
                    value_usd_per_year: valued.then(|| {
                        sum(moves
                            .iter()
                            .filter(|m| m.opt == id && inside(m.at))
                            .filter_map(|m| m.usd_per_year))
                    }),
                }
            })
            .collect();
        let activity: BTreeMap<&str, i64> = summaries
            .iter()
            .map(|s| (s.id.as_str(), activity_at(s)))
            .collect();
        // live first, then newest activity (stable, like the TS sort)
        out_rows.sort_by(|a, b| {
            b.live
                .cmp(&a.live)
                .then_with(|| activity[b.id.as_str()].cmp(&activity[a.id.as_str()]))
        });

        let spent_usd = sum(spend.iter().filter(|e| inside(e.at)).map(|e| e.usd));
        let budget_usd = sum(out_rows
            .iter()
            .filter(|r| r.status != UiStatus::Draft)
            .map(|r| r.budget_usd));

        let win_moves: Vec<(&Move, f64)> = moves
            .iter()
            .filter(|m| inside(m.at))
            .filter_map(|m| m.usd_per_year.map(|u| (m, u)))
            .collect();
        let names: BTreeMap<&str, String> = summaries
            .iter()
            .map(|s| (s.id.as_str(), name_of(s)))
            .collect();
        let mut cumulative = 0.0;
        let wins: Vec<OverviewWin> = win_moves
            .iter()
            .map(|(m, usd)| {
                cumulative += usd;
                OverviewWin {
                    at: m.at,
                    cumulative_usd_per_year: cumulative,
                    usd_per_year: *usd,
                    optimization_id: m.opt.clone(),
                    optimization: names[m.opt.as_str()].clone(),
                }
            })
            .collect();
        let value_usd = cumulative;
        let all_measured = !win_moves.is_empty()
            && win_moves.iter().all(|(m, _)| {
                row_of[m.opt.as_str()]
                    .value_basis
                    .unwrap_or(ValueBasis::Estimated)
                    != ValueBasis::Estimated
            });

        Ok(OverviewVm {
            range,
            from,
            to: now,
            empty: summaries.is_empty(),
            spent: OverviewSpent {
                usd: spent_usd,
                budget_usd,
                optimizations: out_rows
                    .iter()
                    .filter(|r| r.status != UiStatus::Draft)
                    .count() as i64,
            },
            value: has_value_model.then(|| OverviewValue {
                usd_per_year: value_usd,
                wins: wins.len() as i64,
                basis: if all_measured {
                    ValueBasis::Measured
                } else {
                    ValueBasis::Estimated
                },
            }),
            return_on_spend: (has_value_model && value_usd > 0.0 && spent_usd > 0.0).then(|| {
                OverviewReturn {
                    ratio: value_usd / spent_usd,
                    payback_days: (spent_usd / value_usd) * 365.0,
                }
            }),
            wins,
            needs_you: needs,
            rows: out_rows,
        })
    }
}
