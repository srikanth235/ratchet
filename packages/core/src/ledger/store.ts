import { randomUUID } from 'node:crypto';
import type { Database as SqliteDatabase } from 'better-sqlite3';
import { and, asc, desc, eq, gt, inArray, ne, sql } from 'drizzle-orm';
import type { LedgerDb } from './db.js';
import {
  SPEND_CATEGORIES,
  agentEvents,
  agents,
  baselines,
  benchResults,
  candidates,
  optimizations,
  ratchetMoves,
  sandboxes,
  spendEvents,
  type AgentEventRow,
  type AgentKind,
  type AgentRow,
  type AgentStatus,
  type BaselineRow,
  type BenchPhase,
  type BenchResultRow,
  type BenchStats,
  type CandidateRow,
  type Direction,
  type GuardrailResult,
  type OptimizationRow,
  type OptimizationStatus,
  type RatchetMoveRow,
  type EvaluatorKind,
  type SandboxProviderKind,
  type SandboxRow,
  type SandboxStatus,
  type ValueBasis,
  type ValuePeriod,
  type SpendCategory,
  type SpendEventRow,
  type CandidateVerdict,
} from './schema.js';

export type LedgerErrorCode =
  | 'not_found'
  | 'invalid'
  | 'no_baseline'
  | 'ratchet_not_improved';

export class LedgerError extends Error {
  constructor(
    readonly code: LedgerErrorCode,
    message: string,
  ) {
    super(message);
    this.name = 'LedgerError';
  }
}

export interface LedgerOptions {
  /** Clock (epoch ms). Injectable for tests. */
  now?: () => number;
}

// ---- inputs ---------------------------------------------------------------

export interface CreateOptimizationInput {
  id?: string;
  name?: string | null;
  repo: string;
  baseRef: string;
  metric: string;
  direction: Direction;
  minGain: number;
  budgetUsd: number;
  timeLimitS: number;
  configYaml: string;
  status?: OptimizationStatus;
  createdAt?: number;
  pack?: string | null;
  objectiveName?: string | null;
  evaluatorKind?: EvaluatorKind;
  /** `objective.value` from ratchet.yaml; omit when the config has no value model. */
  value?: { usdPerUnit: number; period: ValuePeriod; basis: ValueBasis; unit?: string | null } | null;
  sandboxId?: string | null;
}

export interface RecordSandboxInput {
  id: string;
  provider: SandboxProviderKind;
  region: string;
  accountId?: string | null;
  status?: SandboxStatus;
  createdAt?: number;
}

export interface RecordBaselineInput {
  optimizationId: string;
  samples: number[];
  noiseFloor: number;
  stats: BenchStats;
  /** Headline baseline value; defaults to `stats.value`. */
  value?: number;
  recordedAt?: number;
}

export interface AddAgentInput {
  id?: string;
  optimizationId: string;
  kind: AgentKind;
  branch: string;
  runnerInstanceId?: string | null;
  status?: AgentStatus;
  createdAt?: number;
}

export interface SubmitCandidateInput {
  id?: string;
  optimizationId: string;
  agentId: string;
  sha: string;
  message?: string;
  submittedAt?: number;
}

export interface RecordBenchResultInput {
  candidateId: string;
  phase: BenchPhase;
  samples: number[];
  stats: BenchStats;
  guardrails?: GuardrailResult[];
  startedAt: number;
  finishedAt: number;
}

export interface RecordSpendInput {
  optimizationId: string;
  agentId?: string | null;
  category: SpendCategory;
  usd: number;
  /** True for live estimates, false once reconciled against billing. Default true. */
  estimated?: boolean;
  source: string;
  at?: number;
}

export interface AppendAgentEventInput {
  agentId: string;
  type: string;
  payload?: unknown;
  at?: number;
}

// ---- query results --------------------------------------------------------

/** One row of the Optimizations table. */
export interface OptimizationSummary {
  id: string;
  name: string | null;
  repo: string;
  metric: string;
  direction: Direction;
  status: OptimizationStatus;
  budgetUsd: number;
  createdAt: number;
  startedAt: number | null;
  endedAt: number | null;
  baselineValue: number | null;
  /** Current ratchet value (last verified move, else the baseline); null without a baseline. */
  bestValue: number | null;
  /** Improvement vs baseline in percent; positive = better in the metric's direction. */
  gainPct: number | null;
  spendUsd: number;
  /** Distinct agent kinds in this optimization, sorted. */
  agentKinds: AgentKind[];
  agentCount: number;
  candidateCount: number;
  /** Number of ratchet moves (verified wins). */
  verifiedWins: number;
}

