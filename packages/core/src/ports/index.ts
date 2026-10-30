/**
 * Ports: every external system Ratchet talks to, as an interface.
 * Each port has a real implementation (AWS, GitHub, agent CLIs, EC2 bench)
 * and a mock / local one, so the whole product runs in `simulation` mode.
 * See docs/REAL_ENV_TESTING.md for which mocks must be re-tested for real.
 */
import type { RatchetConfig } from '../config/schema.js';
import type { GuardrailResult } from '../config/guardrails.js';
import type { AgentKind, CandidateVerdict, SandboxProviderKind, SandboxStatus, SpendCategory } from '../ledger/schema.js';

export type OptimizationId = string;
export type AgentId = string;
export type { AgentKind };
export type Mode = 'live' | 'simulation';

export interface Clock {
  now(): number;
  sleep(ms: number, signal?: AbortSignal): Promise<void>;
}

// ---------------------------------------------------------------- sandbox (OpenTofu)

export interface SandboxInput {
  sandboxId: string;
  region: string;
  allowedInstanceTypes: string[];
  maxInstancesPerAgent: number;
  maxInstanceLifetimeHours: number;
  tags?: Record<string, string>;
}

/** Mirrors infra/contract.md outputs. */
export interface SandboxOutputs {
  artifactsBucket: string;
  sealedBucket: string;
  agentBoundaryRef: string;
  evalIdentityRef: string;
  agentRolePath: string;
  network: { subnetId: string; agentRunnerSecurityGroupId: string; benchSecurityGroupId: string };
}

export interface PlanChange {
  action: 'create' | 'update' | 'delete' | 'replace' | 'no-op';
  address: string;
  type: string;
}

export interface SandboxPlan {
  changes: PlanChange[];
  summary: string;
}

export interface SandboxProvider {
  plan(input: SandboxInput): Promise<SandboxPlan>;
  apply(input: SandboxInput): Promise<SandboxOutputs>;
  destroy(input: SandboxInput): Promise<void>;
  outputs(sandboxId: string): Promise<SandboxOutputs | null>;
}

// ---------------------------------------------------------------- cloud compute + identity

export interface AgentIdentity {
  optimizationId: OptimizationId;
  agentId: AgentId;
  roleRef: string;
  instanceProfileRef: string;
}

export interface SecretRef {
  name: string; // e.g. ANTHROPIC_API_KEY
  ref: string; // e.g. SSM parameter name /ratchet/<opt>/<agent>/ANTHROPIC_API_KEY
}

export type InstanceRole = 'agent_runner' | 'experiment' | 'bench';

export interface InstanceInfo {
  id: string;
  role: InstanceRole;
  optimizationId: OptimizationId;
  agentId?: AgentId;
  instanceType: string;
  state: 'pending' | 'running' | 'stopping' | 'stopped' | 'terminated';
  launchedAt: number;
  terminatedAt?: number;
}

export interface RunnerLaunchSpec {
  optimizationId: OptimizationId;
  agentId: AgentId;
  kind: AgentKind;
  instanceType: string;
  imageRef: string;
  identity: AgentIdentity;
  secrets: SecretRef[];
  repoUrl: string;
  branch: string;
}

export interface BenchLaunchSpec {
  optimizationId: OptimizationId;
  instanceType: string;
  imageRef: string;
  /**
   * `deps` is the dependency-snapshot job: it runs on an instance that HAS egress (the agent subnet, through the
   * proxy when enabled) under its own minimal identity, and never executes candidate code.
   */
  purpose: 'baseline' | 'bench' | 'confirm' | 'deps';
}

export interface RemoteCommand {
  script: string; // shell script run on the instance (SSM RunShellScript)
  env?: Record<string, string>;
  timeoutSeconds?: number;
}

export interface RemoteCommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface CloudCompute {
  createAgentIdentity(optimizationId: OptimizationId, agentId: AgentId): Promise<AgentIdentity>;
  /** Kill switch: attach deny-all. Must take effect immediately. */
  revokeAgentIdentity(identity: AgentIdentity): Promise<void>;
  deleteAgentIdentity(identity: AgentIdentity): Promise<void>;
  putSecret(optimizationId: OptimizationId, agentId: AgentId, name: string, value: string): Promise<SecretRef>;
  deleteSecrets(optimizationId: OptimizationId, agentId: AgentId): Promise<void>;
  launchRunner(spec: RunnerLaunchSpec): Promise<InstanceInfo>;
  launchBenchInstance(spec: BenchLaunchSpec): Promise<InstanceInfo>;
  terminate(instanceIds: string[]): Promise<void>;
  /** Every instance carrying the ratchet tags (optionally for one optimization). */
  listTagged(optimizationId?: OptimizationId): Promise<InstanceInfo[]>;
  runCommand(instanceId: string, command: RemoteCommand): Promise<RemoteCommandResult>;
}

