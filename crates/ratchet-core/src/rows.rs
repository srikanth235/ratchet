//! The records of the ledger and the views built from them.
//!
//! Field names are camelCase on the wire, as the Drizzle rows were. Timestamps are integer epoch milliseconds;
//! money is USD as `f64`.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::GuardrailResult;
use crate::enums::{
    AgentKind, AgentStatus, BenchPhase, CandidateVerdict, Direction, EvaluatorKind,
    OptimizationStatus, SandboxProviderKind, SandboxStatus, SpendCategory, ValueBasis, ValuePeriod,
};
use crate::ids::{AgentId, CandidateId, OptimizationId, SandboxId};

/// Summary statistics of a benchmark run set. `value` is the headline number the ratchet compares; the other
/// entries (`mean`, `stddev`, `p95`, ...) are whatever the evaluator reported, in the order it reported them (a JS
/// object keeps insertion order, and so does the JSON stored in the ledger).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BenchStats {
    pub value: f64,
    #[serde(flatten)]
    #[cfg_attr(feature = "utoipa", schema(additional_properties = true, value_type = HashMap<String, f64>))]
    pub extra: IndexMap<String, f64>,
}

impl BenchStats {
    pub fn new(value: f64) -> Self {
        Self {
            value,
            extra: IndexMap::new(),
        }
    }

    pub fn with(mut self, key: &str, v: f64) -> Self {
        self.extra.insert(key.to_owned(), v);
        self
    }

    /// `value`, or an extra statistic by name.
    pub fn get(&self, key: &str) -> Option<f64> {
        if key == "value" {
            Some(self.value)
        } else {
            self.extra.get(key).copied()
        }
    }
}

/// USD per spend category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CategoryTotals {
    pub agent_runner: f64,
    pub experiment: f64,
    pub bench_runner: f64,
    pub storage_egress: f64,
    pub tokens: f64,
}

impl CategoryTotals {
    pub fn get(&self, c: SpendCategory) -> f64 {
        match c {
            SpendCategory::AgentRunner => self.agent_runner,
            SpendCategory::Experiment => self.experiment,
            SpendCategory::BenchRunner => self.bench_runner,
            SpendCategory::StorageEgress => self.storage_egress,
            SpendCategory::Tokens => self.tokens,
        }
    }

    pub fn add(&mut self, c: SpendCategory, usd: f64) {
        match c {
            SpendCategory::AgentRunner => self.agent_runner += usd,
            SpendCategory::Experiment => self.experiment += usd,
            SpendCategory::BenchRunner => self.bench_runner += usd,
            SpendCategory::StorageEgress => self.storage_egress += usd,
            SpendCategory::Tokens => self.tokens += usd,
        }
    }

    pub fn total(&self) -> f64 {
        SpendCategory::ALL.iter().map(|c| self.get(*c)).sum()
    }
}

/// One cloud sandbox (an applied `infra/aws` module). Optimizations run inside one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Sandbox {
    pub id: SandboxId,
    pub provider: SandboxProviderKind,
    pub region: String,
    pub account_id: Option<String>,
    pub status: SandboxStatus,
    /// Contract outputs after apply (see infra/contract.md); `None` until applied.
    #[cfg_attr(feature = "utoipa", schema(value_type = Option<Object>))]
    pub outputs: Option<Value>,
    pub created_at: i64,
    pub applied_at: Option<i64>,
    pub destroyed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Optimization {
    pub id: OptimizationId,
    pub name: Option<String>,
    pub repo: String,
    pub base_ref: String,
    pub metric: String,
    pub direction: Direction,
    /// Minimum relative gain, e.g. 0.03 = 3%.
    pub min_gain: f64,
    pub budget_usd: f64,
    pub time_limit_s: i64,
    pub status: OptimizationStatus,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    /// Snapshot of ratchet.yaml at creation time.
    pub config_yaml: String,
    /// Template category (`perf`, `forecasting`, ...) and the objective's display name, from ratchet.yaml.
    pub pack: Option<String>,
    pub objective_name: Option<String>,
    pub evaluator_kind: EvaluatorKind,
    /// `objective.value`: USD per unit of metric improvement per period. All `None` when no value model is set.
    pub value_usd_per_unit: Option<f64>,
    pub value_period: Option<ValuePeriod>,
    pub value_basis: Option<ValueBasis>,
    pub value_unit: Option<String>,
    pub sandbox_id: Option<SandboxId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub optimization_id: OptimizationId,
    pub samples: Vec<f64>,
    /// The baseline headline value (the ratchet starts here).
    pub value: f64,
    pub noise_floor: f64,
    pub stats: BenchStats,
    pub recorded_at: i64,
}