export interface RatchetHistoryEntry {
  id: number;
  optimizationId: string;
  candidateId: string;
  sha: string;
  agentId: string;
  agentKind: AgentKind;
  fromValue: number;
  toValue: number;
  /** Improvement of this step vs `fromValue`, percent, positive = better. */
  gainPct: number;
  at: number;
}

export interface CandidateWithResults extends CandidateRow {
  agentKind: AgentKind;
  agentBranch: string;
  results: BenchResultRow[];
  /** Headline value of the latest `bench` result, if any. */
  benchValue: number | null;
  /** Headline value of the latest `confirm` result, if any. */
  confirmValue: number | null;
  /** True if this candidate moved the ratchet. */
  movedRatchet: boolean;
}

export interface AgentSpend {
  agentId: string | null;
  kind: AgentKind | null;
  branch: string | null;
  total: number;
  byCategory: Record<SpendCategory, number>;
}

export interface SpendBreakdown {
  total: number;
  estimated: number;
  actual: number;
  byCategory: Record<SpendCategory, number>;
  /** Per agent, plus one entry with `agentId: null` for optimization-level spend (if any). */
  byAgent: AgentSpend[];
}

export type ActivityItem =
  | {
      kind: 'verdict';
      at: number;
      optimizationId: string;
      candidateId: string;
      agentId: string;
      agentKind: AgentKind;
      sha: string;
      message: string;
      verdict: CandidateVerdict;
      reasons: string[];
    }
  | {
      kind: 'ratchet_move';
      at: number;
      optimizationId: string;
      candidateId: string;
      agentId: string;
      agentKind: AgentKind;
      sha: string;
      message: string;
      fromValue: number;
      toValue: number;
    };

// ---- helpers --------------------------------------------------------------

const newId = (prefix: string): string => `${prefix}_${randomUUID().replace(/-/g, '').slice(0, 16)}`;

function emptyCategories(): Record<SpendCategory, number> {
  return Object.fromEntries(SPEND_CATEGORIES.map((c) => [c, 0])) as Record<SpendCategory, number>;
}

function assertFinite(name: string, v: number): void {
  if (!Number.isFinite(v)) throw new LedgerError('invalid', `${name} must be a finite number`);
}

/** Strictly better in `direction`? */
export function isImprovement(direction: Direction, from: number, to: number): boolean {
  return direction === 'minimize' ? to < from : to > from;
}

/** Percent improvement of `to` over `from`; positive = better. Null if `from` is 0. */
export function gainPercent(direction: Direction, from: number, to: number): number | null {
  if (from === 0) return null;
  const delta = direction === 'minimize' ? from - to : to - from;
  return (delta / Math.abs(from)) * 100;
}

// ---- Ledger ---------------------------------------------------------------

export class Ledger {
  private readonly now: () => number;

  constructor(
    readonly db: LedgerDb,
    readonly sqlite: SqliteDatabase,
    options: LedgerOptions = {},
  ) {
    this.now = options.now ?? Date.now;
  }

  close(): void {
    this.sqlite.close();
  }

  /** Run `fn` in a single SQLite transaction. */
  transaction<T>(fn: () => T): T {
    return this.sqlite.transaction(fn).immediate();
  }

  // ---- optimizations ------------------------------------------------------

  createOptimization(input: CreateOptimizationInput): OptimizationRow {
    assertFinite('minGain', input.minGain);
    assertFinite('budgetUsd', input.budgetUsd);
    if (input.minGain < 0) throw new LedgerError('invalid', 'minGain must be >= 0');
    if (input.budgetUsd < 0) throw new LedgerError('invalid', 'budgetUsd must be >= 0');
    const row = this.db
      .insert(optimizations)
      .values({
        id: input.id ?? newId('opt'),
        name: input.name ?? null,
        repo: input.repo,
        baseRef: input.baseRef,
        metric: input.metric,
        direction: input.direction,
        minGain: input.minGain,
        budgetUsd: input.budgetUsd,
        timeLimitS: input.timeLimitS,
        status: input.status ?? 'draft',
        createdAt: input.createdAt ?? this.now(),
        configYaml: input.configYaml,
        pack: input.pack ?? null,
        objectiveName: input.objectiveName ?? null,
        evaluatorKind: input.evaluatorKind ?? 'benchmark',
        valueUsdPerUnit: input.value?.usdPerUnit ?? null,
        valuePeriod: input.value?.period ?? null,
        valueBasis: input.value?.basis ?? null,
        valueUnit: input.value?.unit ?? null,
        sandboxId: input.sandboxId ?? null,
      })
      .returning()
      .get();
    return row;
  }

