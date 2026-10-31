//! The Overview screen (PROVISIONAL API): hero numbers, value staircase, "needs you", the table.

use ratchet_core::enums::{OptimizationStatus, ValueBasis};

use super::Inner;
use crate::api::*;
use crate::service::ServiceResult;
use crate::views::format::{DAY_MS, MIN_MS, fmt_num};
use crate::views::kpis::{KpiInput, Move, moves_of, usd};

fn left(ms: i64) -> String {
    let m = (ms.max(0) / MIN_MS).max(1);
    if m >= 60 {
        format!("{} h {} m left", m / 60, m % 60)
    } else {
        format!("{m} m left")
    }
}

impl Inner {
    pub(crate) async fn overview(&self, range: OverviewRange) -> ServiceResult<OverviewVm> {
        let now = self.clock.now();
        let from = match range {
            OverviewRange::Days30 => now - 30 * DAY_MS,
            OverviewRange::Days90 => now - 90 * DAY_MS,
            OverviewRange::All => 0,
        };
        let rows = self.views.rows().await?;
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
        let spend = self.views.all_spend().await?;
        let spent: f64 = spend.iter().filter(|e| e.at >= from).map(|e| e.usd).sum();

        // the staircase: cumulative yearly value, one step per valued win in the window
        let names: std::collections::BTreeMap<&str, String> = rows
            .iter()
            .map(|(s, _)| {
                (
                    s.id.as_str(),
                    s.name
                        .clone()
                        .unwrap_or_else(|| format!("{} {}", s.repo, s.metric)),
                )
            })
            .collect();
        let mut cum = 0.0;
        let mut points = Vec::new();
        for m in moves.iter().filter(|m| m.at >= from) {
            if let Some(v) = m.usd_per_year {
                cum += v;
                points.push(StaircasePoint {
                    at: m.at,
                    usd_per_year: cum,
                    optimization_id: m.opt.clone(),
                    optimization_name: names.get(m.opt.as_str()).cloned().unwrap_or_default(),
                });
            }
        }
        let wins = points.len() as i64;
        let estimated = rows.iter().any(|(_, r)| {
            r.value_usd_per_unit.is_some() && r.value_basis != Some(ValueBasis::Measured)
        });
        let ratio = (spent > 0.0 && cum > 0.0).then(|| cum / spent);
        let payback_days = (spent > 0.0 && cum > 0.0).then(|| spent / (cum / 365.0));

        // spend vs the monthly caps of the default AWS connector
        let cap = {
            let st = self.st();
            st.connectors
                .items
                .iter()
                .find(|c| c.kind == ConnectorKind::Aws && c.is_default)
                .and_then(|c| c.aws.as_ref().and_then(|a| a.cap_usd_per_month))
        };

        // needs you
        let mut needs = Vec::new();
        for d in self.drafts.waiting() {
            let Some(p) = d.draft.as_ref() else { continue };
            needs.push(NeedsYouItem {
                id: format!("approve-{}", d.id),
                kind: NeedsYouKind::ApproveEnvironment,
                title: "Approve environment".into(),
                subtitle: format!(
                    "{} \u{00b7} {} stack, ${}/h",
                    d.goal,
                    match p.scope {
                        EnvironmentScope::Full => "full",
                        EnvironmentScope::Single => "single-machine",
                        EnvironmentScope::None => "no extra",
                    },
                    ratchet_core::numfmt::fixed_trimmed(p.cost.usd_per_hour, 2)
                ),
                action_label: "Review".into(),
                optimization_id: None,
                environment_draft_id: Some(d.id),
                connector_id: None,
                url: None,
            });
        }
        for ((s, _), _) in rows.iter().zip(&histories) {
            if s.status == OptimizationStatus::Completed && s.verified_wins > 0 {
                let url = self
                    .pr_urls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(s.id.as_str())
                    .cloned();
                needs.push(NeedsYouItem {
                    id: format!("merge-{}", s.id),
                    kind: NeedsYouKind::MergeWins,
                    title: if url.is_some() {
                        format!(
                            "Merge {} verified win{}",
                            s.verified_wins,
                            if s.verified_wins == 1 { "" } else { "s" }
                        )
                    } else {
                        format!(
                            "Open a pull request for {} verified win{}",
                            s.verified_wins,
                            if s.verified_wins == 1 { "" } else { "s" }
                        )
                    },
                    subtitle: format!(
                        "{}{}{}",
                        s.name.clone().unwrap_or_else(|| s.repo.clone()),
                        // the metric's own change: down for "lower is better", up for "higher"
                        s.gain_pct.map_or(String::new(), |g| {
                            let change = if s.direction == ratchet_core::Direction::Minimize {
                                -g
                            } else {
                                g
                            };
                            format!(" \u{00b7} {change:+.0}%")
                        }),
                        url.as_ref().map_or(String::new(), |u| format!(
                            " \u{00b7} {}",
                            u.rsplit('/')
                                .next()
                                .map_or(String::new(), |n| format!("PR #{n}"))
                        )),
                    ),
                    action_label: "Open PR".into(),
                    optimization_id: Some(s.id.to_string()),
                    environment_draft_id: None,
                    connector_id: None,
                    url,
                });
            }
        }
        {
            let st = self.st();
            for c in st
                .connectors
                .items
                .iter()
                .filter(|c| c.status == ConnectorStatus::Expired)
            {
                needs.push(NeedsYouItem {
                    id: format!("signin-{}", c.id),
                    kind: NeedsYouKind::SignIn,
                    title: format!("Sign in to {}", c.name),
                    subtitle: "The session expired".into(),
                    action_label: "Sign in".into(),
                    optimization_id: None,
                    environment_draft_id: None,
                    connector_id: Some(c.id.clone()),
                    url: None,
                });
            }
        }

        // the table
        let mut table = Vec::new();
        for (s, row) in &rows {
            let ui = self.views.optimization_vm(s, row);
            let (state, text, note) = match ui.status {
                UiStatus::Running | UiStatus::Baselining | UiStatus::Stopping => {
                    let elapsed = now - s.started_at.unwrap_or(s.created_at);
                    (
                        OverviewState::Running,
                        "Running",
                        Some(left(row.time_limit_s * 1000 - elapsed)),
                    )
                }
                UiStatus::PrOpen => (
                    OverviewState::WaitingOnYou,
                    "Waiting on you",
                    Some("pull request open".into()),
                ),
                UiStatus::Completed if s.verified_wins == 0 => {
                    (OverviewState::NoWin, "No win", None)
                }
                UiStatus::Completed => (OverviewState::Done, "Done", None),
                UiStatus::Stopped => (OverviewState::Stopped, "Stopped", None),
                UiStatus::Failed => (OverviewState::Failed, "Failed", None),
                UiStatus::Draft => (OverviewState::Draft, "Draft", None),
            };
            let unit = if ui.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", ui.unit)
            };
            let result_text = match (ui.baseline, ui.best, s.verified_wins) {
                (Some(b), Some(best), n) if n > 0 => {
                    format!("{} \u{2192} {}{unit}", fmt_num(b), fmt_num(best))
                }
                (Some(b), _, _) if b != 0.0 => {
                    let noise = self
                        .ledger
                        .get_baseline(&s.id)
                        .await?
                        .map_or(0.0, |x| x.noise_floor);
                    format!(
                        "nothing beat the noise (\u{00b1}{:.0}%)",
                        (noise / b.abs()) * 100.0
                    )
                }
                _ => String::new(),
            };
            table.push(OverviewRow {
                optimization_id: s.id.to_string(),
                name: ui.name.clone(),
                repo: s.repo.clone(),
                state,
                state_text: text.into(),
                state_note: note,
                result_pct: ui.gain_pct.filter(|_| s.verified_wins > 0),
                result_text,
                spent_usd: s.spend_usd,
                budget_usd: (state == OverviewState::Running).then_some(s.budget_usd),
                value_usd_per_year: ui.value.as_ref().map(|v| v.usd_per_year),
            });
        }

