//! View models: the shaped data the screens render (the `*VM` types of `packages/desktop/src/shared/types.ts`).
//!
//! Where `ratchet-core` already has the type (enums, `TemplateInfo`, `Calibration`, `CostReconcileReport`, ...)
//! the contract reuses it; the rest is defined here. JSON is camelCase; absent values are `null`, as in the
//! TypeScript view models. Timestamps are epoch milliseconds, money is USD.

use std::collections::BTreeMap;

use ratchet_core::config::ConfigError;
use ratchet_core::enums::{
    AgentKind, AgentStatus, CandidateVerdict, Direction, EvaluatorKind, Mode, OptimizationStatus,
    ValueBasis, ValuePeriod,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Window the Optimizations KPI cards cover (compared with the equal-length window before it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub enum KpiRange {
    #[serde(rename = "7d")]
    Days7,
    #[default]
    #[serde(rename = "30d")]
    Days30,
    #[serde(rename = "90d")]
    Days90,
}

impl KpiRange {
    pub const fn days(self) -> i64 {
        match self {
            KpiRange::Days7 => 7,
            KpiRange::Days30 => 30,
            KpiRange::Days90 => 90,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Good,
    Bad,
    Neutral,
}

/// Change vs the previous equal-length window, e.g. `+3`, `+6 pts`, `-12%`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KpiChange {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Kpi {
    pub id: String,
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub unit: Option<String>,
    /// `null` when the previous window has no data to compare with.
    #[schema(required = true)]
    pub change: Option<KpiChange>,
    pub foot: String,
    pub trend: Vec<f64>,
}

/// `objective.value` applied to baseline -> best (core `estimate_value` / `return_on_spend`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValueVm {
    pub usd_per_unit: f64,
    #[schema(required = true)]
    pub unit: Option<String>,
    pub period: ValuePeriod,
    pub basis: ValueBasis,
    pub usd_per_year: f64,
    /// Yearly value per dollar spent; `null` when nothing was spent.
    #[schema(required = true)]
    pub return_on_spend: Option<f64>,
}

/// Which way is better, as the screens say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Better {
    Lower,
    Higher,
}

impl From<Direction> for Better {
    fn from(d: Direction) -> Self {
        match d {
            Direction::Minimize => Better::Lower,
            Direction::Maximize => Better::Higher,
        }
    }
}

/// One candidate on the ratchet chart. `verified` = it moved the ratchet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatchetPoint {
    /// Minutes since start.
    pub t: f64,
    pub value: f64,
    pub agent: AgentKind,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub label: Option<String>,
}

/// A metric the evaluator reports that belongs with the objective: the objective's siblings
/// (`http_req_duration.p50` next to `.p95`), or for a top-level objective the other top-level metrics that are not
/// guardrails (`mape`, `bias` next to `wape`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricOption {
    /// Flattened metric name, e.g. `http_req_duration.p99`.
    pub key: String,
    /// Short label, e.g. `p99`.
    pub label: String,
    pub unit: String,
    pub objective: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepPoint {
    pub t: f64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatchetSeries {
    /// The metric this series plots (the objective unless another option was asked for).
    pub metric: String,
    /// Switch options (objective included); the screen shows a switch only when there are several.
    pub options: Vec<MetricOption>,
    pub unit: String,
    pub direction: Better,
    #[schema(required = true)]
    pub baseline: Option<f64>,
    /// Staircase of the best value; the first step is the baseline at t=0.
    pub steps: Vec<StepPoint>,
    /// Every candidate that has a bench result.
    pub points: Vec<RatchetPoint>,
    /// Absolute noise floor of the baseline (metric units).
    #[schema(required = true)]
    pub noise_floor: Option<f64>,
    pub t_max: f64,
}

/// The optimization status as the screens show it: the ledger status, or `pr-open` once a pull request exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UiStatus {
    Draft,
    Baselining,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
    PrOpen,
}

impl From<OptimizationStatus> for UiStatus {
    fn from(s: OptimizationStatus) -> Self {
        match s {
            OptimizationStatus::Draft => UiStatus::Draft,
            OptimizationStatus::Baselining => UiStatus::Baselining,
            OptimizationStatus::Running => UiStatus::Running,
            OptimizationStatus::Stopping => UiStatus::Stopping,
            OptimizationStatus::Completed => UiStatus::Completed,
            OptimizationStatus::Stopped => UiStatus::Stopped,
            OptimizationStatus::Failed => UiStatus::Failed,
        }
    }
}

