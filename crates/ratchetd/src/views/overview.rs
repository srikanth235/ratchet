//! The Overview: what was spent, what the verified wins are worth, and what is waiting on the user, over a range.
//! Port of `packages/desktop/src/main/overview.ts` (`buildOverview`).
//!
//! Everything comes from the ledger. Value uses the same model as the Result screen (`objective.value`, via
//! [`super::value::value_of_move`]), so the staircase, the hero number and the table's Value column always add up.

use std::collections::{BTreeMap, BTreeSet};

use ratchet_core::Direction;
use ratchet_core::enums::{Billing, OptimizationStatus, SpendCategory, ValueBasis};
use ratchet_core::numfmt::to_fixed;
use ratchet_core::rows::{Optimization, OptimizationSummary};

use super::format::{DAY_MS, tokens_compact, unit_for_metric, usd_compact};
use super::kpis::{KpiInput, Move, SpendAt, moves_of};
use super::ledger_views::LedgerViews;
use super::value::value_model_vm;
use crate::api::{
    HomePullRequests, HomeVm, OverviewChartPoint, OverviewDirection, OverviewEmptyState,
    OverviewForecast, OverviewNeed, OverviewNeedKind, OverviewPhase, OverviewPullRequest,
    OverviewRange, OverviewReturn, OverviewRow, OverviewSpent, OverviewValue, OverviewValueState,
    OverviewVm, OverviewWin, PendingAmount, StatusTone, UiStatus, ValueAmount, ValueStory,
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

/// Where an optimization's pull request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pr {
    None,
    Open,
    Merged,
}