  // ------------------------------------------------------------ sandboxes

  /** Register a sandbox (idempotent on id: an existing row is returned unchanged). */
  recordSandbox(input: RecordSandboxInput): SandboxRow {
    const existing = this.getSandbox(input.id);
    if (existing) return existing;
    return this.db
      .insert(sandboxes)
      .values({
        id: input.id,
        provider: input.provider,
        region: input.region,
        accountId: input.accountId ?? null,
        status: input.status ?? 'planned',
        createdAt: input.createdAt ?? this.now(),
      })
      .returning()
      .get();
  }

  getSandbox(id: string): SandboxRow | undefined {
    return this.db.select().from(sandboxes).where(eq(sandboxes.id, id)).get();
  }

  listSandboxes(): SandboxRow[] {
    return this.db.select().from(sandboxes).orderBy(desc(sandboxes.createdAt)).all();
  }

  /**
   * Move a sandbox through apply/destroy, stamping times. `outputs` is stored on apply (`null` clears them on
   * destroy); `accountId` is filled in once the account is known.
   */
  setSandboxStatus(
    id: string,
    status: SandboxStatus,
    opts: { outputs?: Record<string, unknown> | null; accountId?: string | null; at?: number } = {},
  ): SandboxRow {
    if (!this.getSandbox(id)) throw new LedgerError('not_found', `sandbox ${id} not found`);
    const at = opts.at ?? this.now();
    const patch: Partial<SandboxRow> = { status };
    if (status === 'applied') patch.appliedAt = at;
    if (status === 'destroyed') patch.destroyedAt = at;
    if (opts.outputs !== undefined) patch.outputs = opts.outputs;
    if (opts.accountId !== undefined) patch.accountId = opts.accountId;
    return this.db.update(sandboxes).set(patch).where(eq(sandboxes.id, id)).returning().get();
  }

  /** Optimizations that ran in a sandbox; teardown must not destroy one that is still live. */
  optimizationsInSandbox(sandboxId: string): OptimizationRow[] {
    return this.db.select().from(optimizations).where(eq(optimizations.sandboxId, sandboxId)).all();
  }

  getOptimization(id: string): OptimizationRow | undefined {
    return this.db.select().from(optimizations).where(eq(optimizations.id, id)).get();
  }

  /** Move an optimization through its lifecycle, stamping started/ended times. */
  setOptimizationStatus(id: string, status: OptimizationStatus, at: number = this.now()): OptimizationRow {
    const cur = this.requireOptimization(id);
    const patch: Partial<OptimizationRow> = { status };
    if (status === 'running' && cur.startedAt === null) patch.startedAt = at;
    if (status === 'completed' || status === 'stopped' || status === 'failed') patch.endedAt = at;
    return this.db.update(optimizations).set(patch).where(eq(optimizations.id, id)).returning().get();
  }