export interface PriceBook {
  /** On-demand USD per hour for an instance type in the sandbox region. */
  hourlyUsd(instanceType: string): Promise<number>;
}

export interface ArtifactStore {
  put(key: string, body: string | Uint8Array): Promise<void>;
  get(key: string): Promise<string | null>;
  list(prefix: string): Promise<string[]>;
}

// ---------------------------------------------------------------- cost reconciliation

/** What to ask the billing system about: everything tagged to one optimization within a time window. */
export interface CostActualsQuery {
  optimizationId: OptimizationId;
  /** Window start (epoch ms); implementations round down to their billing granularity (UTC day). */
  startMs: number;
  /** Window end (epoch ms); implementations round up to their billing granularity. */
  endMs: number;
}

/** Billed cost of the tagged resources, mapped to ledger categories. */
export interface CostActuals {
  /** USD per ledger category (a category missing from the billing data is absent, not 0). */
  byCategory: Partial<Record<SpendCategory, number>>;
  /** Number of non-empty billing groups that came back; 0 means no billing data at all (not available yet, or tags not activated). */
  groups: number;
  /** False while the billing system flags the window as still being estimated / incomplete. */
  complete: boolean;
  /** Why `complete` is false, or any caveat worth showing the user. */
  note?: string;
}

/**
 * Billing truth for the cost meter's estimate (docs/V0_ARCHITECTURE.md §7 "Reconcile"). The AWS
 * implementation queries Cost Explorer by cost-allocation tag (~24 h lag); the mock returns canned
 * numbers. `reconcileSpend` (core) turns the difference into ledger correction rows.
 */
export interface CostReconciler {
  actuals(query: CostActualsQuery): Promise<CostActuals>;
}

/** One category of a reconcile run. */
export interface CostReconcileLine {
  category: SpendCategory;
  /** Ledger total for the category before this run (estimates plus earlier corrections). */
  ledgerUsd: number;
  /** Billed amount for the category. */
  actualUsd: number;
  /** `actualUsd - ledgerUsd`; the correction row's amount when `written`. */
  deltaUsd: number;
  written: boolean;
}

/**
 * - `reconciled`: at least one correction row was written.
 * - `unchanged`: billing data is complete and already matches the ledger (re-runs land here).
 * - `pending`: billing data is incomplete or missing; nothing was written. Try again after ~24 h.
 */
export type CostReconcileStatus = 'reconciled' | 'unchanged' | 'pending';

export interface CostReconcileReport {
  optimizationId: OptimizationId;
  at: number;
  status: CostReconcileStatus;
  /** Set when `status` is `pending`. */
  reason?: string;
  window: { startMs: number; endMs: number };
  lines: CostReconcileLine[];
  /** Sum of the correction rows written by this run. */
  writtenUsd: number;
}

// ---------------------------------------------------------------- git

export interface CandidateCommit {
  sha: string;
  branch: string;
  message: string;
  committedAt: number;
}

/** One file of a diff between two commits (renames are reported as a delete plus an add). */
export interface FileDiff {
  path: string;
  additions: number;
  deletions: number;
  /** True for binary files: no line counts, no patch. */
  binary: boolean;
  /** This file's unified diff (`@@` hunks only, no `diff --git` / index headers); empty for binary files. */
  patch: string;
}

