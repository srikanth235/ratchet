import type { AgentKind, OptimizationStatus, Verdict } from '@ratchet/ui';

export type { AgentKind, OptimizationStatus, Verdict };
export type Mode = 'live' | 'simulation';

// ------------------------------------------------------------------ view models (main → renderer)

/** Window the Optimizations KPI cards cover (compared with the equal-length window before it). */
export type KpiRange = '7d' | '30d' | '90d';
export const KPI_RANGES: readonly KpiRange[] = ['7d', '30d', '90d'];

/** Change vs the previous equal-length window, e.g. `+3`, `+6 pts`, `−12%`. `good` / `bad` colour it. */
export interface KpiChange {
  text: string;
  tone: 'good' | 'bad' | 'neutral';
}

export interface Kpi {
  id: string;
  label: string;
  value: string;
  unit?: string;
  /** Null when the previous window has no data to compare with. */
  change: KpiChange | null;
  foot: string;
  trend: number[];
}

/** `objective.value` applied to baseline → best (core `estimateValue` / `returnOnSpend`). */
export interface ValueVM {
  usdPerUnit: number;
  unit: string | null;
  period: 'month' | 'year';
  basis: 'estimated' | 'measured';
  usdPerYear: number;
  /** Yearly value per dollar spent; null when nothing was spent. */
  returnOnSpend: number | null;
}

/** One candidate on the ratchet chart. `verified` = it moved the ratchet. */
export interface RatchetPoint {
  t: number; // minutes since start
  value: number;
  agent: AgentKind;
  verified: boolean;
  label?: string;
}

/**
 * A metric the evaluator reports that belongs with the objective: the objective's siblings (`http_req_duration.p50`
 * next to `.p95`), or for a top-level objective the other top-level metrics that are not guardrails (`mape`, `bias`
 * next to `wape`).
 */
export interface MetricOption {
  /** Flattened metric name, e.g. `http_req_duration.p99`. */
  key: string;
  /** Short label, e.g. `p99`. */
  label: string;
  unit: string;
  objective: boolean;
}

export interface RatchetSeries {
  /** The metric this series plots (the objective unless another option was asked for). */
  metric: string;
  /** Switch options (objective included); the screen shows a switch only when there are several. */
  options: MetricOption[];
  unit: string;
  direction: 'lower' | 'higher';
  baseline: number | null;
  /** Staircase of the best value; first step is the baseline at t=0. */
  steps: { t: number; value: number }[];
  /** Every candidate that has a bench result. */
  points: RatchetPoint[];
  /** Absolute noise floor of the baseline (metric units). */
  noiseFloor: number | null;
  tMax: number;
}

export interface Optimization {
  id: string;
  name: string;
  repo: string;
  metric: string;
  unit: string;
  direction: 'lower' | 'higher';
  baseline: number | null;
  best: number | null;
  gainPct: number | null;
  status: OptimizationStatus;
  agents: AgentKind[];
  spendUsd: number;
  budgetUsd: number;
  updated: string;
  candidateCount: number;
  verifiedWins: number;
  createdAt: number;
  /** Domain pack and evaluator kind from ratchet.yaml (ledger columns). */
  pack: string | null;
  evaluatorKind: 'benchmark' | 'backtest' | 'eval' | 'simulation';
  /** Dollar value of the improvement so far; null without `objective.value` or without an improvement. */
  value: ValueVM | null;
}

export interface ActivityItem {
  id: string;
  optimizationId: string;
  agent: AgentKind | null;
  text: string;
  at: string;
  atMs: number;
  tone: 'brass' | 'mint' | 'rose' | 'neutral';
}

export interface OptimizationsSnapshot {
  range: KpiRange;
  kpis: Kpi[];
  featuredId: string | null;
  /** Series for the featured optimization's chart. */
  featuredSeries: RatchetSeries | null;
  optimizations: Optimization[];
  activity: ActivityItem[];
  engine: EngineInfo;
}

export interface EngineInfo {
  /** `dev` = the in-repo simulated engine stub; `real` = @ratchet/core's engine. */
  kind: 'dev' | 'real';
  mode: Mode;
}

export interface AgentRecentEval {
  value: number;
  verdict: Verdict;
  label: string;
}

export interface AgentVM {
  id: string;
  kind: AgentKind;
  branch: string;
  status: 'pending' | 'launching' | 'running' | 'paused' | 'stopped' | 'done' | 'failed';
  /** Latest agent message, used as the "current hypothesis". */
  hypothesis: string | null;
  recent: AgentRecentEval[];
  stats: { candidates: number; verified: number; rejected: number; blocked: number; spendUsd: number };
  lastEventId: number;
  runnerInstanceId: string | null;
}