  /** Exactly what the Optimizations table shows, newest first. */
  listOptimizations(opts: { status?: OptimizationStatus | OptimizationStatus[] } = {}): OptimizationSummary[] {
    const statuses = opts.status === undefined ? undefined : Array.isArray(opts.status) ? opts.status : [opts.status];
    const rows = this.db
      .select()
      .from(optimizations)
      .where(statuses ? inArray(optimizations.status, statuses) : undefined)
      .orderBy(desc(optimizations.createdAt), desc(optimizations.id))
      .all();
    if (rows.length === 0) return [];

    const baselineByOpt = new Map(
      this.db
        .select({ id: baselines.optimizationId, value: baselines.value })
        .from(baselines)
        .all()
        .map((b) => [b.id, b.value]),
    );

    // last move per optimization = current best
    const lastMoveByOpt = new Map<string, number>();
    const moveCountByOpt = new Map<string, number>();
    for (const m of this.db
      .select({ opt: ratchetMoves.optimizationId, to: ratchetMoves.toValue })
      .from(ratchetMoves)
      .orderBy(asc(ratchetMoves.id))
      .all()) {
      lastMoveByOpt.set(m.opt, m.to);
      moveCountByOpt.set(m.opt, (moveCountByOpt.get(m.opt) ?? 0) + 1);
    }

    const spendByOpt = new Map(
      this.db
        .select({ opt: spendEvents.optimizationId, total: sql<number>`coalesce(sum(${spendEvents.usd}), 0)` })
        .from(spendEvents)
        .groupBy(spendEvents.optimizationId)
        .all()
        .map((s) => [s.opt, s.total]),
    );

    const kindsByOpt = new Map<string, Set<AgentKind>>();
    const agentCountByOpt = new Map<string, number>();
    for (const a of this.db
      .select({ opt: agents.optimizationId, kind: agents.kind })
      .from(agents)
      .all()) {
      if (!kindsByOpt.has(a.opt)) kindsByOpt.set(a.opt, new Set());
      kindsByOpt.get(a.opt)!.add(a.kind);
      agentCountByOpt.set(a.opt, (agentCountByOpt.get(a.opt) ?? 0) + 1);
    }

    const candidateCountByOpt = new Map(
      this.db
        .select({ opt: candidates.optimizationId, n: sql<number>`count(*)` })
        .from(candidates)
        .groupBy(candidates.optimizationId)
        .all()
        .map((c) => [c.opt, c.n]),
    );

    return rows.map((o): OptimizationSummary => {
      const baselineValue = baselineByOpt.get(o.id) ?? null;
      const bestValue = lastMoveByOpt.get(o.id) ?? baselineValue;
      const gainPct =
        baselineValue !== null && bestValue !== null ? gainPercent(o.direction, baselineValue, bestValue) : null;
      return {
        id: o.id,
        name: o.name,
        repo: o.repo,
        metric: o.metric,
        direction: o.direction,
        status: o.status,
        budgetUsd: o.budgetUsd,
        createdAt: o.createdAt,
        startedAt: o.startedAt,
        endedAt: o.endedAt,
        baselineValue,
        bestValue,
        gainPct,
        spendUsd: spendByOpt.get(o.id) ?? 0,
        agentKinds: [...(kindsByOpt.get(o.id) ?? [])].sort(),
        agentCount: agentCountByOpt.get(o.id) ?? 0,
        candidateCount: candidateCountByOpt.get(o.id) ?? 0,
        verifiedWins: moveCountByOpt.get(o.id) ?? 0,
      };
    });
  }

  // ---- baseline -----------------------------------------------------------

  /** Record (or re-record, before any ratchet move) the baseline. */
  recordBaseline(input: RecordBaselineInput): BaselineRow {
    this.requireOptimization(input.optimizationId);
    assertFinite('noiseFloor', input.noiseFloor);
    const value = input.value ?? input.stats.value;
    assertFinite('baseline value', value);
    return this.transaction(() => {
      const moved = this.db
        .select({ id: ratchetMoves.id })
        .from(ratchetMoves)
        .where(eq(ratchetMoves.optimizationId, input.optimizationId))
        .limit(1)
        .get();
      if (moved) throw new LedgerError('invalid', 'cannot change the baseline after the ratchet has moved');
      const values = {
        optimizationId: input.optimizationId,
        samples: input.samples,
        value,
        noiseFloor: input.noiseFloor,
        stats: input.stats,
        recordedAt: input.recordedAt ?? this.now(),
      };
      return this.db
        .insert(baselines)
        .values(values)
        .onConflictDoUpdate({ target: baselines.optimizationId, set: values })
        .returning()
        .get();
    });
  }

  getBaseline(optimizationId: string): BaselineRow | undefined {
    return this.db.select().from(baselines).where(eq(baselines.optimizationId, optimizationId)).get();
  }

  // ---- agents -------------------------------------------------------------

  addAgent(input: AddAgentInput): AgentRow {
    this.requireOptimization(input.optimizationId);
    return this.db
      .insert(agents)
      .values({
        id: input.id ?? newId('agt'),
        optimizationId: input.optimizationId,
        kind: input.kind,
        branch: input.branch,
        runnerInstanceId: input.runnerInstanceId ?? null,
        status: input.status ?? 'pending',
        createdAt: input.createdAt ?? this.now(),
      })
      .returning()
      .get();
  }

  getAgent(id: string): AgentRow | undefined {
    return this.db.select().from(agents).where(eq(agents.id, id)).get();
  }