export interface GitHost {
  /** Create `branch` at `fromRef` in the remote. */
  createBranch(fromRef: string, branch: string): Promise<void>;
  /** Commits on `branch` carrying the `Ratchet-Candidate: true` trailer, oldest first, after `sinceSha`. */
  listCandidates(branch: string, sinceSha?: string): Promise<CandidateCommit[]>;
  changedFiles(baseRef: string, sha: string): Promise<string[]>;
  /** Per-file diff of `sha` against its merge base with `baseRef` (what a PR from `sha` into `baseRef` would show). */
  diff(baseRef: string, sha: string): Promise<FileDiff[]>;
  /** Materialize `sha` (code) into `dir`; used by the bench runner. */
  checkout(sha: string, dir: string): Promise<void>;
  /** Squash the verified `shas` (ratchet order; last one's tree wins) onto `base` as branch `head` and open a PR. */
  openPullRequest(input: { base: string; head: string; title: string; body: string; shas: string[] }): Promise<{ url: string }>;
  /** Resolve a ref (branch, tag, sha) to a full commit sha. */
  resolveRef(ref: string): Promise<string>;
  remoteUrl(): string;
}

// ---------------------------------------------------------------- agents

export interface AgentUsage {
  inputTokens: number;
  outputTokens: number;
  costUsd?: number;
}

/**
 * Conventions: `usage` is incremental (tokens/cost for this event only, not cumulative).
 * `status` events carry `data.state` in 'running' | 'paused' | 'done' | 'failed'.
 */
export interface AgentEvent {
  at: number;
  type: 'status' | 'message' | 'tool' | 'commit' | 'usage' | 'error' | 'verdict';
  text?: string;
  data?: Record<string, unknown>;
  usage?: AgentUsage;
}

export interface AgentSpec {
  workdir: string;
  prompt: string;
  model?: string;
  env?: Record<string, string>;
  maxTurns?: number;
}

export interface AgentSession {
  id: string;
  kind: AgentKind;
}

