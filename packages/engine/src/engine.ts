import { randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join, normalize } from 'node:path';
import {
  AGENT_KINDS,
  AGENT_STATUSES,
  evaluateRatchet,
  findProtectedViolations,
  formatConfigError,
  guardrailName,
  median,
  noiseFloor as computeNoiseFloor,
  parseConfig,
  type AgentEvent,
  type AgentIdentity,
  type AgentKind,
  type AgentRow,
  type AgentStatus,
  type BenchOutcome,
  type CandidateWithResults,
  type Clock,
  type EngineApi,
  type EngineCreateOptimizationInput,
  type EngineEvent,
  type GitHost,
  type Ledger,
  type Mode,
  type OptimizationRow,
  type RatchetConfig,
  type RatchetOutcome,
  type RunnerHandle,
  type RunnerMessage,
  type SecretRef,
} from '@ratchet/core';
import { metricsFromStats, samplesOf, statsOf } from './bench-stats.js';
import { CostMeter } from './cost.js';
import { tokenCostUsd, type Deps } from './deps.js';
import { assertValidId, EngineError } from './errors.js';
import { K, KvStore } from './kv.js';
import { reap } from './reaper.js';
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
    this.resumed = this.resume().catch((err) => {
      this.reportError(err, 'resume');
      return { attached: [] };
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

  // ------------------------------------------------------------------ createOptimization

  async createOptimization(input: CreateOptimizationInput): Promise<string> {
    await this.resumed;
    this.validateCreateInput(input);
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
        minGain: config.objective.min_gain,
        budgetUsd: input.budgetUsd,
        timeLimitS: input.timeLimitSeconds,
        configYaml,
        status: 'draft',
        createdAt: now,
      });
      for (const s of slots) {
        this.ledger.addAgent({ id: s.agentId, optimizationId: id, kind: s.kind, branch: s.branch, status: 'pending', createdAt: now });
      }
      this.kv.set(K.plateau(id), String(input.plateauEvals));
    });
    this.emit({ type: 'optimization', optimizationId: id, status: 'draft' });
    return id;
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
    await this.beginStart(id);
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
        { optimizationId: id, sha: await this.baseSha(opt), phase: 'baseline', repeats: config.bench.repeats },
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
        noiseFloor: computeNoiseFloor(samples, this.settings.noise),
        stats,
        value: stats.value,
        recordedAt: this.clock.now(),
      });
    }
    if (config.bench.confirm_trace && this.settings.confirmBaseline && !this.kv.has(K.confirmBest(id))) {
      const confirm = await this.ports.bench.run(
        { optimizationId: id, sha: await this.baseSha(opt), phase: 'confirm', repeats: config.bench.repeats, trace: config.bench.confirm_trace },
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
    const { compute, runners } = this.ports;
    const git = this.gitFor(opt.repo);
    let step = this.kv.getNumber(K.launch(agentId)) ?? 0;
    const done = (n: number): void => {
      step = n;
      this.kv.set(K.launch(agentId), String(n));
    };
    if (step >= 5) return;
    if (agent.status === 'pending') this.ledger.setAgentStatus(agentId, 'launching');

    if (step < 1) {
      const identity = await compute.createAgentIdentity(optId, agentId);
      this.kv.setJson(K.identity(agentId), identity);
      done(1);
    }
    const identity = this.kv.getJson<AgentIdentity>(K.identity(agentId));
    if (!identity) throw new EngineError('invalid_state', `identity for ${agentId} is missing`);

    if (step < 2) {
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
    if (step < 4) {
      const instance = await compute.launchRunner({
        optimizationId: optId,
        agentId,
        kind: agent.kind,
        instanceType: config.agent.allowed_instance_types[0]!,
        imageRef: this.settings.runnerImageRef,
        identity,
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
        .map((a) => this.ports.runners.stop(this.handle(a)).catch((e) => this.reportError(e, `runners.stop ${a.id}`))),
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
        .map((a) => this.ports.runners.stop(this.handle(a)).catch((e) => this.reportError(e, `runners.stop ${a.id}`))),
    );
    // The runners are down: collect what they left behind, then close the agents' access.
    for (const a of agents) {
      const cur = this.requireAgent(a.id);
      await this.pollRunner(opt, cur, true);
      await this.pollGit(opt, this.requireAgent(a.id));
      const after = this.requireAgent(a.id);
      if (!TERMINAL_AGENT.has(after.status)) this.ledger.setAgentStatus(a.id, 'stopped');
    }
    await Promise.allSettled(this.ledger.listAgents(id).map((a) => this.revokeAgent(a)));
  }

  // ------------------------------------------------------------------ agent control

  async pauseAgent(agentId: string): Promise<void> {
    const agent = this.liveAgent(agentId);
    if (agent.status === 'paused') return;
    await this.ports.runners.send(this.handle(agent), { type: 'pause' });
    this.ledger.setAgentStatus(agentId, 'paused');
    this.logAgent(agent, { at: this.clock.now(), type: 'status', text: 'paused by user', data: { state: 'paused' } });
  }

  async resumeAgent(agentId: string): Promise<void> {
    const agent = this.liveAgent(agentId);
    if (agent.status !== 'paused') return;
    await this.ports.runners.send(this.handle(agent), { type: 'resume' });
    this.ledger.setAgentStatus(agentId, 'running');
    this.logAgent(agent, { at: this.clock.now(), type: 'status', text: 'resumed by user', data: { state: 'running' } });
  }

  async messageAgent(agentId: string, text: string): Promise<void> {
    if (typeof text !== 'string' || text.trim() === '') throw new EngineError('invalid_input', 'message must not be empty');
    const agent = this.liveAgent(agentId);
    await this.ports.runners.send(this.handle(agent), { type: 'user', text });
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

    for (const agent of this.ledger.listAgents(id)) {
      if (await attempt(`deleteSecrets ${agent.id}`, () => compute.deleteSecrets(id, agent.id))) report.deletedSecretsFor.push(agent.id);
      const identity = this.kv.getJson<AgentIdentity>(K.identity(agent.id));
      if (identity) {
        await this.revokeAgent(agent);
        if (await attempt(`deleteAgentIdentity ${agent.id}`, () => compute.deleteAgentIdentity(identity))) {
          report.deletedIdentitiesFor.push(agent.id);
          this.kv.delete(K.identity(agent.id));
        }
      }
    }
    await attempt('terminate leftovers', async () => {
      report.terminatedInstanceIds = await this.terminateAll(id);
    });
    await this.safeMeter(id);
    await attempt('verify listTagged', async () => {
      const left = (await compute.listTagged(id)).filter((i) => i.state !== 'terminated');
      report.orphans = left.map((i) => ({ id: i.id, role: i.role, state: i.state }));
    });

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
  }

  private async tickRunning(id: string): Promise<void> {
    const rt = this.state(id);
    if (rt.killing) return;
    await this.collectBenchResult(id);
    const opt = this.requireOpt(id);
    const agents = this.ledger.listAgents(id);
    for (const a of agents) {
      if (rt.killing) return;
      await this.pollRunner(opt, this.requireAgent(a.id), false);
    }
    for (const a of agents) {
      if (rt.killing) return;
      const cur = this.requireAgent(a.id);
      if (LIVE_AGENT.has(cur.status)) await this.pollGit(opt, cur);
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
    await this.scheduleBench(id);
    const pending = this.ledger.candidatesWithResults(id).some((c) => c.verdict === 'pending');
    if (!rt.job && !pending) {
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
      res = await this.ports.runners.poll(this.handle(agent), cursor);
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
      const s = e.type === 'status' ? e.data?.state : undefined;
      if (typeof s === 'string' && (AGENT_STATUSES as readonly string[]).includes(s) && ['done', 'failed', 'paused', 'running'].includes(s)) {
        out = s as AgentStatus;
      }
    }
    return out;
  }

  // ------------------------------------------------------------------ git: candidates

  private async pollGit(opt: OptimizationRow, agent: AgentRow): Promise<void> {
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
      return;
    }
    for (const c of found) {
      if (known.has(c.sha)) continue;
      let files: string[];
      try {
        files = await git.changedFiles(baseSha, c.sha);
      } catch (err) {
        this.reportError(err, `changedFiles ${c.sha}`);
        return; // keep order: retry this commit (and the rest) next tick
      }
      const violations = findProtectedViolations(files, config.protected);
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
      const outcome = await this.settleCandidate(opt, c);
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
        repeats: config.bench.repeats,
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
      const attempts = (rt.attempts.get(`${cand.id}:${job.phase}`) ?? 0) + 1;
      rt.attempts.set(`${cand.id}:${job.phase}`, attempts);
      const msg = job.result.err instanceof Error ? job.result.err.message : String(job.result.err);
      this.reportError(job.result.err, `bench ${job.phase} ${cand.sha}`);
      this.logCandidate(cand, `${job.phase} failed (attempt ${attempts}): ${msg}`, 'error');
      if (attempts >= this.settings.maxBenchAttempts) {
        await this.finalize(opt, cand, 'rejected', [`bench infrastructure error after ${attempts} attempts: ${msg}`], undefined);
      }
      return; // otherwise the scheduler retries it next tick
    }

    const outcome = job.result.outcome;
    this.ledger.recordBenchResult({
      candidateId: cand.id,
      phase: job.phase,
      samples: samplesOf(outcome),
      stats: statsOf(outcome),
      guardrails: outcome.guardrails,
      startedAt: outcome.startedAt,
      finishedAt: outcome.finishedAt,
    });
    const fresh = this.requireCandidate(id, cand.id);
    await this.settleCandidate(opt, fresh);
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
      minGain: opt.minGain,
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
      await this.ports.runners.send(this.handle(cur), msg);
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

function fmt(n: number): string {
  return String(Number(n.toPrecision(5)));
}