  listAgents(optimizationId: string): AgentRow[] {
    return this.db
      .select()
      .from(agents)
      .where(eq(agents.optimizationId, optimizationId))
      .orderBy(asc(agents.createdAt), asc(agents.id))
      .all();
  }

  setAgentStatus(id: string, status: AgentStatus, runnerInstanceId?: string | null): AgentRow {
    this.requireAgent(id);
    const patch: Partial<AgentRow> = { status };
    if (runnerInstanceId !== undefined) patch.runnerInstanceId = runnerInstanceId;
    return this.db.update(agents).set(patch).where(eq(agents.id, id)).returning().get();
  }

  // ---- candidates ---------------------------------------------------------

  submitCandidate(input: SubmitCandidateInput): CandidateRow {
    const agent = this.requireAgent(input.agentId);
    if (agent.optimizationId !== input.optimizationId) {
      throw new LedgerError('invalid', `agent ${input.agentId} does not belong to optimization ${input.optimizationId}`);
    }
    return this.db
      .insert(candidates)
      .values({
        id: input.id ?? newId('cand'),
        optimizationId: input.optimizationId,
        agentId: input.agentId,
        sha: input.sha,
        message: input.message ?? '',
        submittedAt: input.submittedAt ?? this.now(),
      })
      .returning()
      .get();
  }

  getCandidate(id: string): CandidateRow | undefined {
    return this.db.select().from(candidates).where(eq(candidates.id, id)).get();
  }

  recordBenchResult(input: RecordBenchResultInput): BenchResultRow {
    this.requireCandidate(input.candidateId);
    assertFinite('stats.value', input.stats.value);
    return this.db
      .insert(benchResults)
      .values({
        candidateId: input.candidateId,
        phase: input.phase,
        samples: input.samples,
        stats: input.stats,
        guardrails: input.guardrails ?? [],
        startedAt: input.startedAt,
        finishedAt: input.finishedAt,
      })
      .returning()
      .get();
  }

  setVerdict(candidateId: string, verdict: CandidateVerdict, reasons: string[] = [], at: number = this.now()): CandidateRow {
    this.requireCandidate(candidateId);
    return this.db
      .update(candidates)
      .set({ verdict, reasons, verdictAt: verdict === 'pending' ? null : at })
      .where(eq(candidates.id, candidateId))
      .returning()
      .get();
  }

  /** All candidates of an optimization (oldest first) with their bench/confirm results. */
  candidatesWithResults(optimizationId: string): CandidateWithResults[] {
    const rows = this.db
      .select({ candidate: candidates, agentKind: agents.kind, agentBranch: agents.branch })
      .from(candidates)
      .innerJoin(agents, eq(agents.id, candidates.agentId))
      .where(eq(candidates.optimizationId, optimizationId))
      .orderBy(asc(candidates.submittedAt), asc(candidates.id))
      .all();
    if (rows.length === 0) return [];

    const ids = rows.map((r) => r.candidate.id);
    const resultsByCand = new Map<string, BenchResultRow[]>();
    for (const r of this.db
      .select()
      .from(benchResults)
      .where(inArray(benchResults.candidateId, ids))
      .orderBy(asc(benchResults.id))
      .all()) {
      const list = resultsByCand.get(r.candidateId) ?? [];
      list.push(r);
      resultsByCand.set(r.candidateId, list);
    }
    const moved = new Set(
      this.db
        .select({ id: ratchetMoves.candidateId })
        .from(ratchetMoves)
        .where(eq(ratchetMoves.optimizationId, optimizationId))
        .all()
        .map((m) => m.id),
    );

    return rows.map(({ candidate, agentKind, agentBranch }) => {
      const results = resultsByCand.get(candidate.id) ?? [];
      const latest = (phase: BenchPhase): number | null => {
        const r = results.filter((x) => x.phase === phase).at(-1);
        return r ? r.stats.value : null;
      };
      return {
        ...candidate,
        agentKind,
        agentBranch,
        results,
        benchValue: latest('bench'),
        confirmValue: latest('confirm'),
        movedRatchet: moved.has(candidate.id),
      };
    });
  }

  // ---- ratchet ------------------------------------------------------------

  /** Current ratchet value: last move's `to_value`, else the baseline, else null. */
  currentBest(optimizationId: string): number | null {
    const last = this.db
      .select({ to: ratchetMoves.toValue })
      .from(ratchetMoves)
      .where(eq(ratchetMoves.optimizationId, optimizationId))
      .orderBy(desc(ratchetMoves.id))
      .limit(1)
      .get();
    if (last) return last.to;
    return this.getBaseline(optimizationId)?.value ?? null;
  }