/// One coding agent working on its own branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: AgentId,
    pub optimization_id: OptimizationId,
    pub kind: AgentKind,
    pub branch: String,
    pub runner_instance_id: Option<String>,
    pub status: AgentStatus,
    pub created_at: i64,
}

/// A commit an agent submitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: CandidateId,
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub sha: String,
    pub message: String,
    pub submitted_at: i64,
    pub verdict: CandidateVerdict,
    /// When the verdict was set (`None` while pending).
    pub verdict_at: Option<i64>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BenchResult {
    pub id: i64,
    pub candidate_id: CandidateId,
    pub phase: BenchPhase,
    pub samples: Vec<f64>,
    pub stats: BenchStats,
    pub guardrails: Vec<GuardrailResult>,
    pub started_at: i64,
    pub finished_at: i64,
}

/// Append-only. Each row is a verified step of the ratchet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RatchetMove {
    pub id: i64,
    pub optimization_id: OptimizationId,
    pub candidate_id: CandidateId,
    pub from_value: f64,
    pub to_value: f64,
    pub at: i64,
}

/// Append-only. Corrections are written as new (possibly negative) rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SpendEvent {
    pub id: i64,
    pub optimization_id: OptimizationId,
    pub agent_id: Option<AgentId>,
    pub category: SpendCategory,
    pub usd: f64,
    /// True for live estimates, false once reconciled against billing.
    pub estimated: bool,
    pub source: String,
    pub at: i64,
}

/// Live log of an agent's parsed event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentEventRecord {
    pub id: i64,
    pub agent_id: AgentId,
    pub at: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[cfg_attr(feature = "utoipa", schema(value_type = Option<Object>))]
    pub payload: Option<Value>,
}

// ---- query results ------------------------------------------------------------------------------------------