/// One row of the Optimizations table (and the header of the detail screen).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationVm {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub metric: String,
    pub unit: String,
    pub direction: Better,
    #[schema(required = true)]
    pub baseline: Option<f64>,
    #[schema(required = true)]
    pub best: Option<f64>,
    #[schema(required = true)]
    pub gain_pct: Option<f64>,
    pub status: UiStatus,
    pub agents: Vec<AgentKind>,
    pub spend_usd: f64,
    pub budget_usd: f64,
    /// "live" while running, else a relative time ("3h ago").
    pub updated: String,
    pub candidate_count: i64,
    pub verified_wins: i64,
    pub created_at: i64,
    /// Template category and evaluator kind from ratchet.yaml (ledger columns).
    #[schema(required = true)]
    pub pack: Option<String>,
    pub evaluator_kind: EvaluatorKind,
    /// Dollar value of the improvement so far; `null` without `objective.value` or without an improvement.
    #[schema(required = true)]
    pub value: Option<ValueVm>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActivityTone {
    Brass,
    Mint,
    Rose,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityVm {
    pub id: String,
    pub optimization_id: String,
    #[schema(required = true)]
    pub agent: Option<AgentKind>,
    pub text: String,
    /// Relative time ("12m ago").
    pub at: String,
    pub at_ms: i64,
    pub tone: ActivityTone,
}

/// Which engine answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub kind: EngineKind,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    /// The simulation service (seeded demo data, scripted agents).
    Sim,
    /// `ratchet-engine`.
    Real,
}