  /**
   * Advance the ratchet to `toValue` (attributed to `candidateId`). The ratchet only
   * turns one way: rejects a value that is not strictly better than the current best
   * in the optimization's direction.
   */
  moveRatchet(optimizationId: string, candidateId: string, toValue: number, at: number = this.now()): RatchetMoveRow {
    assertFinite('toValue', toValue);
    return this.transaction(() => {
      const opt = this.requireOptimization(optimizationId);
      const cand = this.requireCandidate(candidateId);
      if (cand.optimizationId !== optimizationId) {
        throw new LedgerError('invalid', `candidate ${candidateId} does not belong to optimization ${optimizationId}`);
      }
      const from = this.currentBest(optimizationId);
      if (from === null) throw new LedgerError('no_baseline', `optimization ${optimizationId} has no baseline yet`);
      if (!isImprovement(opt.direction, from, toValue)) {
        throw new LedgerError(
          'ratchet_not_improved',
          `${toValue} is not strictly better than current best ${from} (${opt.direction})`,
        );
      }
      return this.db
        .insert(ratchetMoves)
        .values({ optimizationId, candidateId, fromValue: from, toValue, at })
        .returning()
        .get();
    });
  }

  ratchetHistory(optimizationId: string): RatchetHistoryEntry[] {
    const opt = this.requireOptimization(optimizationId);
    return this.db
      .select({
        id: ratchetMoves.id,
        optimizationId: ratchetMoves.optimizationId,
        candidateId: ratchetMoves.candidateId,
        sha: candidates.sha,
        agentId: candidates.agentId,
        agentKind: agents.kind,
        fromValue: ratchetMoves.fromValue,
        toValue: ratchetMoves.toValue,
        at: ratchetMoves.at,
      })
      .from(ratchetMoves)
      .innerJoin(candidates, eq(candidates.id, ratchetMoves.candidateId))
      .innerJoin(agents, eq(agents.id, candidates.agentId))
      .where(eq(ratchetMoves.optimizationId, optimizationId))
      .orderBy(asc(ratchetMoves.id))
      .all()
      .map((m) => ({ ...m, gainPct: gainPercent(opt.direction, m.fromValue, m.toValue) ?? 0 }));
  }

  // ---- spend --------------------------------------------------------------

  recordSpend(input: RecordSpendInput): SpendEventRow {
    assertFinite('usd', input.usd);
    this.requireOptimization(input.optimizationId);
    if (input.agentId) {
      const agent = this.requireAgent(input.agentId);
      if (agent.optimizationId !== input.optimizationId) {
        throw new LedgerError('invalid', `agent ${input.agentId} does not belong to optimization ${input.optimizationId}`);
      }
    }
    return this.db
      .insert(spendEvents)
      .values({
        optimizationId: input.optimizationId,
        agentId: input.agentId ?? null,
        category: input.category,
        usd: input.usd,
        estimated: input.estimated ?? true,
        source: input.source,
        at: input.at ?? this.now(),
      })
      .returning()
      .get();
  }

  totalSpend(optimizationId: string): number {
    const r = this.db
      .select({ total: sql<number>`coalesce(sum(${spendEvents.usd}), 0)` })
      .from(spendEvents)
      .where(eq(spendEvents.optimizationId, optimizationId))
      .get();
    return r?.total ?? 0;
  }

  spendBreakdown(optimizationId: string): SpendBreakdown {
    const rows = this.db
      .select({
        agentId: spendEvents.agentId,
        category: spendEvents.category,
        estimated: spendEvents.estimated,
        total: sql<number>`sum(${spendEvents.usd})`,
      })
      .from(spendEvents)
      .where(eq(spendEvents.optimizationId, optimizationId))
      .groupBy(spendEvents.agentId, spendEvents.category, spendEvents.estimated)
      .all();

    const out: SpendBreakdown = { total: 0, estimated: 0, actual: 0, byCategory: emptyCategories(), byAgent: [] };
    const perAgent = new Map<string | null, AgentSpend>();
    const agentInfo = new Map(this.listAgents(optimizationId).map((a) => [a.id, a]));

    for (const r of rows) {
      out.total += r.total;
      if (r.estimated) out.estimated += r.total;
      else out.actual += r.total;
      out.byCategory[r.category] += r.total;

      let entry = perAgent.get(r.agentId);
      if (!entry) {
        const info = r.agentId ? agentInfo.get(r.agentId) : undefined;
        entry = {
          agentId: r.agentId,
          kind: info?.kind ?? null,
          branch: info?.branch ?? null,
          total: 0,
          byCategory: emptyCategories(),
        };
        perAgent.set(r.agentId, entry);
      }
      entry.total += r.total;
      entry.byCategory[r.category] += r.total;
    }
    // agents first (creation order), optimization-level spend last
    for (const a of agentInfo.values()) {
      const e = perAgent.get(a.id);
      if (e) out.byAgent.push(e);
    }
    const shared = perAgent.get(null);
    if (shared) out.byAgent.push(shared);
    return out;
  }