fn status_word(
    s: &OptimizationSummary,
    ui: UiStatus,
    waiting: bool,
    pr: Pr,
) -> (&'static str, StatusTone) {
    if pr == Pr::Merged {
        return ("Merged", StatusTone::Ok);
    }
    match s.status {
        OptimizationStatus::Running => ("Running", StatusTone::Run),
        OptimizationStatus::Baselining => ("Baselining", StatusTone::Run),
        OptimizationStatus::Stopping => ("Stopping", StatusTone::Run),
        OptimizationStatus::Draft => ("Draft", StatusTone::Off),
        OptimizationStatus::Failed => ("Failed", StatusTone::Bad),
        OptimizationStatus::Completed | OptimizationStatus::Stopped => {
            if waiting {
                ("Ready to ship", StatusTone::Wait)
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
    fn pr_of(&self, id: &str) -> Pr {
        if (self.pr_merged)(id) {
            Pr::Merged
        } else if (self.pr_url)(id).is_some() {
            Pr::Open
        } else {
            Pr::None
        }
    }

    fn ui_status(&self, s: &OptimizationSummary) -> UiStatus {
        if s.status == OptimizationStatus::Completed && (self.pr_url)(s.id.as_str()).is_some() {
            UiStatus::PrOpen
        } else {
            s.status.into()
        }
    }

    /// What waits on the user, whatever the range: verified wins nobody has opened a pull request for, and recent
    /// failed runs. Only things a person has to do appear here: an optimization never waits for approval (its environment checks run
    /// on their own), and a signed-out connector is the one other case.
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
                && self.pr_of(id) == Pr::None
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

        // Where each optimization's value stands: only a merged pull request delivers it.
        let state_of = |id: &str| -> OverviewValueState {
            if !moves.iter().any(|m| m.opt == id && inside(m.at)) {
                return OverviewValueState::None;
            }
            match self.pr_of(id) {
                Pr::Merged => OverviewValueState::Delivered,
                Pr::Open => OverviewValueState::Pending,
                Pr::None => OverviewValueState::Ready,
            }
        };

        let mut out_rows: Vec<OverviewRow> = summaries
            .iter()
            .filter(|s| in_range(s))
            .map(|s| {
                let id = s.id.as_str();
                let row = row_of[id];
                let ui = self.ui_status(s);
                let live = is_live(s);
                let pr = self.pr_of(id);
                let (label, tone) = status_word(s, ui, waiting.contains(id), pr);
                let valued = row.value_usd_per_unit.is_some();
                let remaining = match (live, s.started_at) {
                    (true, Some(started)) => {
                        Some((row.time_limit_s as f64 - (now - started) as f64 / 1000.0).max(0.0))
                    }
                    _ => None,
                };
                let spent = sum(spend
                    .iter()
                    .filter(|e| e.opt == id && inside(e.at))
                    .map(|e| e.usd));
                let value = valued.then(|| {
                    sum(moves
                        .iter()
                        .filter(|m| m.opt == id && inside(m.at))
                        .filter_map(|m| m.usd_per_year))
                });
                let value_state = if valued {
                    state_of(id)
                } else {
                    OverviewValueState::None
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
                    spent_usd: spent,
                    budget_usd: s.budget_usd,
                    live,
                    value_usd_per_year: value,
                    pull_request: (pr != Pr::None).then(|| OverviewPullRequest {
                        url: (self.pr_url)(id).unwrap_or_default(),
                        merged: pr == Pr::Merged,
                    }),
                    value_state,
                    pending: matches!(
                        value_state,
                        OverviewValueState::Pending | OverviewValueState::Ready
                    ),
                    return_x: value.filter(|v| *v > 0.0 && spent > 0.0).map(|v| v / spent),
                    value_model: valued.then(|| value_model_vm(row)),
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

        // ---- spend: real dollars only; tokens on the user's own plan are counted, not charged
        let in_window: Vec<&SpendAt> = spend.iter().filter(|e| inside(e.at)).collect();
        let aws_usd = sum(in_window
            .iter()
            .filter(|e| e.category != SpendCategory::Tokens)
            .map(|e| e.usd));
        let agents_api_usd = sum(in_window
            .iter()
            .filter(|e| e.category == SpendCategory::Tokens && e.billing == Billing::Api)
            .map(|e| e.usd));
        let included_tokens: i64 = in_window
            .iter()
            .filter(|e| e.billing == Billing::Subscription)
            .map(|e| e.tokens)
            .sum();
        let spent_usd = aws_usd + agents_api_usd;
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
                    state: state_of(&m.opt),
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
        let basis = if all_measured {
            ValueBasis::Measured
        } else {
            ValueBasis::Estimated
        };

        // ---- the value story: the one both Home and Overview tell
        let amount = |state: OverviewValueState| -> ValueAmount {
            let of: Vec<&OverviewWin> = wins.iter().filter(|w| w.state == state).collect();
            ValueAmount {
                usd_per_year: sum(of.iter().map(|w| w.usd_per_year)),
                wins: of.len() as i64,
            }
        };
        let delivered = amount(OverviewValueState::Delivered);
        let ready_to_ship = amount(OverviewValueState::Ready);
        let open: Vec<&OverviewWin> = wins
            .iter()
            .filter(|w| w.state == OverviewValueState::Pending)
            .collect();
        let pending = PendingAmount {
            usd_per_year: sum(open.iter().map(|w| w.usd_per_year)),
            wins: open.len() as i64,
            pull_requests: open
                .iter()
                .map(|w| w.optimization_id.as_str())
                .collect::<BTreeSet<_>>()
                .len() as i64,
        };
        let story_return = (delivered.usd_per_year > 0.0 && spent_usd > 0.0)
            .then(|| delivered.usd_per_year / spent_usd);
        let payback_days = (delivered.usd_per_year > 0.0 && spent_usd > 0.0)
            .then(|| (spent_usd / delivered.usd_per_year) * 365.0);
        let story = ValueStory {
            basis,
            line: story_line(
                has_value_model,
                &delivered,
                &pending,
                &ready_to_ship,
                story_return,
            ),
            delivered,
            pending,
            ready_to_ship,
            spent_usd,
            return_x: story_return,
            payback_days,
        };

        // ---- next 30 days
        let forecast = forecast_of(&rows, &spend, budget_usd);

        // ---- chart
        let chart = chart_points(from, now, &spend, &wins);

        let phase = if summaries.is_empty() {
            OverviewPhase::Empty
        } else if moves.is_empty() {
            OverviewPhase::NoWins
        } else {
            OverviewPhase::Wins
        };
        let empty_state = match phase {
            OverviewPhase::Empty => Some(OverviewEmptyState {
                title: "No wins yet".into(),
                body: "Start your first optimization.".into(),
            }),
            OverviewPhase::NoWins => Some(OverviewEmptyState {
                title: "No wins yet".into(),
                body: "A win shows up here as soon as an optimization proves one.".into(),
            }),
            OverviewPhase::Wins => None,
        };

        Ok(OverviewVm {
            range,
            from,
            to: now,
            empty: summaries.is_empty(),
            phase,
            empty_state,
            story,
            forecast,
            chart,
            spent: OverviewSpent {
                usd: spent_usd,
                budget_usd,
                optimizations: out_rows
                    .iter()
                    .filter(|r| r.status != UiStatus::Draft)
                    .count() as i64,
                aws_usd,
                agents_api_usd,
                included_tokens,
                detail: spend_detail(aws_usd, agents_api_usd, included_tokens),
            },
            value: has_value_model.then_some(OverviewValue {
                usd_per_year: value_usd,
                wins: wins.len() as i64,
                basis,
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

/// What the schedules will start in the next 30 days and what that costs: each scheduled optimization's next runs cost what
/// its runs have cost so far (capped at its budget), split the way the Overview splits spend.
fn forecast_of(
    rows: &[(OptimizationSummary, Optimization)],
    spend: &[SpendAt],
    hard_cap_usd: f64,
) -> OverviewForecast {
    let (mut runs, mut unforecast) = (0i64, 0i64);
    let (mut aws, mut api, mut tokens) = (0.0, 0.0, 0.0_f64);
    for (s, row) in rows
        .iter()
        .filter(|(s, _)| s.status != OptimizationStatus::Draft)
    {
        let Some(kind) = row.schedule else { continue };
        let Some(n) = kind.runs_in_30_days() else {
            unforecast += 1;
            continue;
        };
        if n == 0 {
            continue;
        }
        let mine: Vec<&SpendAt> = spend.iter().filter(|e| e.opt == s.id.as_str()).collect();
        let run_aws = sum(mine
            .iter()
            .filter(|e| e.category != SpendCategory::Tokens)
            .map(|e| e.usd));
        let run_api = sum(mine
            .iter()
            .filter(|e| e.category == SpendCategory::Tokens && e.billing == Billing::Api)
            .map(|e| e.usd));
        let run_tokens: i64 = mine
            .iter()
            .filter(|e| e.billing == Billing::Subscription)
            .map(|e| e.tokens)
            .sum();
        // a run never spends more than its budget
        let scale = if run_aws + run_api > s.budget_usd && run_aws + run_api > 0.0 {
            s.budget_usd / (run_aws + run_api)
        } else {
            1.0
        };
        runs += i64::from(n);
        aws += run_aws * scale * f64::from(n);
        api += run_api * scale * f64::from(n);
        tokens += run_tokens as f64 * f64::from(n);
    }
    let expected = aws + api;
    let cap = (hard_cap_usd > 0.0).then_some(hard_cap_usd);
    let mut line = format!("{} \u{b7} ~{}", plural(runs, "run"), usd_compact(expected));
    if tokens > 0.0 {
        line.push_str(&format!(
            " + {} tokens in your plan",
            tokens_compact(tokens.round() as i64)
        ));
    }
    if let Some(c) = cap {
        line.push_str(&format!(" \u{b7} cap {}", usd_compact(c)));
    }
    OverviewForecast {
        days: 30,
        scheduled_runs: runs,
        unforecast,
        expected_usd: expected,
        expected_aws_usd: aws,
        expected_agents_api_usd: api,
        expected_included_tokens: tokens.round() as i64,
        hard_cap_usd: cap,
        line,
    }
}

/// The line above the composer, from the Overview that was just computed: the same `story`, so the two screens cannot disagree.
pub fn home_of(o: OverviewVm) -> HomeVm {
    let (mut open, mut merged) = (0, 0);
    for r in &o.rows {
        match &r.pull_request {
            Some(p) if p.merged => merged += 1,
            Some(_) => open += 1,
            None => {}
        }
    }
    HomeVm {
        range: o.range,
        empty: o.empty,
        story: o.story,
        optimizations: o.spent.optimizations,
        pull_requests: HomePullRequests { open, merged },
        failed: o
            .needs_you
            .into_iter()
            .find(|n| n.kind == OverviewNeedKind::Failed),
    }
}

/// "68×" (whole from 10 up, one decimal below).
fn ratio_label(x: f64) -> String {
    if x >= 10.0 {
        format!("{}\u{d7}", to_fixed(x, 0))
    } else {
        format!("{}\u{d7}", to_fixed(x, 1))
    }
}

/// "$12.5k delivered + $2.9k pending · 68×": the sentence Home and Overview both show.
fn story_line(
    has_value_model: bool,
    delivered: &ValueAmount,
    pending: &PendingAmount,
    ready: &ValueAmount,
    ret: Option<f64>,
) -> String {
    if !has_value_model {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    if delivered.usd_per_year > 0.0 {
        parts.push(format!("{} delivered", usd_compact(delivered.usd_per_year)));
    }
    if pending.usd_per_year > 0.0 {
        parts.push(format!("{} pending", usd_compact(pending.usd_per_year)));
    }
    if ready.usd_per_year > 0.0 {
        parts.push(format!("{} ready to ship", usd_compact(ready.usd_per_year)));
    }
    if parts.is_empty() {
        return "Nothing delivered yet".into();
    }
    let mut line = parts.join(" + ");
    if let Some(r) = ret {
        line.push_str(&format!(" \u{b7} {}", ratio_label(r)));
    }
    line
}

/// "AWS $121 · agents $63", or with a plan: "AWS $121 · agents included in your plan (2.4M tokens)".
fn spend_detail(aws: f64, api: f64, included_tokens: i64) -> String {
    let agents = if included_tokens > 0 && api <= 0.0 {
        format!(
            "agents included in your plan ({} tokens)",
            tokens_compact(included_tokens)
        )
    } else if included_tokens > 0 {
        format!(
            "agents {} + {} tokens in your plan",
            usd_compact(api),
            tokens_compact(included_tokens)
        )
    } else {
        format!("agents {}", usd_compact(api))
    };
    format!("AWS {} \u{b7} {agents}", usd_compact(aws))
}

/// The most points a chart gets from spend alone; wins add their own.
const CHART_BUCKETS: i64 = 120;

/// The spend line and the value staircases, oldest first: a point at the start, one per spend bucket, one per win, one at the end.
fn chart_points(
    from: i64,
    to: i64,
    spend: &[SpendAt],
    wins: &[OverviewWin],
) -> Vec<OverviewChartPoint> {
    let span = (to - from).max(1);
    let bucket = ((span + CHART_BUCKETS - 1) / CHART_BUCKETS).max(1);
    let mut times: BTreeSet<i64> = BTreeSet::new();
    times.insert(from);
    times.insert(to);
    for e in spend.iter().filter(|e| e.at >= from && e.at <= to) {
        times.insert((from + ((e.at - from) / bucket + 1) * bucket).min(to));
    }
    for w in wins {
        times.insert(w.at);
    }
    times
        .into_iter()
        .map(|at| {
            let spent = sum(spend
                .iter()
                .filter(|e| e.at >= from && e.at <= at)
                .map(|e| e.usd));
            let by = |state: OverviewValueState| {
                sum(wins
                    .iter()
                    .filter(|w| w.at <= at && w.state == state)
                    .map(|w| w.usd_per_year))
            };
            OverviewChartPoint {
                at,
                spent_usd: spent,
                delivered_usd_per_year: by(OverviewValueState::Delivered),
                pending_usd_per_year: by(OverviewValueState::Pending),
                ready_usd_per_year: by(OverviewValueState::Ready),
            }
        })
        .collect()
}