/// `GET /v1/optimizations`: everything the Optimizations screen shows in one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationsSnapshot {
    pub range: KpiRange,
    pub kpis: Vec<Kpi>,
    #[schema(required = true)]
    pub featured_id: Option<String>,
    /// Series for the featured optimization's chart.
    #[schema(required = true)]
    pub featured_series: Option<RatchetSeries>,
    pub optimizations: Vec<OptimizationVm>,
    pub activity: Vec<ActivityVm>,
    pub engine: EngineInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecentEval {
    pub value: f64,
    pub verdict: CandidateVerdict,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentStats {
    pub candidates: i64,
    pub verified: i64,
    pub rejected: i64,
    pub blocked: i64,
    pub spend_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentVm {
    pub id: String,
    pub kind: AgentKind,
    pub branch: String,
    pub status: AgentStatus,
    /// Latest agent message, used as the "current hypothesis".
    #[schema(required = true)]
    pub hypothesis: Option<String>,
    pub recent: Vec<AgentRecentEval>,
    pub stats: AgentStats,
    /// Pass to `GET /v1/agents/{id}/events?cursor=` to read only newer events.
    pub last_event_id: i64,
    #[schema(required = true)]
    pub runner_instance_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StageState {
    Done,
    Active,
    Idle,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StageId {
    Checkout,
    Guardrails,
    Bench,
    Confirm,
    Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStage {
    pub id: StageId,
    pub label: String,
    pub state: StageState,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineCandidate {
    pub id: String,
    pub sha: String,
    pub agent: AgentKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BenchPipelineVm {
    /// Candidate the pipeline is working on / last worked on.
    #[schema(required = true)]
    pub candidate: Option<PipelineCandidate>,
    pub queued: i64,
    pub stages: Vec<PipelineStage>,
}

/// What a sandbox teardown removed (counts; ids stay in the ledger).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptRemoved {
    pub instances: i64,
    pub agent_identities: i64,
    pub secrets: i64,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LabeledUsd {
    pub label: String,
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptRun {
    pub optimization_id: String,
    pub status: String,
    pub spend_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptTrigger {
    /// Removed automatically after the idle window.
    Idle,
    /// "End sandbox now".
    Manual,
}

/// The receipt of the sandbox teardown that covered a run (core `SandboxTeardownReceipt`, trimmed for the UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeardownReceiptVm {
    pub sandbox_id: String,
    pub region: String,
    pub trigger: ReceiptTrigger,
    pub destroyed_at: i64,
    pub spend_usd: f64,
    pub spend_by_category: Vec<LabeledUsd>,
    pub runs: Vec<ReceiptRun>,
    pub removed: ReceiptRemoved,
    /// Verified after the destroy: nothing tagged is left, so nothing is billing.
    pub nothing_running: bool,
    pub orphans: Vec<String>,
    pub errors: Vec<String>,
    pub summary: String,
}

/// One row of "Baseline vs best" / "Before and after".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompareRow {
    pub key: String,
    pub label: String,
    pub unit: String,
    #[schema(required = true)]
    pub baseline: Option<f64>,
    #[schema(required = true)]
    pub best: Option<f64>,
    /// Signed % change best vs baseline.
    #[schema(required = true)]
    pub delta_pct: Option<f64>,
    /// Which way is better for this metric.
    pub better: Better,
}

/// "Where the time went": one evaluator-reported span, baseline vs best verified, in ms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpanRow {
    pub name: String,
    pub baseline: f64,
    pub best: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationDetail {
    pub summary: OptimizationVm,
    pub base_ref: String,
    pub min_gain: f64,
    pub time_limit_s: i64,
    #[schema(required = true)]
    pub started_at: Option<i64>,
    #[schema(required = true)]
    pub ended_at: Option<i64>,
    pub elapsed_s: f64,
    pub baseline_samples: Vec<f64>,
    pub baseline_stats: BTreeMap<String, f64>,
    #[schema(required = true)]
    pub noise_floor: Option<f64>,
    pub agents: Vec<AgentVm>,
    pub pipeline: BenchPipelineVm,
    #[schema(required = true)]
    pub pr_url: Option<String>,
    /// The sandbox teardown receipt; `null` while the environment is still alive (or the run never used one).
    #[schema(required = true)]
    pub receipt: Option<TeardownReceiptVm>,
    /// Plain-words calibration ("this metric is noisy (+-9%); wins must beat 9%"); `null` until the baseline ran.
    #[schema(required = true)]
    pub calibration: Option<String>,
    /// Read-only: the repo owns ratchet.yaml, changes go through a pull request.
    pub config_yaml: String,
    /// Metrics comparable with the objective, from the baseline's stats.
    pub metrics: Vec<MetricOption>,
    /// Baseline vs best verified, objective family first, then guardrail metrics; empty before the first win.
    pub compare: Vec<CompareRow>,
    /// `null` when the evaluator reports no spans or there is no verified win yet.
    #[schema(required = true)]
    pub spans: Option<Vec<SpanRow>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Add,
    Del,
    Ctx,
    Hunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffFileVm {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffExcerpt {
    pub path: String,
    pub lines: Vec<DiffLine>,
}

/// The squashed result: best verified commit vs the base ref, as the pull request will show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WinningChangeVm {
    /// Verified commits (ratchet moves) the PR squashes.
    pub commits: i64,
    /// Short sha of the best verified commit; `null` before the first win.
    #[schema(required = true)]
    pub sha: Option<String>,
    pub base_ref: String,
    pub files: Vec<DiffFileVm>,
    pub additions: u64,
    pub deletions: u64,
    /// First hunk of the most-changed file.
    #[schema(required = true)]
    pub excerpt: Option<DiffExcerpt>,
    /// Why there is no diff (no wins yet, the engine has no git access, git failed); `null` when `files` is real.
    #[schema(required = true)]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CandidateVm {
    pub id: String,
    pub sha: String,
    pub agent_id: String,
    pub agent: AgentKind,
    pub message: String,
    pub submitted_at: i64,
    /// Minutes since the optimization started.
    pub t: f64,
    pub verdict: CandidateVerdict,
    pub reasons: Vec<String>,
    #[schema(required = true)]
    pub bench_value: Option<f64>,
    #[schema(required = true)]
    pub confirm_value: Option<f64>,
    /// Signed % improvement vs the baseline (positive = better).
    #[schema(required = true)]
    pub vs_baseline_pct: Option<f64>,
    pub moved_ratchet: bool,
    pub samples: Vec<f64>,
    /// Stats of the latest bench run (`value`, and p50/p95/p99 when the bench reports them).
    pub stats: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendLine {
    pub id: String,
    pub label: String,
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendByAgent {
    #[schema(required = true)]
    pub agent_id: Option<String>,
    #[schema(required = true)]
    pub kind: Option<AgentKind>,
    #[schema(required = true)]
    pub branch: Option<String>,
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendPoint {
    pub t: f64,
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendVm {
    pub total: f64,
    pub estimated: f64,
    pub actual: f64,
    pub budget_usd: f64,
    pub by_category: Vec<SpendLine>,
    pub by_agent: Vec<SpendByAgent>,
    /// Cumulative spend over minutes since start.
    pub series: Vec<SpendPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventVm {
    pub id: i64,
    pub at: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventsPage {
    pub events: Vec<AgentEventVm>,
    /// Pass back as `cursor` to get only newer events.
    pub cursor: i64,
}

// ---- config ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatorSummary {
    pub kind: EvaluatorKind,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub run: String,
    #[schema(required = true)]
    pub repeats: Option<u64>,
    pub hardware: String,
    #[schema(required = true)]
    pub holdout: Option<String>,
    #[schema(required = true)]
    pub template: Option<String>,
}

/// `environment:` of ratchet.yaml (all optional). `image` null = the evaluator runs on the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSummary {
    #[schema(required = true)]
    pub image: Option<String>,
    pub machine: String,
    #[schema(required = true)]
    pub region: Option<String>,
    pub network: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValueSummary {
    pub usd_per_unit: f64,
    pub period: ValuePeriod,
    pub basis: ValueBasis,
    #[schema(required = true)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedEntryVm {
    pub glob: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSummary {
    /// `objective.name`, the default optimization name.
    #[schema(required = true)]
    pub name: Option<String>,
    #[schema(required = true)]
    pub pack: Option<String>,
    pub metric: String,
    pub direction: Direction,
    pub min_gain: f64,
    pub evaluator: EvaluatorSummary,
    pub environment: EnvironmentSummary,
    #[schema(required = true)]
    pub value: Option<ValueSummary>,
    pub guardrails: Vec<String>,
    /// The user's own `protected:` additions.
    pub protected_globs: Vec<String>,
    /// Everything that is protected (automatic set plus the user's), with why.
    pub protected_entries: Vec<ProtectedEntryVm>,
    pub instance_types: Vec<String>,
}

/// Result of validating `ratchet.yaml` text: a summary when it is valid, the problems when it is not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateConfigResult {
    pub ok: bool,
    #[schema(required = true)]
    pub summary: Option<ConfigSummary>,
    pub errors: Vec<ConfigError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PreflightState {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreflightItem {
    pub id: String,
    pub label: String,
    pub state: PreflightState,
    pub detail: String,
}
