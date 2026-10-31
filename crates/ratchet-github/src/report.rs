//! The result report that goes in a pull request body (ported from `packages/github/src/report.ts`).
//!
//! The TS version read the ledger directly. Here the caller (the engine, once `ratchet-ledger` exists) gathers a plain
//! [`ReportInput`] from the ledger and this module only formats it, so the report is a pure function with no SQLite.

use serde::{Deserialize, Serialize};

use crate::domain::{AgentKind, BenchStats, CandidateVerdict, Direction, gain_percent};
use crate::error::{Error, Result};
use crate::numfmt::{num, ser_num, ser_opt_num, to_fixed};
use crate::squash::{SquashPlan, SquashPlanInput, SquashWin, build_squash_plan, subject_of};

// TODO(core): `SpendCategory` and the ledger row shapes below belong to `ratchet-core` / `ratchet-ledger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendCategory {
    AgentRunner,
    Experiment,
    BenchRunner,
    StorageEgress,
    Tokens,
}

impl SpendCategory {
    pub const ALL: [SpendCategory; 5] = [
        SpendCategory::AgentRunner,
        SpendCategory::Experiment,
        SpendCategory::BenchRunner,
        SpendCategory::StorageEgress,
        SpendCategory::Tokens,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SpendCategory::AgentRunner => "agent_runner",
            SpendCategory::Experiment => "experiment",
            SpendCategory::BenchRunner => "bench_runner",
            SpendCategory::StorageEgress => "storage_egress",
            SpendCategory::Tokens => "tokens",
        }
    }
}

// ---- input ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct OptimizationInfo {
    pub id: String,
    pub name: Option<String>,
    pub repo: String,
    pub base_ref: String,
    pub metric: String,
    pub direction: Direction,
    pub status: String,
    /// `bench.run` from the optimization's `ratchet.yaml`, when it parses (the caller owns config parsing).
    pub bench_run: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BaselineInfo {
    pub value: f64,
    pub noise_floor: f64,
    pub stats: BenchStats,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateInfo {
    pub id: String,
    pub agent_id: String,
    pub agent_branch: String,
    pub message: String,
    pub verdict: CandidateVerdict,
    pub bench_value: Option<f64>,
    pub confirm_value: Option<f64>,
    /// Stats of the candidate's last `bench`-phase result.
    pub bench_stats: Option<BenchStats>,
}

/// One move of the ratchet (a verified win), oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct RatchetMove {
    pub candidate_id: String,
    pub agent_id: String,
    pub agent_kind: AgentKind,
    pub sha: String,
    pub from_value: f64,
    pub to_value: f64,
    pub gain_pct: f64,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    pub id: String,
    pub kind: AgentKind,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpendEvent {
    pub agent_id: Option<String>,
    pub category: SpendCategory,
    pub usd: f64,
    pub estimated: bool,
    pub at: i64,
}

/// Everything `build_result_report` needs from the ledger.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportInput {
    pub optimization: OptimizationInfo,
    /// Current best value of the ratchet (`None` falls back to the baseline value, then 0).
    pub best_value: Option<f64>,
    /// Gain of the best over the baseline, in percent.
    pub gain_pct: Option<f64>,
    pub baseline: Option<BaselineInfo>,
    pub candidates: Vec<CandidateInfo>,
    /// Ratchet moves, oldest first.
    pub history: Vec<RatchetMove>,
    pub agents: Vec<AgentInfo>,
    pub spend_events: Vec<SpendEvent>,
}