/// One row of the Optimizations table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OptimizationSummary {
    pub id: OptimizationId,
    pub name: Option<String>,
    pub repo: String,
    pub metric: String,
    pub direction: Direction,
    pub status: OptimizationStatus,
    pub budget_usd: f64,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub baseline_value: Option<f64>,
    /// Current ratchet value (last verified move, else the baseline); `None` without a baseline.
    pub best_value: Option<f64>,
    /// Improvement vs baseline in percent; positive = better in the metric's direction.
    pub gain_pct: Option<f64>,
    pub spend_usd: f64,
    /// Distinct agent kinds in this optimization, sorted.
    pub agent_kinds: Vec<AgentKind>,
    pub agent_count: i64,
    pub candidate_count: i64,
    /// Number of ratchet moves (verified wins).
    pub verified_wins: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RatchetHistoryEntry {
    pub id: i64,
    pub optimization_id: OptimizationId,
    pub candidate_id: CandidateId,
    pub sha: String,
    pub agent_id: AgentId,
    pub agent_kind: AgentKind,
    pub from_value: f64,
    pub to_value: f64,
    /// Improvement of this step vs `from_value`, percent, positive = better.
    pub gain_pct: f64,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CandidateWithResults {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub agent_kind: AgentKind,
    pub agent_branch: String,
    pub results: Vec<BenchResult>,
    /// Headline value of the latest `bench` result, if any.
    pub bench_value: Option<f64>,
    /// Headline value of the latest `confirm` result, if any.
    pub confirm_value: Option<f64>,
    /// True if this candidate moved the ratchet.
    pub moved_ratchet: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentSpend {
    pub agent_id: Option<AgentId>,
    pub kind: Option<AgentKind>,
    pub branch: Option<String>,
    pub total: f64,
    pub by_category: CategoryTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SpendBreakdown {
    pub total: f64,
    pub estimated: f64,
    pub actual: f64,
    pub by_category: CategoryTotals,
    /// Per agent (creation order), plus one entry with `agent_id: None` for optimization-level spend (if any).
    pub by_agent: Vec<AgentSpend>,
}

/// An item of the merged, newest-first feed of verdicts and ratchet moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActivityItem {
    Verdict {
        at: i64,
        optimization_id: OptimizationId,
        candidate_id: CandidateId,
        agent_id: AgentId,
        agent_kind: AgentKind,
        sha: String,
        message: String,
        verdict: CandidateVerdict,
        reasons: Vec<String>,
    },
    RatchetMove {
        at: i64,
        optimization_id: OptimizationId,
        candidate_id: CandidateId,
        agent_id: AgentId,
        agent_kind: AgentKind,
        sha: String,
        message: String,
        from_value: f64,
        to_value: f64,
    },
}

impl ActivityItem {
    pub fn at(&self) -> i64 {
        match self {
            ActivityItem::Verdict { at, .. } | ActivityItem::RatchetMove { at, .. } => *at,
        }
    }

    pub fn optimization_id(&self) -> &OptimizationId {
        match self {
            ActivityItem::Verdict {
                optimization_id, ..
            }
            | ActivityItem::RatchetMove {
                optimization_id, ..
            } => optimization_id,
        }
    }

    pub fn candidate_id(&self) -> &CandidateId {
        match self {
            ActivityItem::Verdict { candidate_id, .. }
            | ActivityItem::RatchetMove { candidate_id, .. } => candidate_id,
        }
    }
}

/// Strictly better in `direction`?
pub fn is_improvement(direction: Direction, from: f64, to: f64) -> bool {
    match direction {
        Direction::Minimize => to < from,
        Direction::Maximize => to > from,
    }
}

/// Percent improvement of `to` over `from`; positive = better. `None` if `from` is 0.
pub fn gain_percent(direction: Direction, from: f64, to: f64) -> Option<f64> {
    if from == 0.0 {
        return None;
    }
    let delta = match direction {
        Direction::Minimize => from - to,
        Direction::Maximize => to - from,
    };
    Some((delta / from.abs()) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ledger.test.ts "helpers: isImprovement and gainPercent"
    #[test]
    fn helpers_is_improvement_and_gain_percent() {
        assert!(is_improvement(Direction::Minimize, 10.0, 9.0));
        assert!(!is_improvement(Direction::Minimize, 10.0, 10.0));
        assert!(is_improvement(Direction::Maximize, 10.0, 11.0));
        assert!(!is_improvement(Direction::Maximize, 10.0, 9.0));
        assert_eq!(gain_percent(Direction::Minimize, 100.0, 75.0), Some(25.0));
        assert_eq!(gain_percent(Direction::Maximize, 100.0, 150.0), Some(50.0));
        assert_eq!(gain_percent(Direction::Minimize, 0.0, 0.0), None);
    }

    #[test]
    fn bench_stats_flatten_like_the_typescript_record() {
        let s = BenchStats::new(90.0).with("stddev", 1.0);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, serde_json::json!({"value": 90.0, "stddev": 1.0}));
        let back: BenchStats = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.get("stddev"), Some(1.0));
        assert_eq!(back.get("value"), Some(90.0));
    }

    #[test]
    fn activity_items_are_tagged_by_kind() {
        let item = ActivityItem::RatchetMove {
            at: 1,
            optimization_id: "o".into(),
            candidate_id: "c".into(),
            agent_id: "a".into(),
            agent_kind: AgentKind::Codex,
            sha: "s".into(),
            message: "m".into(),
            from_value: 100.0,
            to_value: 90.0,
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["kind"], "ratchet_move");
        assert_eq!(v["fromValue"], 100.0);
        assert_eq!(v["agentKind"], "codex");
    }

    #[test]
    fn category_totals_add_and_total() {
        let mut t = CategoryTotals::default();
        t.add(SpendCategory::Tokens, 4.0);
        t.add(SpendCategory::AgentRunner, 1.5);
        assert_eq!(t.get(SpendCategory::Tokens), 4.0);
        assert_eq!(t.total(), 5.5);
        let v = serde_json::to_value(t).unwrap();
        assert_eq!(v["agent_runner"], 1.5);
    }
}
