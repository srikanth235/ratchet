import {
  evaluateRatchet,
  noiseFloor,
  parseConfig,
  summarize,
  type AgentEvent,
  type AgentId,
  type AgentKind,
  type EngineApi,
  type EngineCreateOptimizationInput,
  type EngineEvent,
  type GuardrailResult,
  type Ledger,
  type Mode,
  type OptimizationId,
  type RatchetConfig,
} from '@ratchet/core';
import { DEFAULT_CONFIG_YAML } from './default-config.js';

export interface DevEngineOptions {
  mode: Mode;
  /** Real-time ms between simulated agent turns. Default 2500. */
  tickMs?: number;
  /** Delay before the simulated baseline completes. Default 1200. */
  baselineMs?: number;
  /** PRNG seed (deterministic tests). */
  seed?: number;
  now?: () => number;
  /** Set false to drive the simulation manually with `tick()`. Default true. */
  autoRun?: boolean;
}

/** `EngineCreateOptimizationInput` plus the reviewed config text the dev engine can use in place of reading the repo. */
export type DevCreateInput = EngineCreateOptimizationInput & { configYaml?: string };

const HYPOTHESES = [
  'Memoise the tokenizer so repeated queries skip re-parsing',
  'Batch index reads to cut round trips on the hot path',
  'Replace the JSON response encoder with a pre-sized buffer writer',
  'Cache compiled query plans keyed by normalised query',
  'Avoid re-sorting results that the index already returns ordered',
  'Move the facet counting off the request path into a lazy iterator',
  'Switch the scoring loop to typed arrays to reduce GC pressure',
  'Short-circuit empty queries before they reach the index',
];

const PROTECTED_TOUCHES = ['bench/k6/search.js', 'tests/contract/search.test.ts'];

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface Run {
  timer: ReturnType<typeof setInterval> | null;
  config: RatchetConfig;
  noEval: number; // consecutive evaluations that did not move the ratchet
  turn: number;
}

/**
 * Temporary in-repo `EngineApi` used by the desktop app until the orchestrator swaps `createEngineForApp`
 * to the real engine. It writes everything it simulates into the ledger through the normal `Ledger` methods
 * (baseline, agents, agent events, candidates, bench results, verdicts via the real ratchet rule, ratchet
 * moves, spend), so the UI reads exactly what the real engine will produce.
 */
export class DevEngine implements EngineApi {
  private readonly listeners = new Set<(e: EngineEvent) => void>();
  private readonly runs = new Map<OptimizationId, Run>();
  private readonly paused = new Set<AgentId>();
  private readonly inbox = new Map<AgentId, string[]>();
  private readonly rand: () => number;
  private readonly now: () => number;
  private readonly tickMs: number;
  private readonly baselineMs: number;
  private readonly autoRun: boolean;
  private readonly modeValue: Mode;
  private pending: ReturnType<typeof setTimeout>[] = [];

  constructor(
    private readonly ledger: Ledger,
    opts: DevEngineOptions,
  ) {
    this.modeValue = opts.mode;
    this.rand = mulberry32(opts.seed ?? Date.now());
    this.now = opts.now ?? Date.now;
    this.tickMs = opts.tickMs ?? 2500;
    this.baselineMs = opts.baselineMs ?? 1200;
    this.autoRun = opts.autoRun ?? true;
  }

  mode(): Mode {
    return this.modeValue;
  }

