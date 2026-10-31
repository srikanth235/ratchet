import { randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join, normalize } from 'node:path';
import {
  AGENT_KINDS,
  AGENT_STATUSES,
  CALIBRATION,
  buildTemplateProposal,
  calibrate,
  effectiveProtected,
  environmentModuleOf,
  evaluateRatchet,
  listTemplates,
  reconcileSpend,
  findProtectedViolations,
  formatConfigError,
  getTemplate,
  guardrailName,
  median,
  noiseFloor as computeNoiseFloor,
  parseConfig,
  type AgentEvent,
  type AgentIdentity,
  type AgentsWhere,
  type AgentKind,
  type AgentRow,
  type AgentStatus,
  type BenchOutcome,
  type Calibration,
  type ApproveEnvironmentOptions,
  type CandidateWithResults,
  type Clock,
  type CostReconcileReport,
  type DraftEnvironmentInput,
  type EffectiveProtected,
  type EnvironmentDraftRecord,
  type EngineApi,
  type EngineCreateOptimizationInput,
  type EngineEvent,
  type GitHost,
  type Ledger,
  type Mode,
  type NoiseFloorOptions,
  type OptimizationRow,
  type ProposeTemplateInput,
  type ProposeTemplateResult,
  type RatchetConfig,
  type RatchetOutcome,
  type RunnerControl,
  type RunnerHandle,
  type RunnerMessage,
  type SandboxActivity,
  type SandboxEnvironment,
  type SandboxInput,
  type SandboxOutputs,
  type SandboxPlan,
  type SandboxState,
  type SandboxTeardownReceipt,
  type SecretRef,
  type TemplateInfo,
  SandboxLifecycle,
  SandboxLifecycleError,
} from '@ratchet/core';
import { metricsFromStats, samplesOf, statsOf } from './bench-stats.js';
import { readEnvironment } from './environment-files.js';
import { CostMeter } from './cost.js';
import { tokenCostUsd, type Deps } from './deps.js';
import { assertValidId, EngineError } from './errors.js';
import { K, KvStore } from './kv.js';
import { reap } from './reaper.js';
import { AutoSandbox } from './sandbox-auto.js';
import { defaultReport } from './report.js';
import { resolveSettings, type EngineSettings, type ResolvedSettings } from './settings.js';
import type { EnginePorts } from './ports.js';

export interface EngineOptions {
  ledger: Ledger;
  mode: Mode;
  clock: Clock;
  ports: EnginePorts;
  settings?: EngineSettings;
}

/** `createOptimization` input plus an optional caller-chosen id (validated: `[a-z0-9-]`). */
export type CreateOptimizationInput = EngineCreateOptimizationInput & { id?: string };

export interface TeardownReport {
  optimizationId: string;
  at: number;
  deletedSecretsFor: string[];
  deletedIdentitiesFor: string[];
  terminatedInstanceIds: string[];
  orphans: Array<{ id: string; role: string; state: string }>;
  errors: string[];
  /** Best-effort cost reconcile at teardown: `pending` is normal (Cost Explorer lags up to 24 h); re-run `reconcileCosts` later. */
  costReconcile?: { status: CostReconcileReport['status']; reason?: string; writtenUsd: number } | { status: 'error'; reason: string };
}

export interface ResumeReport {
  /** Optimizations the engine picked up again (running, stopping, or interrupted while baselining). */
  attached: string[];
}

interface BenchJobState {
  candidateId: string;
  phase: 'bench' | 'confirm';
  result?: { ok: true; outcome: BenchOutcome } | { ok: false; err: unknown };
  promise: Promise<void>;
}

interface Runtime {
  /** Set synchronously when a kill switch starts, so a concurrent tick bails out at its next checkpoint. */
  killing: boolean;
  abort: AbortController;
  job?: BenchJobState;
  attempts: Map<string, number>;
  pollFailures: Map<string, number>;
  lastServedAgent?: string;
  lastSpendEmitted?: number;
}

const LIVE_AGENT = new Set<AgentStatus>(['launching', 'running', 'paused']);
const TERMINAL_AGENT = new Set<AgentStatus>(['stopped', 'done', 'failed']);

/**
 * The Ratchet engine: creates an optimization, runs the baseline, launches agents, then drives the
 * control loop (`tick`) until a stop rule or the user ends it. All durable state is in the ledger
 * (plus a small `engine_kv` table in the same SQLite file), so a new Engine on the same ledger
 * resumes where the old one died.
 *
 * Tick model: `tick()` is one pass over every active optimization and never blocks on a bench job.
 * A bench job is started in one tick and its outcome is collected in a later one. `settle()` awaits
 * running bench jobs (for tests); `run()` is `tick()` + `clock.sleep(tickIntervalMs)` in a loop.
 */
export class Engine implements EngineApi {
  private readonly ledger: Ledger;
  private readonly kv: KvStore;
  private readonly clock: Clock;
  private readonly ports: EnginePorts;
  private readonly settings: ResolvedSettings;
  private readonly modeValue: Mode;
  private readonly deps: Deps;
  private readonly meter: CostMeter;
  private readonly sandboxes: SandboxLifecycle;
  private readonly autoSandbox: AutoSandbox;
  /** Optimizations whose `start()` is in flight (a sandbox may be created for them before their status changes). */
  private readonly pendingStarts = new Set<string>();
  private runActiveFlag = false;

  private readonly listeners = new Set<(e: EngineEvent) => void>();
  private readonly rt = new Map<string, Runtime>();
  private readonly starting = new Map<string, Promise<void>>();
  private readonly configCache = new Map<string, RatchetConfig>();
  private tickChain: Promise<void> = Promise.resolve();
  private runLoop?: { abort: AbortController; done: Promise<void> };
  private readonly resumed: Promise<ResumeReport>;

  /** Swallowed non-fatal errors (poll failures, price lookups, ...), newest last, capped. */
  readonly errors: Array<{ at: number; context: string; message: string }> = [];

  constructor(opts: EngineOptions) {
    this.ledger = opts.ledger;
    this.clock = opts.clock;
    this.ports = opts.ports;
    this.modeValue = opts.mode;
    this.settings = resolveSettings(opts.settings);
    this.kv = new KvStore(this.ledger);
    this.deps = {
      ledger: this.ledger,
      kv: this.kv,
      clock: this.clock,
      compute: this.ports.compute,
      prices: this.ports.prices,
      settings: this.settings,
      reportError: (err, context) => this.reportError(err, context),
    };
    this.meter = new CostMeter(this.deps);
    this.sandboxes = new SandboxLifecycle({
      ledger: this.ledger,
      provider: this.ports.sandbox,
      kind: this.modeValue === 'live' ? 'aws' : 'mock',
      blockers: (id, opts) => this.sandboxBlockers(id, opts),
      inputs: {
        get: (id) => this.kv.getJson<SandboxInput>(K.sandboxInput(id)),
        set: (id, input) => this.kv.setJson(K.sandboxInput(id), input),
      },
    });
    this.autoSandbox = new AutoSandbox({
      ledger: this.ledger,
      kv: this.kv,
      clock: this.clock,
      mode: this.modeValue,
      settings: this.settings,
      lifecycle: this.sandboxes,
      provider: this.ports.sandbox,
      compute: this.ports.compute,
      configOf: (id) => this.config(id),
      environmentOf: (id) => this.environmentOf(id),
      teardown: (id) => this.teardown(id),
      teardownReport: (id) => this.teardownReport(id),
      lifecycleCall: (fn) => this.sandboxCall(fn),
      hasPendingStarts: () => this.pendingStarts.size > 0,
      emit: (e) => this.emit(e),
      reportError: (err, context) => this.reportError(err, context),
    });
    // An apply / destroy that a crash cut short left unknown infrastructure: surface it as `failed`.
    for (const id of this.sandboxes.recover()) this.reportError(new Error('interrupted by a restart'), `sandbox ${id}`);
    this.resumed = this.resume()
      .catch((err) => {
        this.reportError(err, 'resume');
        return { attached: [] };
      })
      .then((r) => {
        this.syncRunActive();
        return r;
      });
  }

  // ------------------------------------------------------------------ small public surface

  mode(): Mode {
    return this.modeValue;
  }

  /** Resolves once the constructor's `resume()` pass has re-attached to running optimizations. */
  ready(): Promise<ResumeReport> {
    return this.resumed;
  }

  /**
   * True while any optimization is starting, baselining, running or stopping: the machine should not sleep. The same signal
   * is emitted as `{ type: 'run-active' }` whenever it flips.
   */
  runActive(): boolean {
    return this.runActiveFlag;
  }

  /** The receipt of the sandbox teardown that covered the optimization, or null while its sandbox is still alive. */
  async teardownReceipt(optimizationId: string): Promise<SandboxTeardownReceipt | null> {
    assertValidId('optimization id', optimizationId);
    await this.resumed;
    return this.autoSandbox.receiptFor(optimizationId);
  }

  /** The automatic sandbox seen from outside: running instances, spend of active runs, when it will be removed, last receipt. */
  async sandboxActivity(sandboxId: string): Promise<SandboxActivity> {
    await this.resumed;
    return this.autoSandbox.activity(sandboxId);
  }

  /**
   * End the sandbox now instead of after the idle window: tears down the finished runs' cloud resources, destroys the sandbox,
   * verifies nothing is left and returns the receipt. Refuses (`sandbox_in_use`) while a run is active.
   */
  async endSandbox(sandboxId: string): Promise<SandboxTeardownReceipt> {
    await this.resumed;
    return this.autoSandbox.destroy(sandboxId, 'manual');
  }