export type PipelineStageState = 'done' | 'active' | 'idle' | 'failed';

export interface PipelineStage {
  id: 'checkout' | 'guardrails' | 'bench' | 'confirm' | 'verdict';
  label: string;
  state: PipelineStageState;
  detail: string;
}

export interface BenchPipelineVM {
  /** Candidate the pipeline is currently working on / last worked on. */
  candidate: { id: string; sha: string; agent: AgentKind } | null;
  queued: number;
  stages: PipelineStage[];
}

export interface TeardownItem {
  id: string;
  label: string;
  done: boolean;
}

/** One row of "Baseline vs best" / "Before and after": objective family and guardrail metrics. */
export interface CompareRow {
  key: string;
  label: string;
  unit: string;
  baseline: number | null;
  best: number | null;
  /** Signed % change best vs baseline (null without both). */
  deltaPct: number | null;
  /** Which way is better for this metric (objective direction, or the guardrail's bound). */
  better: 'lower' | 'higher';
}

/** "Where the time went": one evaluator-reported span (`span.<name>`), baseline vs best verified, in ms. */
export interface SpanRow {
  name: string;
  baseline: number;
  best: number;
}

export interface OptimizationDetail {
  summary: Optimization;
  baseRef: string;
  minGain: number;
  timeLimitS: number;
  startedAt: number | null;
  endedAt: number | null;
  elapsedS: number;
  baselineSamples: number[];
  baselineStats: Record<string, number>;
  noiseFloor: number | null;
  agents: AgentVM[];
  pipeline: BenchPipelineVM;
  prUrl: string | null;
  teardown: TeardownItem[];
  configYaml: string;
  /** Metrics comparable with the objective (see MetricOption), from the baseline's stats. */
  metrics: MetricOption[];
  /** Baseline vs best verified, objective family first, then guardrail metrics; empty before the first win. */
  compare: CompareRow[];
  /** Null when the evaluator reports no spans or there is no verified win yet. */
  spans: SpanRow[] | null;
}

export interface DiffLine {
  kind: 'add' | 'del' | 'ctx' | 'hunk';
  text: string;
}

export interface DiffFileVM {
  path: string;
  additions: number;
  deletions: number;
  binary: boolean;
}

/** The squashed result: best verified commit vs the base ref, as the pull request will show it. */
export interface WinningChangeVM {
  /** Verified commits (ratchet moves) the PR squashes. */
  commits: number;
  /** Short sha of the best verified commit; null before the first win. */
  sha: string | null;
  baseRef: string;
  files: DiffFileVM[];
  additions: number;
  deletions: number;
  /** First hunk of the most-changed file. */
  excerpt: { path: string; lines: DiffLine[] } | null;
  /** Why there is no diff (no wins yet, the engine has no git access, git failed); null when `files` is real. */
  unavailable: string | null;
}

export interface CandidateVM {
  id: string;
  sha: string;
  agentId: string;
  agent: AgentKind;
  message: string;
  submittedAt: number;
  /** Minutes since the optimization started. */
  t: number;
  verdict: Verdict;
  reasons: string[];
  benchValue: number | null;
  confirmValue: number | null;
  /** Signed % improvement vs the baseline (positive = better). */
  vsBaselinePct: number | null;
  movedRatchet: boolean;
  samples: number[];
  /** Stats of the latest bench run (`value`, and p50/p95/p99 when the bench reports them). */
  stats: Record<string, number>;
}

export interface SpendLine {
  id: string;
  label: string;
  usd: number;
}

export interface SpendVM {
  total: number;
  estimated: number;
  actual: number;
  budgetUsd: number;
  byCategory: SpendLine[];
  byAgent: { agentId: string | null; kind: AgentKind | null; branch: string | null; usd: number }[];
  /** Cumulative spend over minutes since start. */
  series: { t: number; usd: number }[];
}

export interface AgentEventVM {
  id: number;
  at: number;
  type: string;
  text: string;
}

export interface AgentEventsPage {
  events: AgentEventVM[];
  /** Pass back as `cursor` to get only newer events. */
  cursor: number;
}

export interface ConfigSummary {
  /** `objective.name`, the default optimization name. */
  name: string | null;
  pack: string | null;
  metric: string;
  direction: 'minimize' | 'maximize';
  minGain: number;
  evaluator: { kind: 'benchmark' | 'backtest' | 'eval' | 'simulation'; ref: string; run: string; repeats: number; hardware: string; holdout: string | null };
  value: { usdPerUnit: number; period: 'month' | 'year'; basis: 'estimated' | 'measured'; unit: string | null } | null;
  guardrails: string[];
  protectedGlobs: string[];
  instanceTypes: string[];
}

