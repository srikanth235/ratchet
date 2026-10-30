import type { AgentKind, OptimizationStatus, Verdict } from '@ratchet/ui';
import type { AwsAuthKind, LiveSettings, LiveSettingsInput } from './live-settings.js';

export type { AgentKind, OptimizationStatus, Verdict };
export type { AwsAuthKind, LiveSettings, LiveSettingsInput };
export type Mode = 'live' | 'simulation';

// ------------------------------------------------------------------ view models (main → renderer)

export interface Kpi {
  id: string;
  label: string;
  value: string;
  unit?: string;
  delta: string;
  trend: number[];
}

/** One candidate on the ratchet chart. `verified` = it moved the ratchet. */
export interface RatchetPoint {
  t: number; // minutes since start
  value: number;
  agent: AgentKind;
  verified: boolean;
  label?: string;
}

export interface RatchetSeries {
  metric: string;
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
  metric: string;
  direction: 'minimize' | 'maximize';
  minGain: number;
  benchRun: string;
  repeats: number;
  hardware: string;
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

/** Where a sandbox is in its lifecycle (`absent` = never planned). */
export type SandboxStatusVM = 'absent' | 'planned' | 'applying' | 'applied' | 'destroying' | 'destroyed' | 'failed';

export interface SandboxOutputsVM {
  artifactsBucket: string;
  sealedBucket: string;
  agentBoundaryRef: string;
  evalIdentityRef: string;
  agentRolePath: string;
  network: { subnetId: string; agentRunnerSecurityGroupId: string; benchSecurityGroupId: string };
}

export interface SandboxInfo {
  /**
   * The OpenTofu plan: free in simulation (always present), but a real `tofu plan` in live mode, so it is `null`
   * until the user asks for it (`planSandbox`) and again after an apply or destroy.
   */
  plan: { changes: PlanChangeVM[]; summary: string } | null;
  /** Contract outputs recorded in the ledger by the last apply; `null` unless the sandbox is applied. */
  outputs: SandboxOutputsVM | null;
  status: SandboxStatusVM;
  /** Why the last apply / destroy failed. */
  lastError: string | null;
  appliedAt: number | null;
  /** Optimizations linked to this sandbox. */
  optimizations: { id: string; status: string }[];
  /** Apply is possible (not busy, live settings complete in live mode). */
  canApply: boolean;
  /** Destroy is possible; otherwise `destroyBlockedReason` says why (a running optimization, un-torn-down runs). */
  canDestroy: boolean;
  destroyBlockedReason: string | null;
  /** Live mode: what has to be configured before the sandbox can be planned or applied. */
  setupProblems: string[];
  account: { sandboxId: string; region: string; profile: string; accountId: string | null; simulated: boolean };
  limits: { allowedInstanceTypes: string[]; maxInstancesPerAgent: number; maxInstanceLifetimeHours: number };
}

export type SecretName = 'anthropicApiKey' | 'openaiApiKey' | 'githubToken';

/** Live mode: non-secret settings, what the paths default to, and what still blocks a live run. */
export interface LiveSettingsVM extends LiveSettings {
  /** What empty `tofuBinary` / `moduleDir` resolve to (bundled `infra/aws` in a packaged app). */
  defaults: { tofuBinary: string; moduleDir: string };
  /** One line per requirement (AWS sign-in, region, OpenTofu, GitHub, model keys, AMIs, sandbox). */
  readiness: PreflightItem[];
  /** Nothing in `readiness` fails. */
  ready: boolean;
  /** Settings changed while an optimization was running; they apply once it has stopped. */
  restartRequired: boolean;
}

export type ConnectionTarget = 'aws' | 'github' | 'anthropic' | 'openai' | 'tofu';

export interface ConnectionResultVM {
  target: ConnectionTarget;
  ok: boolean;
  /** One line; never contains a key or token. */
  detail: string;
}

/** Secrets never cross to the renderer: only whether each one is configured. */
export interface SettingsVM {
  mode: Mode;
  awsProfile: string;
  secrets: Record<SecretName, boolean>;
  live: LiveSettingsVM;
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
  listOptimizations(): Promise<OptimizationsSnapshot>;
  getOptimization(id: string): Promise<OptimizationDetail>;
  ratchetHistory(id: string): Promise<RatchetSeries>;
  candidatesWithResults(id: string): Promise<CandidateVM[]>;
  spendBreakdown(id: string): Promise<SpendVM>;
  recentActivity(opts?: { optimizationId?: string; limit?: number }): Promise<ActivityItem[]>;
  agentEvents(agentId: string, cursor?: number, limit?: number): Promise<AgentEventsPage>;
  validateConfig(text: string): Promise<ValidateConfigResult>;
  preflight(input: { agents: AgentKind[]; configYaml: string }): Promise<PreflightItem[]>;
  sandboxInfo(): Promise<SandboxInfo>;
  getSettings(): Promise<SettingsVM>;
  // sandbox lifecycle (planned -> applying -> applied -> destroying -> destroyed / failed, recorded in the ledger)
  planSandbox(): Promise<SandboxInfo>;
  applySandbox(): Promise<SandboxInfo>;
  destroySandbox(): Promise<SandboxInfo>;
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
  /** Validated as a whole; nothing is saved if any field is invalid. */
  saveLiveSettings(input: LiveSettingsInput): Promise<SettingsVM>;
  /** Read-only probe of one live setting (STS, GitHub, a model API, `tofu version`); secrets are read in the main process. */
  testConnection(target: ConnectionTarget): Promise<ConnectionResultVM>;
  // events
  onEvent(listener: (e: EngineEventVM) => void): () => void;
}