  subscribe(listener: (e: EngineEvent) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Why the optimization stopped, once it has. */
  stopReason(optimizationId: string): string | undefined {
    return this.kv.get(K.stopReason(optimizationId));
  }

  teardownReport(optimizationId: string): TeardownReport | undefined {
    return this.kv.getJson<TeardownReport>(K.teardown(optimizationId));
  }

  /** The report of the latest `reconcileCosts` run for the optimization. */
  costReconcileReport(optimizationId: string): CostReconcileReport | undefined {
    return this.kv.getJson<CostReconcileReport>(K.reconcile(optimizationId));
  }

  // ------------------------------------------------------------------ sandbox lifecycle

  async sandboxState(sandboxId: string): Promise<SandboxState> {
    return this.sandboxes.state(sandboxId);
  }

  /** `tofu plan` (or the mock's): what an apply would change. Records the sandbox as `planned` the first time. */
  planSandbox(input: SandboxInput): Promise<SandboxPlan> {
    return this.sandboxCall(() => this.sandboxes.plan(input));
  }

  /** planned -> applying -> applied (or failed); the contract outputs are stored in the ledger. */
  applySandbox(input: SandboxInput): Promise<SandboxOutputs> {
    this.autoSandbox.release(input.sandboxId); // an explicit apply takes it out of automatic management
    return this.sandboxCall(() => this.sandboxes.apply(input));
  }

  /**
   * applied -> destroying -> destroyed. Refused while an optimization in the sandbox is baselining, running or
   * stopping, while agent identities of its optimizations were not torn down, and while any tagged instance is alive.
   */
  destroySandbox(sandboxId: string): Promise<void> {
    return this.sandboxCall(async () => {
      await this.sandboxes.destroy(sandboxId);
      this.autoSandbox.release(sandboxId);
    });
  }

  private async sandboxCall<T>(fn: () => Promise<T>): Promise<T> {
    try {
      return await fn();
    } catch (err) {
      if (err instanceof SandboxLifecycleError) throw new EngineError(err.code, err.message, err.details);
      throw err;
    }
  }

  /** The teardown guard for `destroySandbox`: the same orphan check `teardown()` ends with, plus leftover identities. */
  private async sandboxBlockers(_sandboxId: string, optimizations: OptimizationRow[]): Promise<string[]> {
    const reasons: string[] = [];
    for (const o of optimizations) {
      const left = this.ledger.listAgents(o.id).filter((a) => this.kv.has(K.identity(a.id)));
      if (left.length > 0) reasons.push(`optimization ${o.id} still has ${left.length} agent identit${left.length === 1 ? 'y' : 'ies'}: run its teardown first`);
    }
    try {
      const alive = (await this.ports.compute.listTagged()).filter((i) => i.state !== 'terminated');
      if (alive.length > 0) reasons.push(`${alive.length} tagged instance(s) still alive (${alive.map((i) => i.id).join(', ')})`);
    } catch (err) {
      // No compute without sandbox outputs: nothing can have been launched. Any other failure blocks (fail closed).
      if (!(err instanceof EngineError && err.code === 'not_configured')) throw err;
    }
    return reasons;
  }

  /**
   * Live optimizations need an applied sandbox and are linked to it. In simulation an applied sandbox is linked
   * when there is one, so the Environments screen and the ledger agree; otherwise the optimization stays unlinked.
   */
  private sandboxForNewOptimization(requested: string | undefined): string | null {
    if (requested !== undefined) assertValidId('sandbox id', requested);
    const row = requested ? this.ledger.getSandbox(requested) : undefined;
    if (this.settings.sandbox.auto) return row ? row.id : null; // created (or re-created) at the first run start
    if (this.modeValue === 'live') {
      this.assertSandboxApplied(requested ?? null);
      return requested!;
    }
    return row?.status === 'applied' ? row.id : null;
  }

  /** Live mode only: an optimization may start only inside an applied sandbox. */
  private assertSandboxApplied(sandboxId: string | null): void {
    if (this.modeValue !== 'live') return;
    if (!sandboxId) {
      throw new EngineError('sandbox_not_applied', 'live optimizations run inside a sandbox: apply one first (Environments)');
    }
    const row = this.ledger.getSandbox(sandboxId);
    if (row?.status !== 'applied') {
      throw new EngineError('sandbox_not_applied', `sandbox ${sandboxId} is ${row?.status ?? 'not set up'}: apply it first (Environments)`);
    }
  }

  // ------------------------------------------------------------------ createOptimization

  async createOptimization(input: CreateOptimizationInput): Promise<string> {
    await this.resumed;
    this.validateCreateInput(input);
    const sandboxId = this.sandboxForNewOptimization(input.sandboxId);
    const id = input.id ?? this.settings.newId?.() ?? `opt-${randomBytes(4).toString('hex')}`;
    assertValidId('optimization id', id);
    if (this.ledger.getOptimization(id)) throw new EngineError('invalid_input', `optimization ${id} already exists`);

    const git = this.gitFor(input.repoUrl);
    const configPath = input.configPath ?? 'ratchet.yaml';
    const { configYaml } = await this.readFromBase(git, input.baseRef, configPath, null);
    const parsed = parseConfig(configYaml);
    if (!parsed.ok) {
      throw new EngineError(
        'invalid_config',
        `${configPath} is invalid:\n${parsed.errors.map((e) => `  - ${formatConfigError(e)}`).join('\n')}`,
        parsed.errors,
      );
    }
    const config = parsed.config;
    // Fail early when the brief is missing, rather than after the baseline has cost money.
    const brief = await this.readFromBase(git, input.baseRef, configPath, config.agent.instructions);
    if (brief.brief === undefined || brief.brief.trim() === '') {
      throw new EngineError('invalid_config', `agent.instructions file ${config.agent.instructions} is missing or empty at ${input.baseRef}`);
    }

    const where = this.resolveAgentsWhere(input.agentsWhere);
    const now = this.clock.now();
    const slots = this.agentSlots(id, input.agents);
    this.ledger.transaction(() => {
      this.ledger.createOptimization({
        id,
        name: input.name ?? null,
        repo: input.repoUrl,
        baseRef: input.baseRef,
        metric: config.objective.metric,
        direction: config.objective.direction,
        // Provisional when ratchet.yaml leaves it out: the calibrated threshold (kv `calibration`) replaces it once the baseline ran.
        minGain: config.objective.min_gain ?? CALIBRATION.fallbackGain,
        budgetUsd: input.budgetUsd,
        timeLimitS: input.timeLimitSeconds,
        configYaml,
        status: 'draft',
        createdAt: now,
        pack: config.pack ?? null,
        objectiveName: config.objective.name ?? null,
        evaluatorKind: config.evaluator.kind,
        value: config.objective.value
          ? { usdPerUnit: config.objective.value.usd_per_unit, period: config.objective.value.period, basis: config.objective.value.basis, unit: config.objective.value.unit ?? null }
          : null,
        sandboxId,
      });
      for (const s of slots) {
        this.ledger.addAgent({ id: s.agentId, optimizationId: id, kind: s.kind, branch: s.branch, status: 'pending', createdAt: now });
      }
      this.kv.set(K.plateau(id), String(input.plateauEvals));
      this.kv.set(K.agentsWhere(id), where);
    });
    this.emit({ type: 'optimization', optimizationId: id, status: 'draft' });
    return id;
  }

  /** Request > engine setting > `local` when a local runner control exists > `cloud`. */
  private resolveAgentsWhere(requested: AgentsWhere | undefined): AgentsWhere {
    const where = requested ?? this.settings.agentsWhere ?? (this.ports.localRunners ? 'local' : 'cloud');
    if (where !== 'local' && where !== 'cloud') throw new EngineError('invalid_input', `agentsWhere must be 'local' or 'cloud'`);
    if (where === 'local' && !this.ports.localRunners) {
      throw new EngineError('invalid_input', 'agents cannot run on this machine here: the engine has no local runner control (use agentsWhere: cloud)');
    }
    return where;
  }

  /** Where this optimization's agents run (optimizations created before the setting existed ran in the cloud). */
  agentsWhere(optimizationId: string): AgentsWhere {
    return this.kv.get(K.agentsWhere(optimizationId)) === 'local' && this.ports.localRunners ? 'local' : 'cloud';
  }

  /** The runner control for an optimization: the local one for local agents, else the cloud (SSM) one. */
  private runnersFor(optimizationId: string): RunnerControl {
    return this.agentsWhere(optimizationId) === 'local' ? this.ports.localRunners! : this.ports.runners;
  }

  private validateCreateInput(input: CreateOptimizationInput): void {
    const bad = (m: string): never => {
      throw new EngineError('invalid_input', m);
    };
    if (input.id !== undefined) assertValidId('optimization id', input.id);
    if (!input.repoUrl || typeof input.repoUrl !== 'string') bad('repoUrl is required');
    if (!input.baseRef || typeof input.baseRef !== 'string') bad('baseRef is required');
    if (!Array.isArray(input.agents) || input.agents.length === 0) bad('at least one agent is required');
    for (const k of input.agents) if (!(AGENT_KINDS as readonly string[]).includes(k)) bad(`unknown agent kind ${JSON.stringify(k)}`);
    if (!Number.isFinite(input.budgetUsd) || input.budgetUsd <= 0) bad('budgetUsd must be a positive number');
    if (!Number.isFinite(input.timeLimitSeconds) || input.timeLimitSeconds <= 0) bad('timeLimitSeconds must be a positive number');
    if (!Number.isInteger(input.plateauEvals) || input.plateauEvals < 1) bad('plateauEvals must be an integer >= 1');
    const p = input.configPath;
    if (p !== undefined && (isAbsolute(p) || normalize(p).startsWith('..'))) bad('configPath must be a path inside the repository');
  }

  /** `<kind>`, `<kind>-2`, ... so two agents of one kind get distinct branches `ratchet/<opt>/<slot>`. */
  private agentSlots(optId: string, kinds: AgentKind[]): Array<{ agentId: string; kind: AgentKind; branch: string }> {
    const seen = new Map<AgentKind, number>();
    return kinds.map((kind) => {
      const n = (seen.get(kind) ?? 0) + 1;
      seen.set(kind, n);
      const slot = n === 1 ? kind : `${kind}-${n}`;
      const agentId = `${optId}-${slot}`;
      assertValidId('agent id', agentId);
      return { agentId, kind, branch: `ratchet/${optId}/${slot}` };
    });
  }

  private gitFor(repoUrl: string): GitHost {
    const g = this.ports.git;
    return typeof g === 'function' ? g(repoUrl) : g;
  }

  /** Check `baseRef` out into a scratch dir and read ratchet.yaml (and optionally the brief). */
  private async readFromBase(
    git: GitHost,
    baseRef: string,
    configPath: string,
    briefPath: string | null,
  ): Promise<{ configYaml: string; brief?: string | undefined }> {
    const scratchRoot = this.settings.scratchDir ?? tmpdir();
    await mkdir(scratchRoot, { recursive: true });
    const dir = await mkdtemp(join(scratchRoot, 'ratchet-engine-'));
    try {
      await git.checkout(baseRef, dir);
      const configYaml = await this.readInside(dir, configPath).catch((err) => {
        throw new EngineError('invalid_config', `cannot read ${configPath} at ${baseRef}: ${(err as Error).message}`);
      });
      let brief: string | undefined;
      if (briefPath !== null) brief = await this.readInside(dir, briefPath).catch(() => undefined);
      return { configYaml, brief };
    } finally {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    }
  }

  private async readInside(dir: string, rel: string): Promise<string> {
    const n = normalize(rel);
    if (isAbsolute(n) || n.startsWith('..')) throw new Error(`path escapes the repository: ${rel}`);
    return readFile(join(dir, n), 'utf8');
  }

  // ------------------------------------------------------------------ start

  async start(id: string): Promise<void> {
    assertValidId('optimization id', id);
    await this.resumed;
    const opt = this.requireOpt(id);
    const resumable = opt.status === 'baselining' && !this.starting.has(id);
    if (opt.status !== 'draft' && !resumable) {
      throw new EngineError('invalid_state', `optimization ${id} is ${opt.status}; only a draft can be started`);
    }
    this.pendingStarts.add(id);
    this.syncRunActive();
    try {
      // The sandbox is created lazily, at the first run start in a region, and reused while runs are active.
      if (!resumable && this.autoSandbox.enabled) await this.autoSandbox.ensureFor(id);
      else if (!resumable && environmentModuleOf(this.config(id))) {
        this.emit({ type: 'notice', optimizationId: id, level: 'warning', text: 'ratchet.yaml declares an environment, but this engine does not manage the sandbox itself (apply it by hand): the environment is not applied.' });
      }
      this.assertSandboxApplied(this.requireOpt(id).sandboxId);
      await this.beginStart(id);
    } finally {
      this.pendingStarts.delete(id);
      this.syncRunActive();
    }
  }

  private beginStart(id: string): Promise<void> {
    const p = this.runStart(id);
    this.starting.set(id, p);
    return p.finally(() => {
      if (this.starting.get(id) === p) this.starting.delete(id);
    });
  }

  private async runStart(id: string): Promise<void> {
    const rt = this.state(id);
    try {
      if (this.requireOpt(id).status === 'draft') this.setStatus(id, 'baselining');
      const config = this.config(id);

      const baselineValue = await this.ensureBaseline(id, config, rt);
      this.assertAlive(id);

      if (this.kv.getNumber(K.deadline(id)) === undefined) {
        this.kv.set(K.deadline(id), String(this.clock.now() + this.requireOpt(id).timeLimitS * 1000));
      }
      const brief = await this.loadBrief(id, config);
      for (const agent of this.ledger.listAgents(id)) {
        this.assertAlive(id);
        await this.launchAgent(id, agent.id, config, brief, baselineValue);
      }
      this.assertAlive(id);
      this.setStatus(id, 'running');
    } catch (err) {
      const status = this.ledger.getOptimization(id)?.status;
      if (rt.killing || (status !== undefined && status !== 'baselining' && status !== 'draft')) return; // stop() took over
      this.reportError(err, `start ${id}`);
      await this.executeKill(id, `start failed: ${(err as Error).message}`, 'failed').catch((e) => this.reportError(e, `cleanup ${id}`));
      throw err;
    }
  }

  private assertAlive(id: string): void {
    const rt = this.state(id);
    const status = this.ledger.getOptimization(id)?.status;
    if (rt.killing || status === 'stopping' || status === 'stopped' || status === 'failed') {
      throw new EngineError('aborted', `optimization ${id} was stopped`);
    }
  }

  private async ensureBaseline(id: string, config: RatchetConfig, rt: Runtime): Promise<number> {
    const opt = this.requireOpt(id);
    let baseline = this.ledger.getBaseline(id);
    if (!baseline) {
      const outcome = await this.ports.bench.run(
        { optimizationId: id, sha: await this.baseSha(opt), phase: 'baseline', repeats: config.bench.repeats ?? CALIBRATION.baselineRepeats },
        config,
        null,
        rt.abort.signal,
      );
      this.assertAlive(id);
      const samples = samplesOf(outcome);
      const stats = statsOf(outcome);
      const commandGuardrails = new Set(config.guardrails.flatMap((g) => (g.kind === 'command' ? [guardrailName(g)] : [])));
      const broken = outcome.guardrails.filter((g) => !g.passed && commandGuardrails.has(g.name));
      if (broken.length > 0) {
        throw new EngineError(
          'invalid_state',
          `baseline fails its own guardrails: ${broken.map((g) => `${g.name}${g.detail ? ` (${g.detail})` : ''}`).join('; ')}`,
        );
      }
      baseline = this.ledger.recordBaseline({
        optimizationId: id,
        samples,
        noiseFloor: computeNoiseFloor(samples, this.noiseOptions(config)),
        stats,
        value: stats.value,
        recordedAt: this.clock.now(),
      });
      const cal = this.calibrationFor(id, config);
      if (cal) this.emit({ type: 'notice', optimizationId: id, level: 'info', text: `Calibrated from the baseline: ${cal.explanation}` });
    }
    if (config.bench.confirm_trace && this.settings.confirmBaseline && !this.kv.has(K.confirmBest(id))) {
      const confirm = await this.ports.bench.run(
        { optimizationId: id, sha: await this.baseSha(opt), phase: 'confirm', repeats: this.repeatsFor(id, config), trace: config.bench.confirm_trace },
        config,
        { metrics: metricsFromStats(baseline.stats) },
        rt.abort.signal,
      );
      this.assertAlive(id);
      this.kv.set(K.confirmBest(id), String(median(samplesOf(confirm))));
    }
    return baseline.value;
  }

  /** `baseRef` resolved to a sha once and remembered, so baseline, confirm-baseline and diffs all use the same commit. */
  private async baseSha(opt: OptimizationRow): Promise<string> {
    const key = K.baseSha(opt.id);
    const known = this.kv.get(key);
    if (known) return known;
    const sha = await this.gitFor(opt.repo).resolveRef(opt.baseRef);
    this.kv.set(key, sha);
    return sha;
  }

  private async loadBrief(id: string, config: RatchetConfig): Promise<string> {
    const opt = this.requireOpt(id);
    const r = await this.readFromBase(this.gitFor(opt.repo), opt.baseRef, 'ratchet.yaml', config.agent.instructions).catch(() => undefined);
    const brief = r?.brief;
    if (brief === undefined || brief.trim() === '') {
      throw new EngineError('invalid_config', `agent brief ${config.agent.instructions} is missing at ${opt.baseRef}`);
    }
    return brief;
  }

  /** Idempotent, resumable launch: `launch_step` in the kv says how far this agent got. */
  private async launchAgent(optId: string, agentId: string, config: RatchetConfig, brief: string, baselineValue: number): Promise<void> {
    const opt = this.requireOpt(optId);
    const agent = this.requireAgent(agentId);
    const { compute } = this.ports;
    const runners = this.runnersFor(optId);
    // Local agents use the user's own CLI login on this machine: no cloud identity, secrets or runner instance.
    const local = this.agentsWhere(optId) === 'local';
    const git = this.gitFor(opt.repo);
    let step = this.kv.getNumber(K.launch(agentId)) ?? 0;
    const done = (n: number): void => {
      step = n;
      this.kv.set(K.launch(agentId), String(n));
    };
    if (step >= 5) return;
    if (agent.status === 'pending') this.ledger.setAgentStatus(agentId, 'launching');

    if (!local && step < 1) {
      const identity = await compute.createAgentIdentity(optId, agentId);
      this.kv.setJson(K.identity(agentId), identity);
      done(1);
    }
    const identity = this.kv.getJson<AgentIdentity>(K.identity(agentId));
    if (!local && !identity) throw new EngineError('invalid_state', `identity for ${agentId} is missing`);

    if (!local && step < 2) {
      const refs: SecretRef[] = [];
      for (const name of this.settings.secretNames[agent.kind]) {
        const value = await this.settings.secrets(agent.kind, name);
        if (value === undefined || value === '') {
          throw new EngineError('missing_secret', `secret ${name} for ${agent.kind} is not available`);
        }
        refs.push(await compute.putSecret(optId, agentId, name, value));
      }
      this.kv.setJson(K.secrets(agentId), refs); // refs only, never values
      done(2);
    }
    const secrets = this.kv.getJson<SecretRef[]>(K.secrets(agentId)) ?? [];

    if (step < 3) {
      await git.createBranch(await this.baseSha(opt), agent.branch);
      done(3);
    }
    if (local && step < 4) {
      this.ledger.setAgentStatus(agentId, 'launching', `local:${agentId}`);
      done(4);
    }
    if (!local && step < 4) {
      const instance = await compute.launchRunner({
        optimizationId: optId,
        agentId,
        kind: agent.kind,
        instanceType: config.agent.allowed_instance_types[0]!,
        imageRef: this.settings.runnerImageRef,
        identity: identity!,
        secrets,
        repoUrl: opt.repo,
        branch: agent.branch,
      });
      this.ledger.setAgentStatus(agentId, 'launching', instance.id);
      done(4);
    }
    if (step < 5) {
      const fresh = this.requireAgent(agentId);
      await runners.start(this.handle(fresh), {
        optimizationId: optId,
        agentId,
        kind: agent.kind,
        repoUrl: opt.repo,
        branch: agent.branch,
        baseRef: opt.baseRef,
        config,
        brief,
        baselineValue,
        ...(this.calibrationFor(optId, config) ? { minGain: this.calibrationFor(optId, config)!.minGain } : {}),
        budgetUsd: opt.budgetUsd,
        deadlineAt: this.kv.getNumber(K.deadline(optId)) ?? this.clock.now() + opt.timeLimitS * 1000,
      });
      this.ledger.setAgentStatus(agentId, 'running');
      done(5);
    }
  }

  // ------------------------------------------------------------------ stop / kill switch

  /** Kill switch. Order matters: revoke identities first, then stop runners, then terminate instances. */
  async stop(id: string, reason = 'stopped by user'): Promise<void> {
    assertValidId('optimization id', id);
    await this.resumed;
    const opt = this.requireOpt(id);
    if (opt.status === 'completed') return;
    if (opt.status === 'stopped' || opt.status === 'failed') {
      // Already ended: just make sure nothing is left alive or unrevoked.
      await Promise.allSettled(this.ledger.listAgents(id).map((a) => this.revokeAgent(a)));
      await this.terminateAll(id);
      await this.safeMeter(id);
      return;
    }
    await this.executeKill(id, reason, 'stopped');
  }

  private async executeKill(id: string, reason: string, finalStatus: 'stopped' | 'failed'): Promise<void> {
    const rt = this.state(id);
    rt.killing = true;
    rt.abort.abort();
    this.kv.set(K.stopMode(id), 'kill');
    this.kv.set(K.stopReason(id), reason);
    const prev = this.requireOpt(id).status;
    if (prev !== 'stopping') this.setStatus(id, 'stopping');

    const agents = this.ledger.listAgents(id);
    // 1. revoke every agent identity: nothing an agent already holds works after this.
    await Promise.allSettled(agents.map((a) => this.revokeAgent(a)));
    // 2. ask the runners to stop.
    await Promise.allSettled(
      agents
        .filter((a) => a.runnerInstanceId && a.status !== 'stopped')
        .map((a) => this.runnersFor(a.optimizationId).stop(this.handle(a)).catch((e) => this.reportError(e, `runners.stop ${a.id}`))),
    );
    // 3. terminate everything tagged for this optimization.
    await this.terminateAll(id);
    for (const a of agents) {
      const cur = this.ledger.getAgent(a.id);
      if (cur && !TERMINAL_AGENT.has(cur.status)) this.ledger.setAgentStatus(a.id, 'stopped');
    }
    await this.safeMeter(id);
    this.setStatus(id, finalStatus);
    rt.job = undefined;
    rt.killing = false;
  }

  private async revokeAgent(agent: AgentRow): Promise<void> {
    if (this.kv.has(K.revoked(agent.id))) return;
    const identity = this.kv.getJson<AgentIdentity>(K.identity(agent.id));
    if (!identity) return;
    try {
      await this.ports.compute.revokeAgentIdentity(identity);
      this.kv.set(K.revoked(agent.id), String(this.clock.now()));
    } catch (err) {
      this.reportError(err, `revoke ${agent.id}`); // retried by the reaper pass
    }
  }

  private async terminateAll(id: string): Promise<string[]> {
    const live = (await this.ports.compute.listTagged(id)).filter((i) => i.state !== 'terminated');
    const ids = live.map((i) => i.id);
    if (ids.length > 0) await this.ports.compute.terminate(ids);
    return ids;
  }

  /** Time limit / plateau / all agents done: stop the agents cleanly and let queued candidates finish. */
  private async beginGraceful(id: string, reason: string): Promise<void> {
    const opt = this.requireOpt(id);
    if (opt.status !== 'running') return;
    this.kv.set(K.stopMode(id), 'graceful');
    this.kv.set(K.stopReason(id), reason);
    this.kv.set(K.stopAt(id), String(this.clock.now()));
    this.setStatus(id, 'stopping');
    const agents = this.ledger.listAgents(id);
    await Promise.allSettled(
      agents
        .filter((a) => a.runnerInstanceId && LIVE_AGENT.has(a.status))
        .map((a) => this.runnersFor(a.optimizationId).stop(this.handle(a)).catch((e) => this.reportError(e, `runners.stop ${a.id}`))),
    );
    // The runners are down: collect what they left behind (events first, then a final git poll of every
    // branch), then close the agents' access. A git poll that fails here is retried on every drain tick.
    await this.drainPoll(opt);
    await Promise.allSettled(this.ledger.listAgents(id).map((a) => this.revokeAgent(a)));
  }

  /**
   * One collection pass while stopping: every runner's last events, then every agent branch, so a
   * commit pushed just before (or while) the runner reported done is never missed. Agents still
   * marked live are closed afterwards. Returns true only when every branch was listed successfully.
   */
  private async drainPoll(opt: OptimizationRow): Promise<boolean> {
    let complete = true;
    for (const a of this.ledger.listAgents(opt.id)) {
      await this.pollRunner(opt, this.requireAgent(a.id), true);
      if (!(await this.pollGit(opt, this.requireAgent(a.id)))) complete = false;
      const after = this.requireAgent(a.id);
      if (!TERMINAL_AGENT.has(after.status)) this.ledger.setAgentStatus(a.id, 'stopped');
    }
    return complete;
  }

  // ------------------------------------------------------------------ agent control

  async pauseAgent(agentId: string): Promise<void> {
    const agent = this.liveAgent(agentId);
    if (agent.status === 'paused') return;
    await this.runnersFor(agent.optimizationId).send(this.handle(agent), { type: 'pause' });
    this.ledger.setAgentStatus(agentId, 'paused');
    this.logAgent(agent, { at: this.clock.now(), type: 'status', text: 'paused by user', data: { state: 'paused' } });
  }

  async resumeAgent(agentId: string): Promise<void> {
    const agent = this.liveAgent(agentId);
    if (agent.status !== 'paused') return;
    await this.runnersFor(agent.optimizationId).send(this.handle(agent), { type: 'resume' });
    this.ledger.setAgentStatus(agentId, 'running');
    this.logAgent(agent, { at: this.clock.now(), type: 'status', text: 'resumed by user', data: { state: 'running' } });
  }

  async messageAgent(agentId: string, text: string): Promise<void> {
    if (typeof text !== 'string' || text.trim() === '') throw new EngineError('invalid_input', 'message must not be empty');
    const agent = this.liveAgent(agentId);
    await this.runnersFor(agent.optimizationId).send(this.handle(agent), { type: 'user', text });
    this.logAgent(agent, { at: this.clock.now(), type: 'message', text, data: { from: 'user' } });
  }

  private liveAgent(agentId: string): AgentRow {
    assertValidId('agent id', agentId);
    const agent = this.ledger.getAgent(agentId);
    if (!agent) throw new EngineError('not_found', `agent ${agentId} not found`);
    const opt = this.requireOpt(agent.optimizationId);
    if (opt.status !== 'running' || !agent.runnerInstanceId || !LIVE_AGENT.has(agent.status)) {
      throw new EngineError('invalid_state', `agent ${agentId} is not running (optimization ${opt.status}, agent ${agent.status})`);
    }
    return agent;
  }

  // ------------------------------------------------------------------ calibration, protected paths

  /** Noise-floor options: the engine setting, with `evaluator.noise_factor` from ratchet.yaml taking precedence for k. */
  private noiseOptions(config: RatchetConfig): NoiseFloorOptions {
    const k = config.bench.noise_factor;
    return k === undefined ? this.settings.noise : { ...this.settings.noise, k };
  }

  /**
   * The calibration of an optimization (repeats per candidate, win threshold), derived from the baseline in the ledger
   * and remembered in the kv. Recomputed from the ledger when the kv entry is missing (a restart between the two writes),
   * so it never disagrees with the baseline. Undefined until the baseline exists.
   */
  private calibrationFor(id: string, config: RatchetConfig): Calibration | undefined {
    const stored = this.kv.getJson<Calibration>(K.calibration(id));
    if (stored) return stored;
    const baseline = this.ledger.getBaseline(id);
    if (!baseline) return undefined;
    const cal = calibrate({
      samples: baseline.samples,
      repeats: config.bench.repeats,
      minGain: config.objective.min_gain,
      noiseFactor: this.noiseOptions(config).k,
    });
    this.kv.setJson(K.calibration(id), cal);
    return cal;
  }

  /** Runs per candidate / confirm: ratchet.yaml's `repeats`, else the calibrated number, else the baseline default. */
  private repeatsFor(id: string, config: RatchetConfig): number {
    return config.bench.repeats ?? this.calibrationFor(id, config)?.repeats ?? CALIBRATION.baselineRepeats;
  }

  async calibration(id: string): Promise<Calibration | null> {
    assertValidId('optimization id', id);
    await this.resumed;
    this.requireOpt(id);
    return this.calibrationFor(id, this.config(id)) ?? null;
  }

  async protectedPaths(id: string): Promise<EffectiveProtected> {
    assertValidId('optimization id', id);
    await this.resumed;
    this.requireOpt(id);
    return effectiveProtected(this.config(id));
  }

  // ------------------------------------------------------------------ dynamic environments

  /**
   * The environment this optimization is measured in (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"), read from a checkout of
   * the PROTECTED ref pinned to one commit for the life of the optimization (`baseSha`): the approved drafted environment under
   * `infra/ratchet/` when `environment.scope` says so, or the custom module `infra:` names. `undefined` for scope `none`.
   * Candidates cannot change it: their copy is never read, and the paths are protected so a candidate touching them is blocked.
   */
  private async environmentOf(id: string): Promise<SandboxEnvironment | undefined> {
    const opt = this.requireOpt(id);
    const config = this.config(id);
    if (!environmentModuleOf(config)) return undefined;
    const git = this.gitFor(opt.repo);
    const commit = await this.baseSha(opt);
    const scratchRoot = this.settings.scratchDir ?? tmpdir();
    await mkdir(scratchRoot, { recursive: true });
    const dir = await mkdtemp(join(scratchRoot, 'ratchet-env-'));
    try {
      await git.checkout(commit, dir);
      return await readEnvironment(dir, config, commit, opt.baseRef);
    } finally {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    }
  }

  private get drafter(): NonNullable<EnginePorts['environment']> {
    const d = this.ports.environment;
    if (!d) throw new EngineError('unavailable', 'this engine has no environment drafter (no setup agent is configured)');
    return d;
  }

  async draftEnvironment(input: DraftEnvironmentInput): Promise<EnvironmentDraftRecord> {
    await this.resumed;
    return this.drafter.draft(input);
  }

  async reviseEnvironment(draftId: string, message: string): Promise<EnvironmentDraftRecord> {
    await this.resumed;
    return this.drafter.revise(draftId, message);
  }

  async approveEnvironment(draftId: string, options?: ApproveEnvironmentOptions): Promise<EnvironmentDraftRecord> {
    await this.resumed;
    return this.drafter.approve(draftId, options);
  }

  async environmentDraft(draftId: string): Promise<EnvironmentDraftRecord | null> {
    return this.drafter.get(draftId) ?? null;
  }

  async discardEnvironmentDraft(draftId: string): Promise<void> {
    this.drafter.discard(draftId);
  }

  // ------------------------------------------------------------------ templates

  async listTemplates(): Promise<TemplateInfo[]> {
    return listTemplates();
  }

  /**
   * "Use template": add a template's files to a repository through a pull request. Nothing in the repo is changed
   * except by the PR: the files go on a new branch (`ratchet/template-<id>`, with a suffix when it is taken), the PR
   * description says what was added and how to run a baseline. Refuses when files of the template already exist on
   * `baseRef` (`overwrite: true` replaces them) and when the git host cannot commit files.
   */
  async proposeTemplate(input: ProposeTemplateInput): Promise<ProposeTemplateResult> {
    await this.resumed;
    const bad = (m: string): never => {
      throw new EngineError('invalid_input', m);
    };
    if (!input.repoUrl || typeof input.repoUrl !== 'string') bad('repoUrl is required');
    if (!input.baseRef || typeof input.baseRef !== 'string') bad('baseRef is required');
    const info = getTemplate(String(input.templateId ?? ''));
    if (!info) bad(`unknown template ${JSON.stringify(input.templateId)}`);
    if (info!.kind === 'demo') bad(`${info!.id} is a demo (${info!.example}); it runs in simulation and adds nothing to a repository`);
    if (input.valueUsd !== undefined && !(Number.isFinite(input.valueUsd) && input.valueUsd > 0)) bad('valueUsd must be a positive number');
    if (input.direction !== undefined && input.direction !== 'minimize' && input.direction !== 'maximize') bad('direction must be "minimize" or "maximize"');

    const proposal = buildTemplateProposal(info!.id, { metric: input.metric, direction: input.direction, valueUsd: input.valueUsd });
    const git = this.gitFor(input.repoUrl);
    if (!git.commitFiles) throw new EngineError('unavailable', 'this git host cannot commit files, so it cannot open a template pull request');

    if (!input.overwrite) {
      const present = await this.pathsPresent(git, input.baseRef, proposal.files.map((f) => f.path));
      if (present.length > 0) {
        throw new EngineError(
          'invalid_state',
          `${present.join(', ')} already exist${present.length === 1 ? 's' : ''} on ${input.baseRef}; the template would replace ${present.length === 1 ? 'it' : 'them'} (pass overwrite to allow that)`,
          { existing: present },
        );
      }
    }

    const commit = { baseRef: input.baseRef, message: proposal.commitMessage, files: proposal.files };
    let branch = proposal.branch;
    let sha: string;
    try {
      ({ sha } = await git.commitFiles({ ...commit, branch }));
    } catch (err) {
      // most likely the branch exists from an earlier proposal: take a fresh name once, then give up
      branch = `${proposal.branch}-${this.clock.now().toString(36)}`;
      try {
        ({ sha } = await git.commitFiles({ ...commit, branch }));
      } catch {
        throw err;
      }
    }
    const pr = await git.openPullRequest({ base: input.baseRef, head: branch, title: proposal.title, body: proposal.body, shas: [] });
    return { prUrl: pr.url, branch, sha, templateId: info!.id, files: proposal.files.map((f) => f.path) };
  }

  /** Which of `paths` exist in the tree of `ref`. */
  private async pathsPresent(git: GitHost, ref: string, paths: string[]): Promise<string[]> {
    const scratchRoot = this.settings.scratchDir ?? tmpdir();
    await mkdir(scratchRoot, { recursive: true });
    const dir = await mkdtemp(join(scratchRoot, 'ratchet-engine-'));
    try {
      await git.checkout(ref, dir);
      const present: string[] = [];
      for (const p of paths) {
        if (await stat(join(dir, p)).then(() => true, () => false)) present.push(p);
      }
      return present;
    } finally {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    }
  }

  // ------------------------------------------------------------------ pull request

  async openPullRequest(id: string): Promise<{ url: string }> {
    assertValidId('optimization id', id);
    await this.resumed;
    const opt = this.requireOpt(id);
    const existing = this.kv.get(K.pr(id));
    if (existing) return { url: existing };
    if (opt.status !== 'stopped' && opt.status !== 'completed') {
      throw new EngineError('invalid_state', `optimization ${id} is ${opt.status}; stop it before opening the pull request`);
    }
    const history = this.ledger.ratchetHistory(id);
    if (history.length === 0) throw new EngineError('no_verified', `optimization ${id} has no verified candidate to propose`);
    const shas = history.map((m) => m.sha);
    const baseline = this.ledger.getBaseline(id);
    const best = this.ledger.currentBest(id)!;
    const gain = baseline && baseline.value !== 0 ? Math.abs(best - baseline.value) / Math.abs(baseline.value) * 100 : 0;
    const title = `Ratchet: ${opt.metric} ${baseline ? fmt(baseline.value) : '?'} -> ${fmt(best)} (${fmt(gain)}% ${opt.direction === 'minimize' ? 'lower' : 'higher'}, ${shas.length} verified)`;
    const body = await (this.settings.reportBuilder ?? defaultReport)(this.ledger, id);
    const res = await this.gitFor(opt.repo).openPullRequest({ base: opt.baseRef, head: `ratchet/${id}/result`, title, body, shas });
    this.kv.set(K.pr(id), res.url);
    await this.ports.artifacts.put(`engine/${id}/report.md`, body).catch((e) => this.reportError(e, 'artifacts.put report'));
    return res;
  }

  // ------------------------------------------------------------------ cost reconcile

  /**
   * Compare the ledger's estimated spend with Cost Explorer (by cost-allocation tag) and append
   * correction rows (`estimated=false`, `source='cost-explorer'`, may be negative). Idempotent.
   * Writes nothing and returns `pending` while billing data is incomplete (optimization still
   * running, within the billing lag of its end, data flagged as estimated, or no data yet).
   */
  async reconcileCosts(id: string): Promise<CostReconcileReport> {
    assertValidId('optimization id', id);
    await this.resumed;
    this.requireOpt(id);
    const reconciler = this.ports.costs;
    if (!reconciler) throw new EngineError('unavailable', 'no cost reconciler is configured (billing data is not available in this runtime)');
    const report = await reconcileSpend(this.ledger, reconciler, id, { now: this.clock.now(), lagMs: this.settings.costLagMs });
    this.kv.setJson(K.reconcile(id), report);
    if (report.status === 'reconciled') {
      this.emit({ type: 'spend', optimizationId: id, totalUsd: this.ledger.totalSpend(id) });
      this.emit({
        type: 'notice',
        optimizationId: id,
        level: 'info',
        text: `spend reconciled with Cost Explorer: ${report.writtenUsd >= 0 ? '+' : '-'}$${Math.abs(report.writtenUsd).toFixed(4)}`,
      });
    }
    return report;
  }

  // ------------------------------------------------------------------ teardown

  async teardown(id: string): Promise<void> {
    assertValidId('optimization id', id);
    await this.resumed;
    let opt = this.requireOpt(id);
    if (opt.status === 'completed') return;
    if (opt.status === 'running' || opt.status === 'baselining' || opt.status === 'stopping') {
      await this.stop(id, 'teardown');
      opt = this.requireOpt(id);
    }
    const { compute } = this.ports;
    const errors: string[] = [];
    const report: TeardownReport = {
      optimizationId: id,
      at: this.clock.now(),
      deletedSecretsFor: [],
      deletedIdentitiesFor: [],
      terminatedInstanceIds: [],
      orphans: [],
      errors,
    };
    const attempt = async (what: string, fn: () => Promise<void>): Promise<boolean> => {
      try {
        await fn();
        return true;
      } catch (err) {
        errors.push(`${what}: ${(err as Error).message}`);
        this.reportError(err, `teardown ${what}`);
        return false;
      }
    };

    const local = this.agentsWhere(id) === 'local';
    for (const agent of this.ledger.listAgents(id)) {
      if (!local && (await attempt(`deleteSecrets ${agent.id}`, () => compute.deleteSecrets(id, agent.id)))) report.deletedSecretsFor.push(agent.id);
      const identity = this.kv.getJson<AgentIdentity>(K.identity(agent.id));
      if (identity) {
        await this.revokeAgent(agent);
        if (await attempt(`deleteAgentIdentity ${agent.id}`, () => compute.deleteAgentIdentity(identity))) {
          report.deletedIdentitiesFor.push(agent.id);
          this.kv.delete(K.identity(agent.id));
        }
      }
    }
    // Local worktrees and the local clone (the commits the agents pushed are on the remote).
    const runners = this.runnersFor(id);
    if (runners.release) await attempt('release agent workspaces', () => runners.release!(id));
    await attempt('terminate leftovers', async () => {
      report.terminatedInstanceIds = await this.terminateAll(id);
    });
    await this.safeMeter(id);
    await attempt('verify listTagged', async () => {
      const left = (await compute.listTagged(id)).filter((i) => i.state !== 'terminated');
      report.orphans = left.map((i) => ({ id: i.id, role: i.role, state: i.state }));
    });

    // Best effort and never a teardown failure: Cost Explorer lags up to 24 h, so `pending` is the usual answer here.
    if (this.ports.costs) {
      try {
        const r = await this.reconcileCosts(id);
        report.costReconcile = { status: r.status, ...(r.reason ? { reason: r.reason } : {}), writtenUsd: r.writtenUsd };
      } catch (err) {
        report.costReconcile = { status: 'error', reason: (err as Error).message };
        this.reportError(err, 'teardown reconcileCosts');
      }
    }

    this.kv.setJson(K.teardown(id), report);
    await this.ports.artifacts.put(`engine/${id}/teardown.json`, JSON.stringify(report, null, 2)).catch((e) => this.reportError(e, 'artifacts.put teardown'));
    if (report.orphans.length > 0) {
      throw new EngineError('orphans', `teardown left ${report.orphans.length} orphaned instance(s): ${report.orphans.map((o) => o.id).join(', ')}`, report);
    }
    if (errors.length > 0) throw new EngineError('teardown_incomplete', `teardown incomplete: ${errors.join('; ')}`, report);
    this.setStatus(id, 'completed');
  }

  // ------------------------------------------------------------------ tick / run

  /** One pass of the control loop over every optimization. Safe to call concurrently (calls queue). */
  tick(): Promise<void> {
    const p = this.tickChain.then(() => this.tickOnce());
    this.tickChain = p.catch(() => undefined);
    return p;
  }

  /** Await bench jobs that are in flight (tests and shutdown). Does not tick. */
  async settle(): Promise<void> {
    await Promise.all([...this.rt.values()].map((r) => r.job?.promise));
  }

  /** True while any bench job is running. */
  get busy(): boolean {
    return [...this.rt.values()].some((r) => r.job !== undefined && r.job.result === undefined);
  }

  /** Tick every `tickIntervalMs` (via the injected clock) until `signal` aborts or `close()` is called. */
  run(signal?: AbortSignal): Promise<void> {
    if (this.runLoop) return this.runLoop.done;
    const abort = new AbortController();
    const onAbort = (): void => abort.abort();
    signal?.addEventListener('abort', onAbort, { once: true });
    const done = (async () => {
      while (!abort.signal.aborted) {
        try {
          await this.tick();
        } catch (err) {
          this.reportError(err, 'tick');
        }
        if (abort.signal.aborted) break;
        await this.clock.sleep(this.settings.tickIntervalMs, abort.signal);
      }
    })().finally(() => {
      signal?.removeEventListener('abort', onAbort);
      this.runLoop = undefined;
    });
    this.runLoop = { abort, done };
    return done;
  }

  /** Stop the `run()` loop and abort in-flight bench work. Does not touch cloud resources. */
  async close(): Promise<void> {
    this.runLoop?.abort.abort();
    await this.runLoop?.done;
    for (const r of this.rt.values()) r.abort.abort();
    await this.settle().catch(() => undefined);
  }

  private async tickOnce(): Promise<void> {
    await this.resumed;
    for (const summary of this.ledger.listOptimizations()) {
      const id = summary.id;
      try {
        switch (summary.status) {
          case 'baselining':
            await this.enforceLimits(id);
            await this.reapOne(id);
            break;
          case 'running':
            await this.tickRunning(id);
            break;
          case 'stopping':
            await this.tickStopping(id);
            break;
          case 'stopped':
          case 'completed':
          case 'failed':
            await this.tickEnded(id);
            break;
          default:
            break;
        }
      } catch (err) {
        this.reportError(err, `tick ${id}`);
      }
    }
    // After the per-run work: the idle clock of the automatic sandbox (destroys it once no run has been active for the idle window).
    await this.autoSandbox.tick();
    this.syncRunActive();
  }

  private async tickRunning(id: string): Promise<void> {
    const rt = this.state(id);
    if (rt.killing) return;
    await this.collectBenchResult(id);
    const opt = this.requireOpt(id);
    const agents = this.ledger.listAgents(id);
    // An agent that reports done / failed in this very poll may have pushed its last commit just before:
    // it still gets one more git poll in this tick (agents live at the start of the tick are polled).
    const liveAtStart = new Set(agents.filter((a) => LIVE_AGENT.has(this.requireAgent(a.id).status)).map((a) => a.id));
    for (const a of agents) {
      if (rt.killing) return;
      await this.pollRunner(opt, this.requireAgent(a.id), false);
    }
    for (const a of agents) {
      if (rt.killing) return;
      const cur = this.requireAgent(a.id);
      if (LIVE_AGENT.has(cur.status) || liveAtStart.has(a.id)) await this.pollGit(opt, cur);
    }
    if (rt.killing) return;
    if (await this.enforceLimits(id)) return;
    await this.scheduleBench(id);
    await this.reapOne(id);
  }

  /** Graceful stop in progress: keep evaluating queued candidates, then finish. */
  private async tickStopping(id: string): Promise<void> {
    const rt = this.state(id);
    if (rt.killing || this.kv.get(K.stopMode(id)) !== 'graceful') return;
    await this.collectBenchResult(id);
    if (await this.enforceLimits(id)) return;
    const since = this.kv.getNumber(K.stopAt(id)) ?? this.clock.now();
    if (this.clock.now() - since > this.settings.drainTimeoutMs) {
      await this.executeKill(id, `${this.kv.get(K.stopReason(id)) ?? 'stopped'} (drain timeout)`, 'stopped');
      return;
    }
    // Keep collecting while draining: a failed or lagging git listing must not strand a candidate.
    const opt = this.requireOpt(id);
    const gitComplete = await this.drainPoll(opt);
    if (rt.killing) return;
    await this.scheduleBench(id);
    const pending = this.ledger.candidatesWithResults(id).some((c) => c.verdict === 'pending');
    if (gitComplete && !rt.job && !pending) {
      await this.executeKill(id, this.kv.get(K.stopReason(id)) ?? 'stopped', 'stopped');
      return;
    }
    await this.reapOne(id);
  }

  /** Stopped / failed / completed optimizations: keep the reaper and any failed revocation retrying. */
  private async tickEnded(id: string): Promise<void> {
    for (const a of this.ledger.listAgents(id)) {
      if (this.kv.has(K.identity(a.id))) await this.revokeAgent(a);
    }
    await this.reapOne(id);
  }

  private async reapOne(id: string): Promise<void> {
    const instances = await this.ports.compute.listTagged(id);
    const killed = await reap(this.deps, id, instances);
    if (killed.length > 0) await this.safeMeter(id);
  }

  private async safeMeter(id: string): Promise<void> {
    try {
      await this.meter.meter(id);
    } catch (err) {
      this.reportError(err, `meter ${id}`);
    }
  }

  // ------------------------------------------------------------------ cost + stop rules

  /** Meter spend, then apply budget / deadline / plateau rules. Returns true when it started a stop. */
  private async enforceLimits(id: string): Promise<boolean> {
    const rt = this.state(id);
    const opt = this.requireOpt(id);
    await this.safeMeter(id);
    const total = this.ledger.totalSpend(id);
    if (rt.lastSpendEmitted !== total) {
      rt.lastSpendEmitted = total;
      this.emit({ type: 'spend', optimizationId: id, totalUsd: total });
    }
    if (total >= opt.budgetUsd) {
      await this.executeKill(id, `budget exhausted ($${total.toFixed(2)} of $${opt.budgetUsd.toFixed(2)})`, 'stopped');
      return true;
    }
    if (total >= opt.budgetUsd * this.settings.warnFraction && !this.kv.has(K.warned(id))) {
      this.kv.set(K.warned(id), String(this.clock.now()));
      this.emit({
        type: 'notice',
        optimizationId: id,
        level: 'warning',
        text: `spend $${total.toFixed(2)} reached ${Math.round(this.settings.warnFraction * 100)}% of the $${opt.budgetUsd.toFixed(2)} budget`,
      });
    }
    if (opt.status !== 'running') return false;

    const deadline = this.kv.getNumber(K.deadline(id));
    if (deadline !== undefined && this.clock.now() >= deadline) {
      await this.beginGraceful(id, 'time limit reached');
      return true;
    }
    const plateau = this.kv.getNumber(K.plateau(id));
    if (plateau !== undefined) {
      const streak = this.noGainStreak(id);
      if (streak >= plateau) {
        await this.beginGraceful(id, `plateau: ${streak} consecutive evaluations without a verified win`);
        return true;
      }
    }
    const agents = this.ledger.listAgents(id);
    if (agents.length > 0 && agents.every((a) => TERMINAL_AGENT.has(a.status))) {
      await this.beginGraceful(id, 'all agents finished');
      return true;
    }
    return false;
  }

  /** Trailing run of decided, non-verified candidates (blocked, rejected and failed-confirm all count). */
  private noGainStreak(id: string): number {
    const decided = this.ledger
      .candidatesWithResults(id)
      .filter((c) => c.verdict !== 'pending')
      .sort((a, b) => (a.verdictAt ?? 0) - (b.verdictAt ?? 0) || a.submittedAt - b.submittedAt);
    let streak = 0;
    for (let i = decided.length - 1; i >= 0 && decided[i]!.verdict !== 'verified'; i--) streak++;
    return streak;
  }

  // ------------------------------------------------------------------ runner events

  private async pollRunner(opt: OptimizationRow, agent: AgentRow, force: boolean): Promise<void> {
    if (!agent.runnerInstanceId) return;
    if (!force && !LIVE_AGENT.has(agent.status)) return;
    const rt = this.state(opt.id);
    const cursor = this.kv.get(K.cursor(agent.id));
    let res: { events: AgentEvent[]; cursor: string };
    try {
      res = await this.runnersFor(agent.optimizationId).poll(this.handle(agent), cursor);
    } catch (err) {
      const n = (rt.pollFailures.get(agent.id) ?? 0) + 1;
      rt.pollFailures.set(agent.id, n);
      if (n === 1) this.reportError(err, `poll ${agent.id}`);
      return;
    }
    rt.pollFailures.delete(agent.id);
    if (res.events.length === 0 && res.cursor === cursor) return;

    let tokenUsd = 0;
    for (const e of res.events) if (e.usage) tokenUsd += tokenCostUsd(e.usage, this.settings.tokenPricing);
    const now = this.clock.now();
    const newStatus = this.statusFromEvents(res.events);
    // Events, token spend and the cursor commit together: a crash can neither lose nor replay events.
    this.ledger.transaction(() => {
      for (const e of res.events) {
        this.ledger.appendAgentEvent({ agentId: agent.id, type: e.type, payload: e, at: e.at });
      }
      if (tokenUsd > 0) {
        this.ledger.recordSpend({
          optimizationId: opt.id,
          agentId: agent.id,
          category: 'tokens',
          usd: tokenUsd,
          estimated: true,
          source: `tokens:${agent.id}`,
          at: now,
        });
      }
      this.kv.set(K.cursor(agent.id), res.cursor);
      if (newStatus && newStatus !== agent.status) this.ledger.setAgentStatus(agent.id, newStatus);
    });
    for (const e of res.events) this.emit({ type: 'agent-event', optimizationId: opt.id, agentId: agent.id, event: e });
    if (tokenUsd > 0) {
      this.emit({ type: 'spend', optimizationId: opt.id, totalUsd: this.ledger.totalSpend(opt.id) });
    }
  }

  /** `status` events may carry `data.state` = done / failed / paused / running; the last one wins. */
  private statusFromEvents(events: AgentEvent[]): AgentStatus | undefined {
    let out: AgentStatus | undefined;
    for (const e of events) {
      const s = e.type === 'status' ? (e.data?.state ?? finishedState(e.data)) : undefined;
      if (typeof s === 'string' && (AGENT_STATUSES as readonly string[]).includes(s) && ['done', 'failed', 'paused', 'running'].includes(s)) {
        out = s as AgentStatus;
      }
    }
    return out;
  }

  // ------------------------------------------------------------------ git: candidates

  /** Submit the agent branch's new candidate commits. False when the branch could not be fully read (retry later). */
  private async pollGit(opt: OptimizationRow, agent: AgentRow): Promise<boolean> {
    const config = this.config(opt.id);
    const git = this.gitFor(opt.repo);
    const baseSha = await this.baseSha(opt).catch((err) => {
      this.reportError(err, `resolveRef ${opt.baseRef}`);
      return opt.baseRef;
    });
    const all = this.ledger.candidatesWithResults(opt.id);
    let seq = all.length;
    const mine = all.filter((c) => c.agentId === agent.id);
    const known = new Set(mine.map((c) => c.sha));
    const last = mine.at(-1)?.sha;
    let found;
    try {
      found = await git.listCandidates(agent.branch, last);
    } catch (err) {
      this.reportError(err, `listCandidates ${agent.branch}`);
      return false;
    }
    for (const c of found) {
      if (known.has(c.sha)) continue;
      let files: string[];
      try {
        files = await git.changedFiles(baseSha, c.sha);
      } catch (err) {
        this.reportError(err, `changedFiles ${c.sha}`);
        return false; // keep order: retry this commit (and the rest) next tick
      }
      const violations = findProtectedViolations(files, effectiveProtected(config).globs);
      const now = this.clock.now();
      // Sortable ids: the ledger orders by (submittedAt, id), and one poll can find several commits in the same millisecond.
      const candidateId = `${opt.id}-c${String(++seq).padStart(5, '0')}`;
      const row = this.ledger.submitCandidate({ id: candidateId, optimizationId: opt.id, agentId: agent.id, sha: c.sha, message: c.message, submittedAt: now });
      known.add(c.sha);
      this.emit({ type: 'candidate', optimizationId: opt.id, candidateId: row.id, verdict: 'pending' });
      if (violations.length > 0) {
        const reasons = violations.map((v) => `protected path changed: ${v.path} (matches ${v.pattern})`);
        await this.finalize(opt, this.requireCandidate(opt.id, row.id), 'blocked', reasons, undefined);
      }
    }
    return true;
  }

  // ------------------------------------------------------------------ bench queue

  private async scheduleBench(id: string): Promise<void> {
    const rt = this.state(id);
    if (rt.job || rt.killing) return;
    const opt = this.requireOpt(id);
    const config = this.config(id);

    // Recover candidates whose bench result is already recorded (after a crash, or a confirm that is due).
    let needsConfirm: CandidateWithResults | undefined;
    for (const c of this.ledger.candidatesWithResults(id)) {
      if (c.verdict !== 'pending' || c.results.length === 0) continue;
      let outcome: RatchetOutcome | undefined;
      try {
        outcome = await this.settleCandidate(opt, c);
      } catch (err) {
        // Never let one unevaluable candidate wedge the queue (and the drain) forever.
        this.reportError(err, `settle ${c.sha}`);
        await this.finalize(opt, c, 'rejected', [`could not evaluate candidate: ${(err as Error).message}`], undefined);
        continue;
      }
      if (outcome?.verdict === 'pending_confirm' && !needsConfirm) needsConfirm = c;
    }
    if (needsConfirm) {
      this.launchJob(opt, config, needsConfirm, 'confirm');
      return;
    }

    const pending = this.ledger
      .candidatesWithResults(id)
      .filter((c) => c.verdict === 'pending' && c.results.length === 0);
    if (pending.length === 0) return;
    const agents = this.ledger.listAgents(id);
    const start = rt.lastServedAgent ? agents.findIndex((a) => a.id === rt.lastServedAgent) + 1 : 0;
    for (let i = 0; i < agents.length; i++) {
      const agent = agents[(start + i) % agents.length]!;
      const next = pending.find((c) => c.agentId === agent.id);
      if (next) {
        rt.lastServedAgent = agent.id;
        this.launchJob(opt, config, next, 'bench');
        return;
      }
    }
  }

  private launchJob(opt: OptimizationRow, config: RatchetConfig, cand: CandidateWithResults, phase: 'bench' | 'confirm'): void {
    const rt = this.state(opt.id);
    const baseline = this.ledger.getBaseline(opt.id);
    const job: BenchJobState = { candidateId: cand.id, phase, promise: Promise.resolve() };
    const run = this.ports.bench.run(
      {
        optimizationId: opt.id,
        candidateId: cand.id,
        sha: cand.sha,
        phase,
        repeats: this.repeatsFor(opt.id, config),
        ...(phase === 'confirm' && config.bench.confirm_trace ? { trace: config.bench.confirm_trace } : {}),
      },
      config,
      baseline ? { metrics: metricsFromStats(baseline.stats) } : null,
      rt.abort.signal,
    );
    job.promise = Promise.resolve(run).then(
      (outcome) => {
        job.result = { ok: true, outcome };
      },
      (err: unknown) => {
        job.result = { ok: false, err };
      },
    );
    rt.job = job;
    this.logCandidate(cand, `${phase} started`);
  }

  private async collectBenchResult(id: string): Promise<void> {
    const rt = this.state(id);
    const job = rt.job;
    if (!job?.result) return;
    rt.job = undefined;
    if (rt.killing) return;
    const opt = this.requireOpt(id);
    const cand = this.ledger.candidatesWithResults(id).find((c) => c.id === job.candidateId);
    if (!cand || cand.verdict !== 'pending') return;

    if (!job.result.ok) {
      await this.benchAttemptFailed(rt, opt, cand, job.phase, job.result.err);
      return; // otherwise the scheduler retries it next tick
    }

    const outcome = job.result.outcome;
    // A guardrail that fails before any bench run leaves no samples (the pipeline fails fast): the
    // candidate is blocked outright, there is nothing to record or to retry.
    if (outcome.samples.length === 0 && outcome.runs.length === 0) {
      const failed = outcome.guardrails.filter((g) => !g.passed);
      if (failed.length > 0) {
        const reasons = failed.map((g) => `guardrail failed: ${g.name}${g.detail ? ` (${g.detail})` : ''}`);
        await this.finalize(opt, cand, 'blocked', reasons, undefined);
        return;
      }
    }
    try {
      this.ledger.recordBenchResult({
        candidateId: cand.id,
        phase: job.phase,
        samples: samplesOf(outcome),
        stats: statsOf(outcome),
        guardrails: outcome.guardrails,
        startedAt: outcome.startedAt,
        finishedAt: outcome.finishedAt,
      });
    } catch (err) {
      // An unusable outcome counts like a failed run, so it can never be retried forever.
      await this.benchAttemptFailed(rt, opt, cand, job.phase, err);
      return;
    }
    const fresh = this.requireCandidate(id, cand.id);
    await this.settleCandidate(opt, fresh);
  }

  /** Count a failed bench attempt; after `maxBenchAttempts` the candidate is rejected as an infrastructure failure. */
  private async benchAttemptFailed(rt: Runtime, opt: OptimizationRow, cand: CandidateWithResults, phase: 'bench' | 'confirm', err: unknown): Promise<void> {
    const attempts = (rt.attempts.get(`${cand.id}:${phase}`) ?? 0) + 1;
    rt.attempts.set(`${cand.id}:${phase}`, attempts);
    const msg = err instanceof Error ? err.message : String(err);
    this.reportError(err, `bench ${phase} ${cand.sha}`);
    this.logCandidate(cand, `${phase} failed (attempt ${attempts}): ${msg}`, 'error');
    if (attempts >= this.settings.maxBenchAttempts) {
      await this.finalize(opt, cand, 'rejected', [`bench infrastructure error after ${attempts} attempts: ${msg}`], undefined);
    }
  }

  /**
   * Evaluate a candidate purely from what the ledger holds (bench result, optional confirm result,
   * baseline noise floor, current best) and finalize it unless it still needs its confirm run.
   * Because it only reads the ledger, it is also how a restarted engine finishes interrupted work.
   */
  private async settleCandidate(opt: OptimizationRow, cand: CandidateWithResults): Promise<RatchetOutcome | undefined> {
    const bench = cand.results.filter((r) => r.phase === 'bench').at(-1);
    if (!bench) return undefined;
    const confirm = cand.results.filter((r) => r.phase === 'confirm').at(-1);
    const baseline = this.ledger.getBaseline(opt.id);
    const currentBest = this.ledger.currentBest(opt.id);
    if (!baseline || currentBest === null) return undefined;
    const confirmBest = this.kv.getNumber(K.confirmBest(opt.id));
    const outcome = evaluateRatchet({
      direction: opt.direction,
      minGain: this.calibrationFor(opt.id, this.config(opt.id))?.minGain ?? opt.minGain,
      noiseFloor: baseline.noiseFloor,
      currentBest,
      candidate: bench.samples,
      confirm: confirm?.samples,
      confirmBest,
      guardrails: [...bench.guardrails, ...(confirm?.guardrails ?? [])],
      protectedViolations: [],
    });
    if (outcome.verdict === 'pending_confirm') return outcome;
    await this.finalize(opt, cand, outcome.verdict, outcome.reasons, { outcome, confirm: confirm?.samples });
    return outcome;
  }

  /** Persist the verdict (and the ratchet move) atomically, then tell the world and the runners. */
  private async finalize(
    opt: OptimizationRow,
    cand: { id: string; agentId: string; sha: string },
    verdict: 'verified' | 'rejected' | 'failed_confirm' | 'blocked',
    reasons: string[],
    detail: { outcome: RatchetOutcome; confirm: readonly number[] | undefined } | undefined,
  ): Promise<void> {
    const now = this.clock.now();
    const before = this.ledger.currentBest(opt.id);
    let finalVerdict = verdict;
    let finalReasons = reasons;
    let moved: { from: number; to: number } | undefined;
    try {
      this.ledger.transaction(() => {
        this.ledger.setVerdict(cand.id, finalVerdict, finalReasons, now);
        if (finalVerdict === 'verified' && detail) {
          const move = this.ledger.moveRatchet(opt.id, cand.id, detail.outcome.candidateValue, now);
          moved = { from: move.fromValue, to: move.toValue };
          if (detail.confirm && detail.confirm.length > 0) this.kv.set(K.confirmBest(opt.id), String(median(detail.confirm)));
        }
        this.ledger.appendAgentEvent({
          agentId: cand.agentId,
          type: 'verdict',
          payload: { candidateId: cand.id, sha: cand.sha, verdict: finalVerdict, reasons: finalReasons },
          at: now,
        });
      });
    } catch (err) {
      if ((err as { code?: string }).code !== 'ratchet_not_improved') throw err;
      // Lost a race with a better result: the ratchet cannot move backwards, so this is a plain rejection.
      finalVerdict = 'rejected';
      finalReasons = [...reasons, `ratchet already at ${before}; ${(err as Error).message}`];
      this.ledger.setVerdict(cand.id, 'rejected', finalReasons, now);
    }

    this.emit({ type: 'candidate', optimizationId: opt.id, candidateId: cand.id, verdict: finalVerdict });
    this.emit({
      type: 'agent-event',
      optimizationId: opt.id,
      agentId: cand.agentId,
      event: { at: now, type: 'verdict', text: `${cand.sha.slice(0, 10)}: ${finalVerdict}`, data: { sha: cand.sha, verdict: finalVerdict, reasons: finalReasons } },
    });
    const best = this.ledger.currentBest(opt.id) ?? 0;
    if (moved) this.emit({ type: 'ratchet', optimizationId: opt.id, from: moved.from, to: moved.to });

    // Tell the submitting agent, and on a ratchet move announce the new shared best to every other runner.
    const agents = this.ledger.listAgents(opt.id);
    const owner = agents.find((a) => a.id === cand.agentId);
    if (owner) await this.sendTo(owner, { type: 'verdict', sha: cand.sha, verdict: finalVerdict, reasons: finalReasons, currentBest: best });
    if (moved && owner) {
      for (const other of agents) {
        if (other.id === owner.id) continue;
        await this.sendTo(other, { type: 'ratchet', sha: cand.sha, branch: owner.branch, value: moved.to });
      }
    }
  }

  private async sendTo(agent: AgentRow, msg: RunnerMessage): Promise<void> {
    const cur = this.ledger.getAgent(agent.id);
    if (!cur?.runnerInstanceId || !LIVE_AGENT.has(cur.status)) return;
    try {
      await this.runnersFor(cur.optimizationId).send(this.handle(cur), msg);
    } catch (err) {
      this.reportError(err, `send ${msg.type} to ${agent.id}`);
      this.logAgent(cur, { at: this.clock.now(), type: 'error', text: `could not deliver ${msg.type}: ${(err as Error).message}` });
    }
  }

  // ------------------------------------------------------------------ resume

  /**
   * Re-attach to whatever the ledger says was in flight. Everything the control loop needs is read
   * from the ledger / kv on each tick, so this only restarts work that was blocking a call when the
   * process died: a baseline in progress, a half-finished launch, a kill switch that did not finish.
   */
  async resume(): Promise<ResumeReport> {
    const attached: string[] = [];
    for (const o of this.ledger.listOptimizations({ status: ['baselining', 'running', 'stopping'] })) {
      attached.push(o.id);
      if (o.status === 'baselining' && !this.starting.has(o.id)) {
        void this.beginStart(o.id).catch((err) => this.reportError(err, `resume start ${o.id}`));
      } else if (o.status === 'stopping' && this.kv.get(K.stopMode(o.id)) === 'kill' && !this.state(o.id).killing) {
        const reason = this.kv.get(K.stopReason(o.id)) ?? 'stopped';
        void this.executeKill(o.id, reason, 'stopped').catch((err) => this.reportError(err, `resume kill ${o.id}`));
      } else if (o.status === 'running') {
        const last = this.lastBenchedAgent(o.id);
        if (last) this.state(o.id).lastServedAgent = last;
      }
    }
    return { attached };
  }

  private lastBenchedAgent(id: string): string | undefined {
    let best: { resultId: number; agentId: string } | undefined;
    for (const c of this.ledger.candidatesWithResults(id)) {
      for (const r of c.results) if (!best || r.id > best.resultId) best = { resultId: r.id, agentId: c.agentId };
    }
    return best?.agentId;
  }

  // ------------------------------------------------------------------ helpers

  private state(id: string): Runtime {
    let r = this.rt.get(id);
    if (!r) {
      r = { killing: false, abort: new AbortController(), attempts: new Map(), pollFailures: new Map() };
      this.rt.set(id, r);
    }
    return r;
  }

  private requireOpt(id: string): OptimizationRow {
    const o = this.ledger.getOptimization(id);
    if (!o) throw new EngineError('not_found', `optimization ${id} not found`);
    return o;
  }

  private requireAgent(id: string): AgentRow {
    const a = this.ledger.getAgent(id);
    if (!a) throw new EngineError('not_found', `agent ${id} not found`);
    return a;
  }

  private requireCandidate(optId: string, candId: string): CandidateWithResults {
    const c = this.ledger.candidatesWithResults(optId).find((x) => x.id === candId);
    if (!c) throw new EngineError('not_found', `candidate ${candId} not found`);
    return c;
  }

  private config(id: string): RatchetConfig {
    let c = this.configCache.get(id);
    if (!c) {
      const parsed = parseConfig(this.requireOpt(id).configYaml);
      if (!parsed.ok) throw new EngineError('invalid_config', `stored config for ${id} is invalid: ${parsed.errors.map(formatConfigError).join('; ')}`);
      c = parsed.config;
      this.configCache.set(id, c);
    }
    return c;
  }

  private handle(agent: AgentRow): RunnerHandle {
    return { optimizationId: agent.optimizationId, agentId: agent.id, instanceId: agent.runnerInstanceId ?? '' };
  }

  private setStatus(id: string, status: Parameters<Ledger['setOptimizationStatus']>[1]): void {
    this.ledger.setOptimizationStatus(id, status, this.clock.now());
    this.emit({ type: 'optimization', optimizationId: id, status });
    this.syncRunActive();
  }

  /** Optimizations that hold the machine awake: starting, baselining, running or stopping. */
  private activeRunIds(): string[] {
    const ids = new Set(this.pendingStarts);
    for (const o of this.ledger.listOptimizations({ status: ['baselining', 'running', 'stopping'] })) ids.add(o.id);
    return [...ids];
  }

  /** Emit `run-active` when the "any run is live" signal flips (the desktop turns it into a power-save blocker). */
  private syncRunActive(): void {
    const ids = this.activeRunIds();
    const active = ids.length > 0;
    if (active === this.runActiveFlag) return;
    this.runActiveFlag = active;
    this.emit({ type: 'run-active', active, optimizationIds: ids });
  }

  private emit(e: EngineEvent): void {
    for (const l of [...this.listeners]) {
      try {
        l(e);
      } catch (err) {
        this.reportError(err, 'listener');
      }
    }
  }

  private logAgent(agent: AgentRow, e: AgentEvent): void {
    this.ledger.appendAgentEvent({ agentId: agent.id, type: e.type, payload: e, at: e.at });
    this.emit({ type: 'agent-event', optimizationId: agent.optimizationId, agentId: agent.id, event: e });
  }

  private logCandidate(cand: { agentId: string; sha: string }, text: string, type: AgentEvent['type'] = 'status'): void {
    const agent = this.ledger.getAgent(cand.agentId);
    if (agent) this.logAgent(agent, { at: this.clock.now(), type, text: `${cand.sha.slice(0, 10)}: ${text}` });
  }

  private reportError(err: unknown, context: string): void {
    const message = err instanceof Error ? err.message : String(err);
    this.errors.push({ at: this.clock.now(), context, message });
    if (this.errors.length > 200) this.errors.shift();
    try {
      this.settings.onError(err, context);
    } catch {
      // never let a logging hook break the loop
    }
  }
}

/**
 * The runner loop's closing status event carries `{ reason, turns, candidates, ... }` and no `state`:
 * map its stop reason so an agent that ran out of ideas, budget or time counts as done, and one that
 * crashed or could not start as failed. A `stopped` / `aborted` reason is the engine's own doing.
 */
function finishedState(data: Record<string, unknown> | undefined): AgentStatus | undefined {
  if (!data || typeof data.reason !== 'string' || typeof data.turns !== 'number') return undefined;
  switch (data.reason) {
    case 'idle':
    case 'budget':
    case 'deadline':
      return 'done';
    case 'failed':
    case 'agent-unavailable':
      return 'failed';
    default:
      return undefined;
  }
}

function fmt(n: number): string {
  return String(Number(n.toPrecision(5)));
}