/** Runs inside `ratchet runner` on an agent runner (or locally in simulation). */
export interface AgentAdapter {
  readonly kind: AgentKind;
  preflight(): Promise<{ ok: boolean; problems: string[] }>;
  /** Run one agent turn to completion, streaming events. */
  start(spec: AgentSpec, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<AgentSession>;
  /** Continue the same session with a new message; resolves when the turn ends. */
  resume(session: AgentSession, spec: AgentSpec, message: string, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<void>;
}

// ---------------------------------------------------------------- runner control (engine ↔ runner loop)

export type RunnerMessage =
  | { type: 'verdict'; sha: string; verdict: CandidateVerdict; reasons: string[]; currentBest: number }
  /** The shared ratchet moved (possibly by another agent): rebase onto `sha` from `branch`. */
  | { type: 'ratchet'; sha: string; branch: string; value: number }
  | { type: 'user'; text: string }
  | { type: 'pause' }
  | { type: 'resume' }
  | { type: 'stop' };

export interface RunnerConfig {
  optimizationId: OptimizationId;
  agentId: AgentId;
  kind: AgentKind;
  repoUrl: string;
  branch: string;
  baseRef: string;
  config: RatchetConfig;
  brief: string; // RATCHET.md contents
  baselineValue: number;
  budgetUsd: number;
  deadlineAt: number;
}

export interface RunnerHandle {
  optimizationId: OptimizationId;
  agentId: AgentId;
  instanceId: string;
}

export interface RunnerControl {
  start(handle: RunnerHandle, config: RunnerConfig): Promise<void>;
  send(handle: RunnerHandle, message: RunnerMessage): Promise<void>;
  /** Events since `cursor` (opaque); returns the next cursor. */
  poll(handle: RunnerHandle, cursor?: string): Promise<{ events: AgentEvent[]; cursor: string }>;
  stop(handle: RunnerHandle): Promise<void>;
  /**
   * Free whatever the control holds on this machine for an optimization (local worktrees and clones). Called by the
   * engine's `teardown`. Optional: the SSM control keeps nothing locally.
   */
  release?(optimizationId: OptimizationId): Promise<void>;
}

/** Where an optimization's agents run: this machine (their own CLI login, a git worktree each) or cloud runners. */
export type AgentsWhere = 'local' | 'cloud';

// ---------------------------------------------------------------- bench

export interface BenchJob {
  optimizationId: OptimizationId;
  candidateId?: string;
  sha: string;
  phase: 'baseline' | 'bench' | 'confirm';
  repeats: number;
  /** For confirm: the hidden trace reference (S3 key or local path). */
  trace?: string;
}

export interface BenchRun {
  value: number; // the objective metric for this run
  metrics: Record<string, number>; // flattened numeric output, e.g. http_req_duration.p95, error_rate, peak_rss_mb
  durationMs: number;
}

export interface BenchOutcome {
  runs: BenchRun[];
  samples: number[]; // runs.map(r => r.value)
  guardrails: GuardrailResult[];
  logsKey?: string;
  startedAt: number;
  finishedAt: number;
}

export interface BenchRunner {
  /**
   * Check out `job.sha` (bench code from `config.bench.ref`), run command guardrails,
   * then the benchmark `job.repeats` times, then threshold guardrails against `baseline`.
   */
  run(job: BenchJob, config: RatchetConfig, baseline: { metrics: Record<string, number> } | null, signal?: AbortSignal): Promise<BenchOutcome>;
}

// ---------------------------------------------------------------- engine API (desktop ↔ core over IPC)

export interface EngineCreateOptimizationInput {
  name?: string;
  repoUrl: string;
  baseRef: string;
  configPath?: string; // path inside repo, default ratchet.yaml
  agents: AgentKind[];
  budgetUsd: number;
  timeLimitSeconds: number;
  plateauEvals: number;
  /**
   * The sandbox this optimization runs in. Live mode requires an applied sandbox and links it
   * (`optimizations.sandbox_id`); in simulation an applied one is linked, otherwise the field stays null.
   */
  sandboxId?: string;
  /**
   * Where the agents run. Default: the engine's `agentsWhere` setting, else `local` when the engine has a local runner
   * control, else `cloud`. The evaluator always runs in the cloud sandbox.
   */
  agentsWhere?: AgentsWhere;
}

/** What the ledger knows about one sandbox, plus the optimizations linked to it. */
export interface SandboxState {
  sandboxId: string;
  /** `absent` = never planned or applied. */
  status: SandboxStatus | 'absent';
  provider: SandboxProviderKind | null;
  region: string | null;
  accountId: string | null;
  /** Contract outputs, from the ledger (set on apply, cleared on destroy). */
  outputs: SandboxOutputs | null;
  appliedAt: number | null;
  destroyedAt: number | null;
  /** Why the last apply / destroy failed (this process only). */
  lastError: string | null;
  optimizations: Array<{ id: string; status: string }>;
}

export type EngineEvent =
  | { type: 'optimization'; optimizationId: OptimizationId; status: string }
  | { type: 'agent-event'; optimizationId: OptimizationId; agentId: AgentId; event: AgentEvent }
  | { type: 'candidate'; optimizationId: OptimizationId; candidateId: string; verdict: CandidateVerdict }
  | { type: 'ratchet'; optimizationId: OptimizationId; from: number; to: number }
  | { type: 'spend'; optimizationId: OptimizationId; totalUsd: number }
  | { type: 'notice'; optimizationId: OptimizationId; level: 'info' | 'warning' | 'error'; text: string }
  /**
   * True while any optimization is starting, baselining, running or stopping (false again once none is). The desktop main
   * process turns this into `powerSaveBlocker` start/stop so the machine does not sleep under a live run.
   */
  | { type: 'run-active'; active: boolean; optimizationIds: OptimizationId[] }
  /** The automatic sandbox changing phase (docs/SIMPLIFIED_MODEL.md, "Sandbox lifecycle: invisible"). */
  | { type: 'sandbox'; sandboxId: string; phase: SandboxPhase; region: string | null; text: string };

/** `creating` -> `ready` -> (`idle` while no run is active) -> `destroying` -> `destroyed`; `failed` when a step failed. */
export type SandboxPhase = 'creating' | 'ready' | 'idle' | 'destroying' | 'destroyed' | 'failed';

/**
 * What the engine records when it tears the sandbox down (automatically after the idle window, or on demand):
 * what was removed, what it cost, and proof that nothing is left running. Shown on the Result screen and Environments.
 */
export interface SandboxTeardownReceipt {
  sandboxId: string;
  region: string;
  /** `idle`: the idle window after the last run elapsed; `manual`: `endSandbox`. */
  trigger: 'idle' | 'manual';
  createdAt: number | null;
  destroyedAt: number;
  runs: Array<{ optimizationId: OptimizationId; status: string; spendUsd: number }>;
  /** Estimated spend of every run that used the sandbox, split by category (tokens, compute, ...). */
  spendUsd: number;
  spendByCategory: Record<string, number>;
  removed: {
    /** Instances terminated during teardown (agent runners, bench, experiment). */
    instances: string[];
    agentIdentities: string[];
    secrets: string[];
    /** The sandbox's own resources, named from its outputs (buckets, network, roles). */
    resources: string[];
  };
  /** Verified by `listTagged` after the destroy: nothing tagged is left, so nothing is billing. */
  nothingRunning: boolean;
  orphans: Array<{ id: string; role: string; state: string }>;
  errors: string[];
  /** One plain-words line for the UI. */
  summary: string;
}

/** The invisible sandbox seen from outside: what is running, what it costs, when it goes away, the last receipt. */
export interface SandboxActivity {
  sandboxId: string;
  status: SandboxState['status'];
  region: string | null;
  /** True when the engine created it itself and will destroy it after the idle window. */
  auto: boolean;
  activeRuns: OptimizationId[];
  /** Tagged instances alive now (null: could not be listed). */
  instancesRunning: number | null;
  /** Estimated spend of the runs currently active in it. */
  spendUsd: number;
  idleSince: number | null;
  /** When the automatic destroy is due; null while runs are active or the sandbox is not auto-managed. */
  destroyAt: number | null;
  lastReceipt: SandboxTeardownReceipt | null;
}

export interface EngineApi {
  mode(): Mode;
  createOptimization(input: EngineCreateOptimizationInput): Promise<OptimizationId>;
  start(id: OptimizationId): Promise<void>;
  /** Kill switch: revoke identities, stop agents, terminate instances. */
  stop(id: OptimizationId, reason?: string): Promise<void>;
  pauseAgent(agentId: AgentId): Promise<void>;
  resumeAgent(agentId: AgentId): Promise<void>;
  messageAgent(agentId: AgentId, text: string): Promise<void>;
  openPullRequest(id: OptimizationId): Promise<{ url: string }>;
  teardown(id: OptimizationId): Promise<void>;
  /**
   * Compare the ledger's estimated infrastructure spend with the billing data (Cost Explorer) and
   * record correction rows. Idempotent; reports `pending` while billing data lags. Optional so
   * existing implementations keep compiling.
   */
  reconcileCosts?(id: OptimizationId): Promise<CostReconcileReport>;
  /** Sandbox lifecycle: planned -> applying -> applied -> destroying -> destroyed / failed, recorded in the ledger. */
  sandboxState(sandboxId: string): Promise<SandboxState>;
  planSandbox(input: SandboxInput): Promise<SandboxPlan>;
  applySandbox(input: SandboxInput): Promise<SandboxOutputs>;
  /** Refuses while an optimization in the sandbox is running, or while tagged instances / agent identities remain. */
  destroySandbox(sandboxId: string): Promise<void>;
  subscribe(listener: (e: EngineEvent) => void): () => void;
  /**
   * The receipt of the sandbox teardown that covered this optimization (null while the sandbox is still alive or the
   * run never used one). Optional so existing implementations keep compiling.
   */
  teardownReceipt?(optimizationId: OptimizationId): Promise<SandboxTeardownReceipt | null>;
  /** Live view of the automatic sandbox (running instances, spend, destroy time, last receipt). */
  sandboxActivity?(sandboxId: string): Promise<SandboxActivity>;
  /** "End the environment now": tear the sandbox down without waiting for the idle window. Refuses while a run is active. */
  endSandbox?(sandboxId: string): Promise<SandboxTeardownReceipt>;
  /** True while any run is active (the same signal as the `run-active` event), for the sleep-prevention hook. */
  runActive?(): boolean;
}

// ---------------------------------------------------------------- runner loop (`ratchet runner`)

/** The inbox the runner loop reads engine messages from (SSM/S3 in live mode, in-memory in simulation). */
export interface RunnerInbox {
  /** Resolves with the next message, or null when `signal` aborts. */
  next(signal?: AbortSignal): Promise<RunnerMessage | null>;
}

/**
 * What `runRunnerLoop` (in @ratchet/agents) needs. The loop owns one agent on one branch:
 * prompt → agent turn → commit with `Ratchet-Candidate: true` trailer → push → wait for verdict
 * → rebase onto the ratchet best when it moved → resume the agent with the verdict → repeat,
 * until stop / deadline / budget.
 */
export interface RunnerLoopOptions {
  config: RunnerConfig;
  adapter: AgentAdapter;
  /** A git clone of `config.repoUrl` checked out on `config.branch`, with push access. */
  workdir: string;
  inbox: RunnerInbox;
  emit(event: AgentEvent): void;
  clock: Clock;
  signal?: AbortSignal;
}