        // the sandbox chip
        let envs = self.environments().await?;
        let active = envs
            .environments
            .iter()
            .find(|e| !e.active_runs.is_empty())
            .or_else(|| envs.environments.first());
        let sandbox = match active {
            Some(e) => SandboxChip {
                on: true,
                name: Some(e.sandbox_id.clone()),
                spent_usd: e.spend_usd,
                off_at: e.destroy_at,
            },
            None => SandboxChip {
                on: false,
                name: None,
                spent_usd: 0.0,
                off_at: None,
            },
        };

        Ok(OverviewVm {
            range,
            spent: SpentHero {
                usd: spent,
                cap_usd: cap,
                fraction_of_cap: cap.map(|c| (spent / c).clamp(0.0, 1.0)),
                note: cap.map_or("across AWS and agent keys".into(), |c| {
                    format!("of {} across AWS and agent keys", usd(c))
                }),
            },
            value_locked_in: ValueHero {
                usd_per_year: cum,
                wins,
                basis: if estimated {
                    ValueBasis::Estimated
                } else {
                    ValueBasis::Measured
                },
                note: format!(
                    "from {wins} verified win{} ({})",
                    if wins == 1 { "" } else { "s" },
                    if estimated { "estimated" } else { "measured" }
                ),
            },
            return_on_spend: ReturnHero {
                ratio,
                payback_days,
                note: match (ratio, payback_days) {
                    (Some(_), Some(d)) => format!(
                        "yearly value for every $1 spent \u{00b7} paid back in {} day{}",
                        d.ceil().max(1.0),
                        if d.ceil() <= 1.0 { "" } else { "s" }
                    ),
                    _ => "yearly value for every $1 spent".into(),
                },
            },
            staircase: Staircase {
                from: if from == 0 {
                    points.first().map_or(now, |p| p.at)
                } else {
                    from
                },
                to: now,
                points,
            },
            needs_you: needs,
            rows: table,
            sandbox,
        })
    }
}
