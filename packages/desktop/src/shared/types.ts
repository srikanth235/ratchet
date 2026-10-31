import type { AgentKind, OptimizationStatus, Verdict } from '@ratchet/ui';
import type { AwsAuthKind, LiveSettings, LiveSettingsInput } from './live-settings.js';

export type { AgentKind, OptimizationStatus, Verdict };
export type { AwsAuthKind, LiveSettings, LiveSettingsInput };
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

/** The receipt of the sandbox teardown that covered a run (core `SandboxTeardownReceipt`, trimmed for the UI). */
export interface TeardownReceiptVM {
  sandboxId: string;
  region: string;
  /** `idle`: removed automatically after the idle window; `manual`: "End sandbox now". */
  trigger: 'idle' | 'manual';
  destroyedAt: number;
  spendUsd: number;
  spendByCategory: { label: string; usd: number }[];
  runs: { optimizationId: string; status: string; spendUsd: number }[];
  removed: { instances: number; agentIdentities: number; secrets: number; resources: string[] };
  /** Verified after the destroy: nothing tagged is left, so nothing is billing. */
  nothingRunning: boolean;
  orphans: string[];
  errors: string[];
  summary: string;
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
  /** The sandbox teardown receipt; null while the environment is still alive (or the run never used one). */
  receipt: TeardownReceiptVM | null;
  /** Plain-words calibration ("this metric is noisy (±9%); wins must beat 9%"); null until the baseline has run. */
  calibration: string | null;
  /** Read-only: the repo owns ratchet.yaml, changes go through a pull request. */
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
  evaluator: { kind: 'benchmark' | 'backtest' | 'eval' | 'simulation'; ref: string; run: string; repeats: number | null; hardware: string; holdout: string | null; template: string | null };
  /** `environment:` of ratchet.yaml (all optional). `image` null = the evaluator runs on the host. */
  environment: { image: string | null; machine: string; region: string | null; network: string[] };
  value: { usdPerUnit: number; period: 'month' | 'year'; basis: 'estimated' | 'measured'; unit: string | null } | null;
  guardrails: string[];
  /** The user's own `protected:` additions. */
  protectedGlobs: string[];
  /** Everything that is protected (automatic set plus the user's), with why: core `effectiveProtected`. */
  protectedEntries: { glob: string; reason: string }[];
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

// ------------------------------------------------------------------ settings: GitHub, AWS, agents

export type SecretName = 'anthropicApiKey' | 'openaiApiKey' | 'githubToken';

/** The GitHub row of Settings. The token itself never leaves the main process. */
export interface GithubStatusVM {
  signedIn: boolean;
  /** The signed-in account. */
  login: string | null;
  /** `device`: "Sign in with GitHub"; `token`: pasted (Advanced); `env`: GITHUB_TOKEN / GH_TOKEN. */
  source: 'device' | 'token' | 'env' | null;
  /** False while the built-in placeholder OAuth client id is in use (set RATCHET_GITHUB_CLIENT_ID). */
  clientIdConfigured: boolean;
  /** The device-flow sign-in in progress (or the reason the last one ended). */
  flow: GithubFlowVM | null;
}

export interface GithubFlowVM {
  state: 'pending' | 'expired' | 'denied' | 'error';
  /** Shown to the user, who types it at `verificationUri`. */
  userCode: string;
  verificationUri: string;
  expiresAt: number;
  message: string | null;
}

/** A profile found in ~/.aws/config or ~/.aws/credentials (names only, never a key). */
export interface AwsProfileVM {
  name: string;
  kind: 'sso' | 'role' | 'keys' | 'process' | 'other';
  /** Where the name was found. */
  source: 'config' | 'credentials';
  /** The `sso-session` a profile uses. */
  ssoSession: string | null;
}

export interface AwsProfilesVM {
  profiles: AwsProfileVM[];
  /** `[sso-session x]` names in ~/.aws/config. */
  ssoSessions: string[];
  /** The profile Ratchet uses ('' = none chosen). */
  selected: string;
  region: string;
  defaultRegion: string;
  /** False when neither file exists. */
  filesFound: boolean;
}

export type HarnessKind = 'claude-code' | 'codex';

/** One detected agent CLI (core `HarnessInfo`). */
export interface HarnessVM {
  kind: HarnessKind;
  installed: boolean;
  version: string | null;
  path: string | null;
  auth: 'signed-in' | 'api-key' | 'signed-out';
  /** What to do next, or what was found. Never contains a credential. */
  hint: string;
}

export interface SettingsVM {
  awsProfile: string;
  awsRegion: string;
  secrets: Record<SecretName, boolean>;
  /** Development overrides (AMI ids, OpenTofu paths): under "Advanced (development)" in Settings. */
  dev: LiveSettings & {
    /** What empty `tofuBinary` / `moduleDir` resolve to (bundled `infra/aws` in a packaged app). */
    defaults: { tofuBinary: string; moduleDir: string };
  };
  /** False when the OS keychain (safeStorage) is unavailable, so secrets cannot be stored. */
  encryptionAvailable: boolean;
  engine: EngineInfo;
}

export type ConnectionTarget = 'aws' | 'github' | 'anthropic' | 'openai';

export interface ConnectionResultVM {
  target: ConnectionTarget;
  ok: boolean;
  /** One line; never contains a key or token. */
  detail: string;
}

export type ExportReportResult = { saved: true; path: string } | { saved: false };

// ------------------------------------------------------------------ repos and templates

export interface RepoVM {
  /** `owner/name` */
  fullName: string;
  defaultBranch: string;
  private: boolean;
  description: string | null;
  pushedAt: string | null;
}

/** Where an optimization starts from: a GitHub repo of the signed-in account, or a local folder (this-machine runs). */
export type RepoSourceInput = { kind: 'github'; repo: string } | { kind: 'local'; path: string };

export interface TemplateVM {
  id: string;
  kind: 'template' | 'demo';
  category: 'perf' | 'forecasting' | 'llm';
  title: string;
  description: string;
  measures: string;
  holdout: string | null;
  machine: string;
  evaluatorKind: 'benchmark' | 'backtest' | 'eval' | 'simulation';
  objective: { metric: string; direction: 'minimize' | 'maximize'; unit: string };
  files: string[];
}

/** What step 1 learned about the chosen repo (read from the default branch, never typed). */
export interface RepoInspectVM {
  source: RepoSourceInput;
  /** `owner/name` (also for a local folder, from its `origin`); null for a local folder with no GitHub remote. */
  repo: string | null;
  baseRef: string;
  /** The ratchet.yaml text, read-only; null when the repo has none. */
  configYaml: string | null;
  /** Parsed summary, or why it does not parse. Both empty when there is no ratchet.yaml. */
  config: ConfigSummary | null;
  errors: { path: string; message: string }[];
  /** Offered when there is no ratchet.yaml: "Add via pull request". */
  templates: TemplateVM[];
  /** Why the repo cannot be used here (a local folder with no GitHub remote, not a git repo, ...). */
  problem: string | null;
}

export interface ProposeTemplateInput {
  /** `owner/name` or a GitHub URL. */
  repoUrl: string;
  baseRef: string;
  templateId: string;
  overwrite?: boolean;
}

export interface ProposeTemplateVM {
  prUrl: string;
  branch: string;
  sha: string;
  templateId: string;
  files: string[];
}

export type DemoId = 'demo-search' | 'demo-cli' | 'demo-forecast';

// ------------------------------------------------------------------ environments (read-only)

export type SandboxPhaseVM = 'creating' | 'ready' | 'idle' | 'destroying' | 'destroyed' | 'failed';

/** One automatic environment, as seen from outside. */
export interface EnvironmentVM {
  sandboxId: string;
  region: string | null;
  phase: SandboxPhaseVM | 'none';
  /** The last phase message ("environment ready"). */
  text: string;
  simulated: boolean;
  activeRuns: string[];
  instancesRunning: number | null;
  /** Estimated spend of the runs that are active in it. */
  spendUsd: number;
  /** When the automatic destroy is due. */
  destroyAt: number | null;
  idleSince: number | null;
  /** Ending it now is possible (no run is active). */
  canEnd: boolean;
}

export interface EnvironmentsVM {
  environments: EnvironmentVM[];
  /** The most recent teardown across environments. */
  lastReceipt: TeardownReceiptVM | null;
  /** Idle window before an unused environment is removed, minutes. */
  idleMinutes: number;
}

export type EngineEventVM =
  | { type: 'optimization'; optimizationId: string; status: string }
  | { type: 'agent-event'; optimizationId: string; agentId: string }
  | { type: 'candidate'; optimizationId: string; candidateId: string; verdict: Verdict }
  | { type: 'ratchet'; optimizationId: string; from: number; to: number }
  | { type: 'spend'; optimizationId: string; totalUsd: number }
  | { type: 'notice'; optimizationId: string; level: 'info' | 'warning' | 'error'; text: string }
  /** True while any run is active. The main process turns it into `powerSaveBlocker` start / stop (no UI involved). */
  | { type: 'run-active'; active: boolean; optimizationIds: string[] }
  /** The automatic environment (sandbox) changing phase. */
  | { type: 'sandbox'; sandboxId: string; phase: 'creating' | 'ready' | 'idle' | 'destroying' | 'destroyed' | 'failed'; region: string | null; text: string };

// ------------------------------------------------------------------ inputs (renderer → main), validated by zod in shared/ipc.ts


// ------------------------------------------------------------------ inputs (renderer → main), validated by zod in shared/ipc.ts

export type AgentsWhere = 'local' | 'cloud';

export interface CreateOptimizationInput {
  name?: string;
  /** A GitHub repo (`owner/name`). Ignored for a demo. */
  repoUrl: string;
  baseRef: string;
  /** Run a demo template in simulation (mock agents, no setup) instead of a repo. */
  demo?: DemoId;
  agents: AgentKind[];
  agentsWhere?: AgentsWhere;
  budgetUsd: number;
  timeLimitSeconds: number;
  plateauEvals: number;
}

export interface PreflightInput {
  agents: AgentKind[];
  agentsWhere: AgentsWhere;
  /** `demo` needs nothing; `github` / `local` need GitHub, AWS and agents. */
  source: 'github' | 'local' | 'demo';
}

export * from './overview.js';
import type { OverviewRange, OverviewVM } from './overview.js';

/** The only surface the renderer gets from the main process (`window.ratchet`). */
export interface RatchetApi {
  // reads
  listOptimizations(opts?: { range?: KpiRange }): Promise<OptimizationsSnapshot>;
  /** The Overview: spend, value locked in, return, the value staircase, what needs the user, and the optimizations of the range (default 90d). */
  overview(range?: OverviewRange): Promise<OverviewVM>;
  getOptimization(id: string): Promise<OptimizationDetail>;
  /** `metric`: one of the series' `options` keys; default the objective. */
  ratchetHistory(id: string, metric?: string): Promise<RatchetSeries>;
  winningChange(id: string): Promise<WinningChangeVM>;
  candidatesWithResults(id: string): Promise<CandidateVM[]>;
  spendBreakdown(id: string): Promise<SpendVM>;
  recentActivity(opts?: { optimizationId?: string; limit?: number }): Promise<ActivityItem[]>;
  agentEvents(agentId: string, cursor?: number, limit?: number): Promise<AgentEventsPage>;
  /** Parse ratchet.yaml text (read-only; the repo owns the file). */
  validateConfig(text: string): Promise<ValidateConfigResult>;
  /** "What's missing" for a run: the short list on the New optimization page. */
  preflight(input: PreflightInput): Promise<PreflightItem[]>;
  getSettings(): Promise<SettingsVM>;
  environments(): Promise<EnvironmentsVM>;
  // GitHub
  githubStatus(): Promise<GithubStatusVM>;
  /** Start the device flow: returns the code to show; the main process polls and stores the token. */
  githubSignIn(): Promise<GithubStatusVM>;
  githubSignOut(): Promise<GithubStatusVM>;
  listRepos(query?: string): Promise<RepoVM[]>;
  inspectRepo(source: RepoSourceInput): Promise<RepoInspectVM>;
  /** Native folder dialog; null when cancelled. */
  pickLocalFolder(): Promise<string | null>;
  // AWS and agents
  awsProfiles(): Promise<AwsProfilesVM>;
  detectHarnesses(): Promise<HarnessVM[]>;
  /** Probe the machine again ("Re-detect"). */
  redetectHarnesses(): Promise<HarnessVM[]>;
  // templates
  listTemplates(): Promise<TemplateVM[]>;
  /** "Use template" / "Add via pull request": commit the template's files on a branch and open the PR. */
  proposeTemplate(input: ProposeTemplateInput): Promise<ProposeTemplateVM>;
  // commands (mirror EngineApi)
  createOptimization(input: CreateOptimizationInput): Promise<string>;
  startOptimization(id: string): Promise<void>;
  stopOptimization(id: string, reason?: string): Promise<void>;
  pauseAgent(agentId: string): Promise<void>;
  resumeAgent(agentId: string): Promise<void>;
  messageAgent(agentId: string, text: string): Promise<void>;
  openPullRequest(id: string): Promise<{ url: string }>;
  exportReport(id: string): Promise<ExportReportResult>;
  /** "End sandbox now": tear the environment down without waiting for the idle window. Refused while a run is active. */
  endSandbox(sandboxId: string): Promise<EnvironmentsVM>;
  // settings
  setAwsProfile(profile: string): Promise<SettingsVM>;
  setSecret(name: SecretName, value: string): Promise<SettingsVM>;
  clearSecret(name: SecretName): Promise<SettingsVM>;
  /** Region and the development overrides; validated as a whole, nothing is saved if any field is invalid. */
  saveLiveSettings(input: LiveSettingsInput): Promise<SettingsVM>;
  /** Read-only probe (STS, GitHub, a model API); secrets are read in the main process. */
  testConnection(target: ConnectionTarget): Promise<ConnectionResultVM>;
  // events
  onEvent(listener: (e: EngineEventVM) => void): () => void;
}