// ---- output -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportWin {
    pub index: usize,
    pub candidate_id: String,
    pub agent_id: String,
    pub agent_kind: AgentKind,
    pub branch: String,
    pub sha: String,
    pub change: String,
    #[serde(serialize_with = "ser_opt_num")]
    pub bench_value: Option<f64>,
    #[serde(serialize_with = "ser_opt_num")]
    pub confirm_value: Option<f64>,
    #[serde(serialize_with = "ser_num")]
    pub from_value: f64,
    #[serde(serialize_with = "ser_num")]
    pub to_value: f64,
    #[serde(serialize_with = "ser_num")]
    pub gain_pct: f64,
    /// Agent spend since that agent's previous verified win (or its start), up to this win.
    #[serde(serialize_with = "ser_num")]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportAgent {
    pub agent_id: String,
    pub kind: AgentKind,
    pub branch: String,
    pub candidates: usize,
    pub verified: usize,
    #[serde(serialize_with = "ser_num")]
    pub spend_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportBaseline {
    #[serde(serialize_with = "ser_num")]
    pub value: f64,
    #[serde(serialize_with = "ser_num")]
    pub noise_floor: f64,
    pub stats: BenchStats,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportBest {
    #[serde(serialize_with = "ser_num")]
    pub value: f64,
    pub stats: Option<BenchStats>,
    pub sha: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct VerdictCounts {
    pub pending: u32,
    pub verified: u32,
    pub rejected: u32,
    pub failed_confirm: u32,
    pub blocked: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct SpendByCategory {
    #[serde(serialize_with = "ser_num")]
    pub agent_runner: f64,
    #[serde(serialize_with = "ser_num")]
    pub experiment: f64,
    #[serde(serialize_with = "ser_num")]
    pub bench_runner: f64,
    #[serde(serialize_with = "ser_num")]
    pub storage_egress: f64,
    #[serde(serialize_with = "ser_num")]
    pub tokens: f64,
}

impl SpendByCategory {
    fn slot(&mut self, c: SpendCategory) -> &mut f64 {
        match c {
            SpendCategory::AgentRunner => &mut self.agent_runner,
            SpendCategory::Experiment => &mut self.experiment,
            SpendCategory::BenchRunner => &mut self.bench_runner,
            SpendCategory::StorageEgress => &mut self.storage_egress,
            SpendCategory::Tokens => &mut self.tokens,
        }
    }

    pub fn get(&self, c: SpendCategory) -> f64 {
        match c {
            SpendCategory::AgentRunner => self.agent_runner,
            SpendCategory::Experiment => self.experiment,
            SpendCategory::BenchRunner => self.bench_runner,
            SpendCategory::StorageEgress => self.storage_egress,
            SpendCategory::Tokens => self.tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSpend {
    #[serde(serialize_with = "ser_num")]
    pub total: f64,
    #[serde(serialize_with = "ser_num")]
    pub estimated: f64,
    #[serde(serialize_with = "ser_num")]
    pub actual: f64,
    pub by_category: SpendByCategory,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reproduction {
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultReportData {
    pub optimization_id: String,
    pub name: Option<String>,
    pub repo: String,
    pub base_ref: String,
    pub metric: String,
    pub direction: Direction,
    pub status: String,
    pub baseline: Option<ReportBaseline>,
    pub best: ReportBest,
    #[serde(serialize_with = "ser_opt_num")]
    pub gain_pct: Option<f64>,
    pub wins: Vec<ReportWin>,
    pub agents: Vec<ReportAgent>,
    pub verdicts: VerdictCounts,
    pub spend: ReportSpend,
    #[serde(serialize_with = "ser_opt_num")]
    pub cost_per_verified_win_usd: Option<f64>,
    pub reproduction: Reproduction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultReport {
    pub data: ResultReportData,
    pub markdown: String,
    /// `data` pretty-printed.
    pub json: String,
}

const PERCENTILE_ORDER: [&str; 11] = [
    "value", "min", "mean", "median", "p50", "p75", "p90", "p95", "p99", "max", "stddev",
];

pub fn build_result_report(input: &ReportInput) -> Result<ResultReport> {
    let opt = &input.optimization;
    let baseline = input.baseline.as_ref();

    let agent_cost = |agent_id: &str, after: Option<i64>, up_to: i64| -> f64 {
        input
            .spend_events
            .iter()
            .filter(|e| {
                e.agent_id.as_deref() == Some(agent_id)
                    && after.is_none_or(|a| e.at > a)
                    && e.at <= up_to
            })
            .map(|e| e.usd)
            .sum()
    };
    let mut last_win_at: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut wins: Vec<ReportWin> = Vec::with_capacity(input.history.len());
    for (i, h) in input.history.iter().enumerate() {
        let c = input
            .candidates
            .iter()
            .find(|c| c.id == h.candidate_id)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "ratchet move for unknown candidate {}",
                    h.candidate_id
                ))
            })?;
        let cost_usd = agent_cost(
            &h.agent_id,
            last_win_at.get(h.agent_id.as_str()).copied(),
            h.at,
        );
        last_win_at.insert(&h.agent_id, h.at);
        wins.push(ReportWin {
            index: i + 1,
            candidate_id: h.candidate_id.clone(),
            agent_id: h.agent_id.clone(),
            agent_kind: h.agent_kind,
            branch: c.agent_branch.clone(),
            sha: h.sha.clone(),
            change: subject_of(&c.message),
            bench_value: c.bench_value,
            confirm_value: c.confirm_value,
            from_value: h.from_value,
            to_value: h.to_value,
            gain_pct: h.gain_pct,
            cost_usd,
        });
    }

    let mut verdicts = VerdictCounts::default();
    for c in &input.candidates {
        match c.verdict {
            CandidateVerdict::Pending => verdicts.pending += 1,
            CandidateVerdict::Verified => verdicts.verified += 1,
            CandidateVerdict::Rejected => verdicts.rejected += 1,
            CandidateVerdict::FailedConfirm => verdicts.failed_confirm += 1,
            CandidateVerdict::Blocked => verdicts.blocked += 1,
        }
    }

    let best_stats = input
        .history
        .last()
        .and_then(|h| input.candidates.iter().find(|c| c.id == h.candidate_id))
        .and_then(|c| c.bench_stats.clone());

    let mut spend = ReportSpend {
        total: 0.0,
        estimated: 0.0,
        actual: 0.0,
        by_category: SpendByCategory::default(),
    };
    for e in &input.spend_events {
        spend.total += e.usd;
        if e.estimated {
            spend.estimated += e.usd;
        } else {
            spend.actual += e.usd;
        }
        *spend.by_category.slot(e.category) += e.usd;
    }

    let agents: Vec<ReportAgent> = input
        .agents
        .iter()
        .map(|a| ReportAgent {
            agent_id: a.id.clone(),
            kind: a.kind,
            branch: a.branch.clone(),
            candidates: input
                .candidates
                .iter()
                .filter(|c| c.agent_id == a.id)
                .count(),
            verified: wins.iter().filter(|w| w.agent_id == a.id).count(),
            spend_usd: input
                .spend_events
                .iter()
                .filter(|e| e.agent_id.as_deref() == Some(a.id.as_str()))
                .map(|e| e.usd)
                .sum(),
        })
        .collect();

    let best_value = input
        .best_value
        .or(baseline.map(|b| b.value))
        .unwrap_or(0.0);
    let final_sha = wins.last().map(|w| w.sha.clone());
    let branches = agents
        .iter()
        .map(|a| a.branch.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let reproduction = vec![
        format!("git fetch origin {} {}", opt.base_ref, branches)
            .trim()
            .to_string(),
        match &final_sha {
            Some(sha) => format!("git checkout {sha}"),
            None => format!("git checkout {}", opt.base_ref),
        },
        opt.bench_run.clone().unwrap_or_else(|| {
            "# run the benchmark configured in ratchet.yaml (bench.run)".to_string()
        }),
    ];

    let data = ResultReportData {
        optimization_id: opt.id.clone(),
        name: opt.name.clone(),
        repo: opt.repo.clone(),
        base_ref: opt.base_ref.clone(),
        metric: opt.metric.clone(),
        direction: opt.direction,
        status: opt.status.clone(),
        baseline: baseline.map(|b| ReportBaseline {
            value: b.value,
            noise_floor: b.noise_floor,
            stats: b.stats.clone(),
        }),
        best: ReportBest {
            value: best_value,
            stats: best_stats,
            sha: final_sha,
        },
        gain_pct: input.gain_pct,
        cost_per_verified_win_usd: if wins.is_empty() {
            None
        } else {
            Some(spend.total / wins.len() as f64)
        },
        wins,
        agents,
        verdicts,
        spend,
        reproduction: Reproduction {
            commands: reproduction,
        },
    };
    let markdown = render_report_markdown(&data);
    let json = serde_json::to_string_pretty(&data).map_err(Error::invalid)?;
    Ok(ResultReport {
        data,
        markdown,
        json,
    })
}

/// The squash plan for an optimization's verified wins (`buildSquashPlanFromLedger`).
pub fn build_squash_plan_from_report_input(input: &ReportInput) -> Result<SquashPlan> {
    let wins = input
        .history
        .iter()
        .map(|h| SquashWin {
            sha: h.sha.clone(),
            agent_kind: h.agent_kind,
            message: input
                .candidates
                .iter()
                .find(|c| c.id == h.candidate_id)
                .map(|c| c.message.clone())
                .unwrap_or_default(),
            from_value: h.from_value,
            to_value: h.to_value,
        })
        .collect();
    build_squash_plan(&SquashPlanInput {
        metric: input.optimization.metric.clone(),
        direction: input.optimization.direction,
        wins,
    })
}

// ---- markdown ---------------------------------------------------------------

fn usd(n: f64) -> String {
    format!("${}", to_fixed(n, 2))
}

/// Relative change of the metric (not the gain): a 56% latency gain renders as −56.3%, a 10% throughput gain as +10.0%.
fn signed_pct(gain: Option<f64>, direction: Direction) -> String {
    let Some(gain) = gain else {
        return "n/a".into();
    };
    let change = if direction == Direction::Minimize {
        -gain
    } else {
        gain
    };
    format!(
        "{}{}%",
        if change >= 0.0 { "+" } else { "−" },
        to_fixed(change.abs(), 1)
    )
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn table(head: &[&str], rows: &[Vec<String>]) -> String {
    let mut lines = vec![
        format!("| {} |", head.join(" | ")),
        format!(
            "| {} |",
            head.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
        ),
    ];
    for r in rows {
        lines.push(format!(
            "| {} |",
            r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
        ));
    }
    lines.join("\n")
}

/// Pretty "better" sign for a raw stat change: relative change in the metric's units, direction-aware.
fn stat_change(direction: Direction, from: f64, to: f64) -> String {
    signed_pct(gain_percent(direction, from, to), direction)
}

pub fn render_report_markdown(d: &ResultReportData) -> String {
    let mut out: Vec<String> = Vec::new();
    out.push(format!(
        "# Ratchet result: {}",
        d.name.as_deref().unwrap_or(&d.optimization_id)
    ));
    out.push(String::new());
    if let Some(b) = &d.baseline {
        let mut line = format!(
            "**{}: {} → {} ({})** — {} verified {}, {} spent",
            d.metric,
            num(b.value),
            num(d.best.value),
            signed_pct(d.gain_pct, d.direction),
            d.wins.len(),
            if d.wins.len() == 1 { "win" } else { "wins" },
            usd(d.spend.total)
        );
        if let Some(per) = d.cost_per_verified_win_usd {
            line.push_str(&format!(", {} per verified win", usd(per)));
        }
        line.push('.');
        out.push(line);
    } else {
        out.push("_No baseline recorded yet._".into());
    }
    out.push(String::new());
    out.push(format!(
        "Repo `{}` · base `{}` · {} · status {}",
        d.repo,
        d.base_ref,
        d.direction.as_str(),
        d.status
    ));
    out.push(String::new());

    if let Some(b) = &d.baseline {
        out.push("## Percentiles".into());
        out.push(String::new());
        let mut keys: Vec<&String> = Vec::new();
        for k in b
            .stats
            .keys()
            .chain(d.best.stats.iter().flat_map(|s| s.keys()))
        {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let rank = |k: &str| PERCENTILE_ORDER.iter().position(|p| *p == k).unwrap_or(99);
        keys.sort_by(|a, c| rank(a).cmp(&rank(c)).then_with(|| a.cmp(c)));
        let rows: Vec<Vec<String>> = keys
            .iter()
            .map(|k| {
                let from = b.stats.get(k);
                let to =
                    d.best
                        .stats
                        .as_ref()
                        .and_then(|s| s.get(k))
                        .or(if k.as_str() == "value" {
                            Some(d.best.value)
                        } else {
                            None
                        });
                vec![
                    if k.as_str() == "value" {
                        format!("value ({})", d.metric)
                    } else {
                        k.to_string()
                    },
                    from.map_or("–".into(), num),
                    to.map_or("–".into(), num),
                    match (from, to) {
                        (Some(f), Some(t)) => stat_change(d.direction, f, t),
                        _ => "–".into(),
                    },
                ]
            })
            .collect();
        out.push(table(&["Stat", "Baseline", "Best", "Change"], &rows));
        out.push(String::new());
        out.push(format!("Noise floor: {}", num(b.noise_floor)));
        out.push(String::new());
    }

    out.push("## Verified wins".into());
    out.push(String::new());
    if d.wins.is_empty() {
        out.push("_None._".into());
        out.push(String::new());
    } else {
        let rows: Vec<Vec<String>> = d
            .wins
            .iter()
            .map(|w| {
                vec![
                    w.index.to_string(),
                    w.agent_kind.as_str().to_string(),
                    format!("`{}`", &w.sha[..w.sha.len().min(8)]),
                    w.change.clone(),
                    w.bench_value.map_or("–".into(), num),
                    w.confirm_value.map_or("–".into(), num),
                    signed_pct(Some(w.gain_pct), d.direction),
                    usd(w.cost_usd),
                ]
            })
            .collect();
        out.push(table(
            &[
                "#", "Agent", "SHA", "Change", "Bench", "Confirm", "Step", "Cost",
            ],
            &rows,
        ));
        out.push(String::new());
        out.push("_Cost: agent spend since that agent’s previous verified win._".into());
        out.push(String::new());
    }

    let v = &d.verdicts;
    out.push("## Candidates".into());
    out.push(String::new());
    out.push(table(
        &[
            "Verified",
            "Rejected",
            "Failed confirm",
            "Blocked",
            "Pending",
        ],
        &[[
            v.verified,
            v.rejected,
            v.failed_confirm,
            v.blocked,
            v.pending,
        ]
        .iter()
        .map(|n| n.to_string())
        .collect()],
    ));
    out.push(String::new());

    if !d.agents.is_empty() {
        out.push("## Agents".into());
        out.push(String::new());
        let rows: Vec<Vec<String>> = d
            .agents
            .iter()
            .map(|a| {
                vec![
                    a.kind.as_str().to_string(),
                    format!("`{}`", a.branch),
                    a.candidates.to_string(),
                    a.verified.to_string(),
                    usd(a.spend_usd),
                ]
            })
            .collect();
        out.push(table(
            &["Agent", "Branch", "Candidates", "Verified", "Spend"],
            &rows,
        ));
        out.push(String::new());
    }

    let mut spend_rows: Vec<Vec<String>> = SpendCategory::ALL
        .iter()
        .filter(|c| d.spend.by_category.get(**c) != 0.0)
        .map(|c| vec![c.as_str().to_string(), usd(d.spend.by_category.get(*c))])
        .collect();
    spend_rows.push(vec![
        "**total**".into(),
        format!("**{}**", usd(d.spend.total)),
    ]);
    out.push("## Spend".into());
    out.push(String::new());
    out.push(table(&["Category", "USD"], &spend_rows));
    out.push(String::new());
    out.push(format!(
        "{} reconciled, {} estimated. Cost per verified win: {}.",
        usd(d.spend.actual),
        usd(d.spend.estimated),
        d.cost_per_verified_win_usd.map_or("n/a".into(), usd)
    ));
    out.push(String::new());
    out.push("## Reproduce".into());
    out.push(String::new());
    out.push("```sh".into());
    out.extend(d.reproduction.commands.iter().cloned());
    out.push("```".into());
    out.push(String::new());
    out.join("\n")
}