  // ---- activity & agent events -------------------------------------------

  /** Merged, newest-first feed of verdicts and ratchet moves. */
  recentActivity(limit = 20, opts: { optimizationId?: string } = {}): ActivityItem[] {
    if (limit <= 0) return [];
    const optId = opts.optimizationId;

    const verdicts = this.db
      .select({ c: candidates, agentKind: agents.kind })
      .from(candidates)
      .innerJoin(agents, eq(agents.id, candidates.agentId))
      .where(and(ne(candidates.verdict, 'pending'), optId ? eq(candidates.optimizationId, optId) : undefined))
      .orderBy(desc(candidates.verdictAt), desc(candidates.id))
      .limit(limit)
      .all()
      .map(
        ({ c, agentKind }): ActivityItem => ({
          kind: 'verdict',
          at: c.verdictAt ?? c.submittedAt,
          optimizationId: c.optimizationId,
          candidateId: c.id,
          agentId: c.agentId,
          agentKind,
          sha: c.sha,
          message: c.message,
          verdict: c.verdict,
          reasons: c.reasons,
        }),
      );

    const moves = this.db
      .select({ m: ratchetMoves, c: candidates, agentKind: agents.kind })
      .from(ratchetMoves)
      .innerJoin(candidates, eq(candidates.id, ratchetMoves.candidateId))
      .innerJoin(agents, eq(agents.id, candidates.agentId))
      .where(optId ? eq(ratchetMoves.optimizationId, optId) : undefined)
      .orderBy(desc(ratchetMoves.at), desc(ratchetMoves.id))
      .limit(limit)
      .all()
      .map(
        ({ m, c, agentKind }): ActivityItem => ({
          kind: 'ratchet_move',
          at: m.at,
          optimizationId: m.optimizationId,
          candidateId: c.id,
          agentId: c.agentId,
          agentKind,
          sha: c.sha,
          message: c.message,
          fromValue: m.fromValue,
          toValue: m.toValue,
        }),
      );

    // Newest first; on a tie a ratchet move precedes the verdict that caused it.
    return [...verdicts, ...moves]
      .sort((a, b) => b.at - a.at || (a.kind === b.kind ? 0 : a.kind === 'ratchet_move' ? -1 : 1))
      .slice(0, limit);
  }

  appendAgentEvent(input: AppendAgentEventInput): AgentEventRow {
    this.requireAgent(input.agentId);
    return this.db
      .insert(agentEvents)
      .values({
        agentId: input.agentId,
        at: input.at ?? this.now(),
        type: input.type,
        payload: input.payload ?? null,
      })
      .returning()
      .get();
  }

  /** Events with id > `sinceId` (default: all), oldest first. Poll with the last seen id. */
  agentEvents(agentId: string, sinceId = 0, limit = 500): AgentEventRow[] {
    return this.db
      .select()
      .from(agentEvents)
      .where(and(eq(agentEvents.agentId, agentId), gt(agentEvents.id, sinceId)))
      .orderBy(asc(agentEvents.id))
      .limit(limit)
      .all();
  }

  // ---- guards -------------------------------------------------------------

  private requireOptimization(id: string): OptimizationRow {
    const row = this.getOptimization(id);
    if (!row) throw new LedgerError('not_found', `optimization ${id} not found`);
    return row;
  }

  private requireAgent(id: string): AgentRow {
    const row = this.getAgent(id);
    if (!row) throw new LedgerError('not_found', `agent ${id} not found`);
    return row;
  }

  private requireCandidate(id: string): CandidateRow {
    const row = this.getCandidate(id);
    if (!row) throw new LedgerError('not_found', `candidate ${id} not found`);
    return row;
  }
}