  subscribe(listener: (e: EngineEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Stop all timers (app quit / mode switch). Ledger state is untouched. */
  dispose(): void {
    for (const r of this.runs.values()) if (r.timer) clearInterval(r.timer);
    for (const t of this.pending) clearTimeout(t);
    this.runs.clear();
    this.pending = [];
    this.listeners.clear();
  }

  // ------------------------------------------------------------ EngineApi

  async createOptimization(input: DevCreateInput): Promise<OptimizationId> {
    const parsed = parseConfig(input.configYaml ?? DEFAULT_CONFIG_YAML);
    if (!parsed.ok) throw new Error(`invalid ratchet.yaml: ${parsed.errors.map((e) => e.message).join('; ')}`);
    const cfg = parsed.config;
    const repo = repoSlug(input.repoUrl);
    const opt = this.ledger.createOptimization({
      name: input.name ?? `${repo.split('/').pop()} ${cfg.objective.metric}`,
      repo,
      baseRef: input.baseRef,
      metric: cfg.objective.metric,
      direction: cfg.objective.direction,
      minGain: cfg.objective.min_gain,
      budgetUsd: input.budgetUsd,
      timeLimitS: input.timeLimitSeconds,
      configYaml: input.configYaml ?? DEFAULT_CONFIG_YAML,
      status: 'draft',
    });
    input.agents.forEach((kind, i) => {
      this.ledger.addAgent({ optimizationId: opt.id, kind, branch: `ratchet/${opt.id.slice(4, 12)}/${kind}-${i + 1}` });
    });
    this.plateauByOpt.set(opt.id, input.plateauEvals);
    this.emit({ type: 'optimization', optimizationId: opt.id, status: 'draft' });
    return opt.id;
  }

  async start(id: OptimizationId): Promise<void> {
    const opt = this.ledger.getOptimization(id);
    if (!opt) throw new Error(`optimization ${id} not found`);
    if (this.runs.has(id)) return;
    const parsed = parseConfig(opt.configYaml);
    if (!parsed.ok) throw new Error('stored ratchet.yaml is invalid');
    const run: Run = { timer: null, config: parsed.config, noEval: 0, turn: 0 };
    this.runs.set(id, run);

    if (this.ledger.getBaseline(id)) {
      this.beginRunning(id, run);
      return;
    }
    this.ledger.setOptimizationStatus(id, 'baselining');
    for (const a of this.ledger.listAgents(id)) this.ledger.setAgentStatus(a.id, 'launching', `i-sim${a.id.slice(-8)}`);
    this.emit({ type: 'optimization', optimizationId: id, status: 'baselining' });
    const finish = () => {
      this.recordBaseline(id, run);
      this.beginRunning(id, run);
    };
    if (this.autoRun) this.pending.push(setTimeout(finish, this.baselineMs));
    else finish();
  }

  async stop(id: OptimizationId, _reason?: string): Promise<void> {
    const opt = this.ledger.getOptimization(id);
    if (!opt) throw new Error(`optimization ${id} not found`);
    this.halt(id);
    this.ledger.setOptimizationStatus(id, 'stopping');
    for (const a of this.ledger.listAgents(id)) this.ledger.setAgentStatus(a.id, 'stopped');
    this.ledger.setOptimizationStatus(id, 'stopped');
    this.emit({ type: 'optimization', optimizationId: id, status: 'stopped' });
  }

  async pauseAgent(agentId: AgentId): Promise<void> {
    this.requireAgent(agentId);
    this.paused.add(agentId);
    this.ledger.setAgentStatus(agentId, 'paused');
    this.logEvent(agentId, { type: 'status', text: 'Paused by user' });
  }

  async resumeAgent(agentId: AgentId): Promise<void> {
    this.requireAgent(agentId);
    this.paused.delete(agentId);
    this.ledger.setAgentStatus(agentId, 'running');
    this.logEvent(agentId, { type: 'status', text: 'Resumed' });
  }

  async messageAgent(agentId: AgentId, text: string): Promise<void> {
    this.requireAgent(agentId);
    const q = this.inbox.get(agentId) ?? [];
    q.push(text);
    this.inbox.set(agentId, q);
    this.logEvent(agentId, { type: 'message', text: `You: ${text}`, data: { from: 'user' } });
  }

  async openPullRequest(id: OptimizationId): Promise<{ url: string }> {
    const opt = this.ledger.getOptimization(id);
    if (!opt) throw new Error(`optimization ${id} not found`);
    if (this.ledger.ratchetHistory(id).length === 0) throw new Error('no verified wins to open a pull request for');
    return { url: `https://github.com/${opt.repo}/pull/${100 + (id.charCodeAt(id.length - 1) % 800)}` };
  }

  async teardown(id: OptimizationId): Promise<void> {
    if (!this.ledger.getOptimization(id)) throw new Error(`optimization ${id} not found`);
    this.halt(id);
  }

  // ------------------------------------------------------------ simulation

  /** Run one simulated agent turn for every running agent of `id` (called by the timer; public for tests). */
  tick(id: OptimizationId): void {
    const run = this.runs.get(id);
    const opt = this.ledger.getOptimization(id);
    if (!run || !opt || opt.status !== 'running') return;
    run.turn += 1;

    const elapsed = this.now() - (opt.startedAt ?? opt.createdAt);
    const spent = this.ledger.totalSpend(id);
    if (spent >= opt.budgetUsd) return this.finish(id, 'budget exhausted');
    if (elapsed >= opt.timeLimitS * 1000) return this.finish(id, 'time limit reached');
    if (run.noEval >= this.plateau(id)) return this.finish(id, 'plateau: no verified win in the last evaluations');

    for (const agent of this.ledger.listAgents(id)) {
      if (agent.status !== 'running') continue;
      this.agentTurn(id, run, agent.id, agent.kind);
      if (this.ledger.getOptimization(id)?.status !== 'running') return;
    }
  }

  /** Plateau stop-rule (consecutive non-improving evaluations), kept in memory: the ledger has no column for it. */
  private plateauByOpt = new Map<OptimizationId, number>();
  private plateau(id: OptimizationId): number {
    return this.plateauByOpt.get(id) ?? 25;
  }

  private recordBaseline(id: OptimizationId, run: Run): void {
    const max = run.config.objective.direction === 'maximize';
    const center = max ? 40 + this.rand() * 10 : 150 + this.rand() * 60;
    const samples = Array.from({ length: Math.max(5, run.config.bench.repeats) }, () => round(center * (1 + (this.rand() - 0.5) * 0.04)));
    const s = summarize(samples);
    const t = this.now();
    this.ledger.recordBaseline({
      optimizationId: id,
      samples,
      noiseFloor: noiseFloor(samples),
      stats: { value: s.median, p50: s.median, p95: round(s.median * 1.18), p99: round(s.median * 1.42), mean: s.mean },
      recordedAt: t,
    });
    this.ledger.recordSpend({ optimizationId: id, category: 'bench_runner', usd: 0.62, source: 'dev-engine:baseline', at: t });
  }

  private beginRunning(id: OptimizationId, run: Run): void {
    this.ledger.setOptimizationStatus(id, 'running');
    for (const a of this.ledger.listAgents(id)) {
      if (a.status !== 'running' && a.status !== 'paused') this.ledger.setAgentStatus(a.id, 'running', a.runnerInstanceId ?? `i-sim${a.id.slice(-8)}`);
      this.logEvent(a.id, { type: 'status', text: 'Agent runner online; reading RATCHET.md' });
    }
    this.emit({ type: 'optimization', optimizationId: id, status: 'running' });
    if (this.autoRun) run.timer = setInterval(() => this.tick(id), this.tickMs);
  }

  private agentTurn(id: OptimizationId, run: Run, agentId: AgentId, kind: AgentKind): void {
    const opt = this.ledger.getOptimization(id)!;
    const cfg = run.config;
    const baseline = this.ledger.getBaseline(id)!;
    const best = this.ledger.currentBest(id) ?? baseline.value;
    const dir = cfg.objective.direction;
    const sign = dir === 'minimize' ? -1 : 1;

    const queued = this.inbox.get(agentId)?.shift();
    const hypothesis = queued ? `Following your note: ${queued}` : HYPOTHESES[Math.floor(this.rand() * HYPOTHESES.length)]!;
    this.logEvent(agentId, { type: 'message', text: hypothesis });
    this.logEvent(agentId, { type: 'tool', text: 'edit src/search/index.ts' });

    const sha = Array.from({ length: 40 }, () => Math.floor(this.rand() * 16).toString(16)).join('');
    this.logEvent(agentId, { type: 'commit', text: `Ratchet-Candidate: ${sha.slice(0, 7)}`, data: { sha } });
    const t0 = this.now();
    const cand = this.ledger.submitCandidate({ optimizationId: id, agentId, sha, message: hypothesis, submittedAt: t0 });

    // Effect: mostly small, sometimes a real win, sometimes a regression.
    const r = this.rand();
    const effect = r < 0.22 ? 0.04 + this.rand() * 0.05 : r < 0.75 ? (this.rand() - 0.4) * 0.03 : -0.02 - this.rand() * 0.03;
    const centre = best * (1 + sign * effect);
    const sample = (c: number) => round(c * (1 + (this.rand() - 0.5) * 0.03));
    const samples = Array.from({ length: cfg.bench.repeats }, () => sample(centre));
    const guardrails: GuardrailResult[] = cfg.guardrails.map((g) => {
      const name = g.kind === 'command' ? g.run : `${g.metric} ${g.raw}`;
      return this.rand() < 0.04 ? { name, passed: false, detail: 'regressed' } : { name, passed: true };
    });
    const touched = this.rand() < 0.04 ? [PROTECTED_TOUCHES[Math.floor(this.rand() * PROTECTED_TOUCHES.length)]!] : [];
    const protectedViolations = touched.map((path) => ({ path, pattern: path.startsWith('bench') ? 'bench/**' : 'tests/contract/**' }));
    const stats = (xs: number[]) => {
      const s = summarize(xs);
      return { value: s.median, p50: s.median, p95: round(s.median * 1.18), p99: round(s.median * 1.42), mean: s.mean };
    };

    const input = { direction: dir, minGain: opt.minGain, noiseFloor: baseline.noiseFloor, currentBest: best, candidate: samples, guardrails, protectedViolations };
    const bench0 = this.now();
    let outcome = evaluateRatchet(input);
    this.ledger.recordBenchResult({ candidateId: cand.id, phase: 'bench', samples, stats: stats(samples), guardrails, startedAt: bench0, finishedAt: this.now() });
    this.spend(id, agentId, 'bench_runner', 0.18 + this.rand() * 0.12, 'dev-engine:bench');
    this.spend(id, agentId, 'tokens', 0.25 + this.rand() * 0.55, `dev-engine:${kind}`);
    this.spend(id, agentId, 'agent_runner', 0.06 + this.rand() * 0.04, 'dev-engine:runner');
    this.spend(id, agentId, 'experiment', this.rand() * 0.1, 'dev-engine:experiment');
    if (this.rand() < 0.2) this.spend(id, agentId, 'storage_egress', 0.01, 'dev-engine:s3');

    if (outcome.verdict === 'pending_confirm') {
      // Hidden-trace confirm: usually holds, occasionally doesn't.
      const holds = this.rand() > 0.15;
      const confirm = Array.from({ length: cfg.bench.repeats }, () => sample(holds ? centre : best * (1 + (-sign) * 0.01)));
      const c0 = this.now();
      outcome = evaluateRatchet({ ...input, confirm });
      this.ledger.recordBenchResult({ candidateId: cand.id, phase: 'confirm', samples: confirm, stats: stats(confirm), guardrails: [], startedAt: c0, finishedAt: this.now() });
      this.spend(id, agentId, 'bench_runner', 0.2, 'dev-engine:confirm');
    }

    const verdict = outcome.verdict === 'pending_confirm' ? 'rejected' : outcome.verdict;
    this.ledger.setVerdict(cand.id, verdict, outcome.reasons);
    this.emit({ type: 'candidate', optimizationId: id, candidateId: cand.id, verdict });
    if (verdict === 'verified') {
      this.ledger.moveRatchet(id, cand.id, outcome.candidateValue);
      this.emit({ type: 'ratchet', optimizationId: id, from: best, to: outcome.candidateValue });
      run.noEval = 0;
    } else {
      run.noEval += 1;
    }
    this.logEvent(agentId, {
      type: 'verdict',
      text: `${verdict}: ${outcome.reasons[0] ?? ''}`,
      data: { sha, verdict, value: outcome.candidateValue },
    });
    this.emit({ type: 'spend', optimizationId: id, totalUsd: this.ledger.totalSpend(id) });
  }

  private finish(id: OptimizationId, why: string): void {
    this.halt(id);
    for (const a of this.ledger.listAgents(id)) {
      this.ledger.setAgentStatus(a.id, 'done');
      this.logEvent(a.id, { type: 'status', text: `Stopped: ${why}` });
    }
    this.ledger.setOptimizationStatus(id, 'completed');
    this.emit({ type: 'optimization', optimizationId: id, status: 'completed' });
  }

  private halt(id: OptimizationId): void {
    const run = this.runs.get(id);
    if (run?.timer) clearInterval(run.timer);
    this.runs.delete(id);
  }

  private spend(id: OptimizationId, agentId: AgentId, category: 'bench_runner' | 'tokens' | 'agent_runner' | 'experiment' | 'storage_egress', usd: number, source: string): void {
    this.ledger.recordSpend({ optimizationId: id, agentId, category, usd: round(usd, 4), source, at: this.now() });
  }

  private logEvent(agentId: AgentId, e: Omit<AgentEvent, 'at'>): void {
    const at = this.now();
    const optimizationId = this.ledger.getAgent(agentId)!.optimizationId;
    this.ledger.appendAgentEvent({ agentId, type: e.type, payload: { text: e.text, data: e.data }, at });
    this.emit({ type: 'agent-event', optimizationId, agentId, event: { at, ...e } });
  }

  private requireAgent(agentId: AgentId): void {
    if (!this.ledger.getAgent(agentId)) throw new Error(`agent ${agentId} not found`);
  }

  private emit(e: EngineEvent): void {
    for (const l of this.listeners) {
      try {
        l(e);
      } catch {
        /* a failing listener must not break the simulation */
      }
    }
  }
}

const round = (n: number, dp = 2): number => Number(n.toFixed(dp));

/** `https://github.com/acme/search-api.git` / `git@github.com:acme/search-api` / `acme/search-api` → `acme/search-api`. */
export function repoSlug(repoUrl: string): string {
  const m = /([\w.-]+\/[\w.-]+?)(?:\.git)?\/?$/.exec(repoUrl.trim());
  return m?.[1] ?? repoUrl.trim();
}