export type ValidateConfigResult =
  | { ok: true; summary: ConfigSummary }
  | { ok: false; errors: { path: string; message: string }[] };

export interface PreflightItem {
  id: string;
  label: string;
  state: 'ok' | 'warn' | 'fail';
  detail: string;
}

export interface PlanChangeVM {
  action: 'create' | 'update' | 'delete' | 'replace' | 'no-op';
  address: string;
  type: string;
}

export interface SandboxInfo {
  plan: { changes: PlanChangeVM[]; summary: string };
  outputs: {
    artifactsBucket: string;
    sealedBucket: string;
    agentBoundaryRef: string;
    evalIdentityRef: string;
    agentRolePath: string;
    network: { subnetId: string; agentRunnerSecurityGroupId: string; benchSecurityGroupId: string };
  } | null;
  account: { sandboxId: string; region: string; profile: string; accountId: string | null; simulated: boolean };
  limits: { allowedInstanceTypes: string[]; maxInstancesPerAgent: number; maxInstanceLifetimeHours: number };
}

export type SecretName = 'anthropicApiKey' | 'openaiApiKey' | 'githubToken';

/** Secrets never cross to the renderer: only whether each one is configured. */
export interface SettingsVM {
  mode: Mode;
  awsProfile: string;
  secrets: Record<SecretName, boolean>;
  /** False when the OS keychain (safeStorage) is unavailable, so secrets cannot be stored. */
  encryptionAvailable: boolean;
  engine: EngineInfo;
}

export type ExportReportResult = { saved: true; path: string } | { saved: false };

export type EngineEventVM =
  | { type: 'optimization'; optimizationId: string; status: string }
  | { type: 'agent-event'; optimizationId: string; agentId: string }
  | { type: 'candidate'; optimizationId: string; candidateId: string; verdict: Verdict }
  | { type: 'ratchet'; optimizationId: string; from: number; to: number }
  | { type: 'spend'; optimizationId: string; totalUsd: number }
  | { type: 'notice'; optimizationId: string; level: 'info' | 'warning' | 'error'; text: string };

// ------------------------------------------------------------------ inputs (renderer → main), validated by zod in shared/ipc.ts

export interface CreateOptimizationInput {
  name?: string;
  repoUrl: string;
  baseRef: string;
  configPath?: string;
  /** The reviewed ratchet.yaml text (the engine reads the repo's copy; the dev engine uses this one). */
  configYaml?: string;
  agents: AgentKind[];
  budgetUsd: number;
  timeLimitSeconds: number;
  plateauEvals: number;
}

/** The only surface the renderer gets from the main process (`window.ratchet`). */
export interface RatchetApi {
  // reads
  listOptimizations(opts?: { range?: KpiRange }): Promise<OptimizationsSnapshot>;
  getOptimization(id: string): Promise<OptimizationDetail>;
  /** `metric`: one of the series' `options` keys; default the objective. */
  ratchetHistory(id: string, metric?: string): Promise<RatchetSeries>;
  winningChange(id: string): Promise<WinningChangeVM>;
  candidatesWithResults(id: string): Promise<CandidateVM[]>;
  spendBreakdown(id: string): Promise<SpendVM>;
  recentActivity(opts?: { optimizationId?: string; limit?: number }): Promise<ActivityItem[]>;
  agentEvents(agentId: string, cursor?: number, limit?: number): Promise<AgentEventsPage>;
  validateConfig(text: string): Promise<ValidateConfigResult>;
  preflight(input: { agents: AgentKind[]; configYaml: string }): Promise<PreflightItem[]>;
  sandboxInfo(): Promise<SandboxInfo>;
  getSettings(): Promise<SettingsVM>;
  // commands (mirror EngineApi)
  createOptimization(input: CreateOptimizationInput): Promise<string>;
  startOptimization(id: string): Promise<void>;
  stopOptimization(id: string, reason?: string): Promise<void>;
  pauseAgent(agentId: string): Promise<void>;
  resumeAgent(agentId: string): Promise<void>;
  messageAgent(agentId: string, text: string): Promise<void>;
  openPullRequest(id: string): Promise<{ url: string }>;
  teardown(id: string): Promise<void>;
  exportReport(id: string): Promise<ExportReportResult>;
  // settings
  setMode(mode: Mode): Promise<SettingsVM>;
  setAwsProfile(profile: string): Promise<SettingsVM>;
  setSecret(name: SecretName, value: string): Promise<SettingsVM>;
  clearSecret(name: SecretName): Promise<SettingsVM>;
  // events
  onEvent(listener: (e: EngineEventVM) => void): () => void;
}
