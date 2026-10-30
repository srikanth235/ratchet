/**
 * Ledger schema (SQLite via drizzle). The ledger is the single source of truth
 * for an optimization. `ratchet_moves` and `spend_events` are append-only,
 * enforced by triggers at the end of migrations/0000_init.sql.
 * Pre-release: there is ONE migration. Edit the schema, regenerate 0000_init.sql and re-append the triggers.
 *
 * Conventions:
 *  - timestamps are integer epoch milliseconds;
 *  - JSON columns are TEXT holding JSON (typed through `$type`);
 *  - money is USD as REAL.
 */
import { sql, type SQL } from 'drizzle-orm';
import {
  check,
  index,
  integer,
  real,
  sqliteTable,
  text,
  uniqueIndex,
  type AnySQLiteColumn,
} from 'drizzle-orm/sqlite-core';

export const DIRECTIONS = ['minimize', 'maximize'] as const;
import { EVALUATOR_KINDS, type Direction, type EvaluatorKind } from '../config/schema.js';
export type { Direction, EvaluatorKind };
export { EVALUATOR_KINDS };

export const VALUE_PERIODS = ['month', 'year'] as const;
export type ValuePeriod = (typeof VALUE_PERIODS)[number];
export const VALUE_BASES = ['estimated', 'measured'] as const;
export type ValueBasis = (typeof VALUE_BASES)[number];

export const SANDBOX_PROVIDERS = ['aws', 'mock'] as const;
export type SandboxProviderKind = (typeof SANDBOX_PROVIDERS)[number];
export const SANDBOX_STATUSES = ['planned', 'applying', 'applied', 'destroying', 'destroyed', 'failed'] as const;
export type SandboxStatus = (typeof SANDBOX_STATUSES)[number];

export const OPTIMIZATION_STATUSES = [
  'draft',
  'baselining',
  'running',
  'stopping',
  'completed',
  'stopped',
  'failed',
] as const;
export type OptimizationStatus = (typeof OPTIMIZATION_STATUSES)[number];

export const AGENT_KINDS = ['claude-code', 'codex', 'mock'] as const;
export type AgentKind = (typeof AGENT_KINDS)[number];

export const AGENT_STATUSES = [
  'pending',
  'launching',
  'running',
  'paused',
  'stopped',
  'done',
  'failed',
] as const;
export type AgentStatus = (typeof AGENT_STATUSES)[number];

/** `pending` until the bench runner decides; the rest mirror the ratchet rule (§3). */
export const VERDICTS = ['pending', 'verified', 'rejected', 'failed_confirm', 'blocked'] as const;
export type CandidateVerdict = (typeof VERDICTS)[number];

export const BENCH_PHASES = ['bench', 'confirm'] as const;
export type BenchPhase = (typeof BENCH_PHASES)[number];

export const SPEND_CATEGORIES = [
  'agent_runner',
  'experiment',
  'bench_runner',
  'storage_egress',
  'tokens',
] as const;
export type SpendCategory = (typeof SPEND_CATEGORIES)[number];

/** Summary statistics of a benchmark run set. `value` is the headline number the ratchet compares. */
export type BenchStats = { value: number } & Record<string, number>;

/** Per-guardrail outcome stored on a bench result (same shape the ratchet rule consumes). */
import type { GuardrailResult } from '../config/guardrails.js';
export type { GuardrailResult };

const inList = (col: AnySQLiteColumn, values: readonly string[]): SQL =>
  sql`${col} in (${sql.raw(values.map((v) => `'${v}'`).join(', '))})`;

/** One cloud sandbox (an applied infra/aws module). Optimizations run inside one. */
export const sandboxes = sqliteTable(
  'sandboxes',
  {
    id: text('id').primaryKey(),
    provider: text('provider', { enum: SANDBOX_PROVIDERS }).notNull(),
    region: text('region').notNull(),
    accountId: text('account_id'),
    status: text('status', { enum: SANDBOX_STATUSES }).notNull().default('planned'),
    /** Contract outputs after apply (see infra/contract.md); null until applied. */
    outputs: text('outputs', { mode: 'json' }).$type<Record<string, unknown>>(),
    createdAt: integer('created_at').notNull(),
    appliedAt: integer('applied_at'),
    destroyedAt: integer('destroyed_at'),
  },
  (t) => [
    check('sandboxes_provider_check', inList(t.provider, SANDBOX_PROVIDERS)),
    check('sandboxes_status_check', inList(t.status, SANDBOX_STATUSES)),
  ],
);

export const optimizations = sqliteTable(
  'optimizations',
  {
    id: text('id').primaryKey(),
    name: text('name'),
    repo: text('repo').notNull(),
    baseRef: text('base_ref').notNull(),
    metric: text('metric').notNull(),
    direction: text('direction', { enum: DIRECTIONS }).notNull(),
    /** Minimum relative gain, e.g. 0.03 = 3%. */
    minGain: real('min_gain').notNull(),
    budgetUsd: real('budget_usd').notNull(),
    timeLimitS: integer('time_limit_s').notNull(),
    status: text('status', { enum: OPTIMIZATION_STATUSES }).notNull().default('draft'),
    createdAt: integer('created_at').notNull(),
    startedAt: integer('started_at'),
    endedAt: integer('ended_at'),
    /** Snapshot of ratchet.yaml at creation time. */
    configYaml: text('config_yaml').notNull(),
    /** Domain pack (`perf`, `forecasting`, …) and the objective's display name, from ratchet.yaml. */
    pack: text('pack'),
    objectiveName: text('objective_name'),
    evaluatorKind: text('evaluator_kind', { enum: EVALUATOR_KINDS }).notNull().default('benchmark'),
    /** `objective.value`: USD per unit of metric improvement per period. All null when no value model is set. */
    valueUsdPerUnit: real('value_usd_per_unit'),
    valuePeriod: text('value_period', { enum: VALUE_PERIODS }),
    valueBasis: text('value_basis', { enum: VALUE_BASES }),
    valueUnit: text('value_unit'),
    sandboxId: text('sandbox_id').references(() => sandboxes.id),
  },
  (t) => [
    index('optimizations_sandbox_idx').on(t.sandboxId),
    check('optimizations_direction_check', inList(t.direction, DIRECTIONS)),
    check('optimizations_status_check', inList(t.status, OPTIMIZATION_STATUSES)),
    check('optimizations_evaluator_kind_check', inList(t.evaluatorKind, EVALUATOR_KINDS)),
    check('optimizations_value_period_check', sql`${t.valuePeriod} is null or ${inList(t.valuePeriod, VALUE_PERIODS)}`),
    check('optimizations_value_basis_check', sql`${t.valueBasis} is null or ${inList(t.valueBasis, VALUE_BASES)}`),
    check('optimizations_value_usd_check', sql`${t.valueUsdPerUnit} is null or ${t.valueUsdPerUnit} > 0`),
  ],
);

export const baselines = sqliteTable('baselines', {
  optimizationId: text('optimization_id')
    .primaryKey()
    .references(() => optimizations.id),
  samples: text('samples', { mode: 'json' }).$type<number[]>().notNull(),
  /** The baseline headline value (the ratchet starts here). */
  value: real('value').notNull(),
  noiseFloor: real('noise_floor').notNull(),
  stats: text('stats', { mode: 'json' }).$type<BenchStats>().notNull(),
  recordedAt: integer('recorded_at').notNull(),
});

export const agents = sqliteTable(
  'agents',
  {
    id: text('id').primaryKey(),
    optimizationId: text('optimization_id')
      .notNull()
      .references(() => optimizations.id),
    kind: text('kind', { enum: AGENT_KINDS }).notNull(),
    branch: text('branch').notNull(),
    runnerInstanceId: text('runner_instance_id'),
    status: text('status', { enum: AGENT_STATUSES }).notNull().default('pending'),
    createdAt: integer('created_at').notNull(),
  },
  (t) => [
    index('agents_optimization_idx').on(t.optimizationId),
    check('agents_kind_check', inList(t.kind, AGENT_KINDS)),
    check('agents_status_check', inList(t.status, AGENT_STATUSES)),
  ],
);

export const candidates = sqliteTable(
  'candidates',
  {
    id: text('id').primaryKey(),
    optimizationId: text('optimization_id')
      .notNull()
      .references(() => optimizations.id),
    agentId: text('agent_id')
      .notNull()
      .references(() => agents.id),
    sha: text('sha').notNull(),
    message: text('message').notNull().default(''),
    submittedAt: integer('submitted_at').notNull(),
    verdict: text('verdict', { enum: VERDICTS }).notNull().default('pending'),
    /** When the verdict was set (null while pending). */
    verdictAt: integer('verdict_at'),
    reasons: text('reasons', { mode: 'json' }).$type<string[]>().notNull().default(sql`'[]'`),
  },
  (t) => [
    index('candidates_optimization_idx').on(t.optimizationId, t.submittedAt),
    index('candidates_agent_idx').on(t.agentId),
    uniqueIndex('candidates_agent_sha_unique').on(t.agentId, t.sha),
    check('candidates_verdict_check', inList(t.verdict, VERDICTS)),
  ],
);

export const benchResults = sqliteTable(
  'bench_results',
  {
    id: integer('id').primaryKey({ autoIncrement: true }),
    candidateId: text('candidate_id')
      .notNull()
      .references(() => candidates.id),
    phase: text('phase', { enum: BENCH_PHASES }).notNull(),
    samples: text('samples', { mode: 'json' }).$type<number[]>().notNull(),
    stats: text('stats', { mode: 'json' }).$type<BenchStats>().notNull(),
    guardrails: text('guardrails', { mode: 'json' })
      .$type<GuardrailResult[]>()
      .notNull()
      .default(sql`'[]'`),
    startedAt: integer('started_at').notNull(),
    finishedAt: integer('finished_at').notNull(),
  },
  (t) => [
    index('bench_results_candidate_idx').on(t.candidateId),
    check('bench_results_phase_check', inList(t.phase, BENCH_PHASES)),
  ],
);

/** Append-only (UPDATE/DELETE raise). Each row is a verified step of the ratchet. */
export const ratchetMoves = sqliteTable(
  'ratchet_moves',
  {
    id: integer('id').primaryKey({ autoIncrement: true }),
    optimizationId: text('optimization_id')
      .notNull()
      .references(() => optimizations.id),
    candidateId: text('candidate_id')
      .notNull()
      .references(() => candidates.id),
    fromValue: real('from_value').notNull(),
    toValue: real('to_value').notNull(),
    at: integer('at').notNull(),
  },
  (t) => [
    index('ratchet_moves_optimization_idx').on(t.optimizationId, t.id),
    uniqueIndex('ratchet_moves_candidate_unique').on(t.candidateId),
  ],
);

/** Append-only (UPDATE/DELETE raise). Corrections are written as new (possibly negative) rows. */
export const spendEvents = sqliteTable(
  'spend_events',
  {
    id: integer('id').primaryKey({ autoIncrement: true }),
    optimizationId: text('optimization_id')
      .notNull()
      .references(() => optimizations.id),
    agentId: text('agent_id').references(() => agents.id),
    category: text('category', { enum: SPEND_CATEGORIES }).notNull(),
    usd: real('usd').notNull(),
    estimated: integer('estimated', { mode: 'boolean' }).notNull().default(true),
    source: text('source').notNull(),
    at: integer('at').notNull(),
  },
  (t) => [
    index('spend_events_optimization_idx').on(t.optimizationId),
    check('spend_events_category_check', inList(t.category, SPEND_CATEGORIES)),
  ],
);

/** Live log of an agent's parsed event stream. */
export const agentEvents = sqliteTable(
  'agent_events',
  {
    id: integer('id').primaryKey({ autoIncrement: true }),
    agentId: text('agent_id')
      .notNull()
      .references(() => agents.id),
    at: integer('at').notNull(),
    type: text('type').notNull(),
    payload: text('payload', { mode: 'json' }).$type<unknown>(),
  },
  (t) => [index('agent_events_agent_idx').on(t.agentId, t.id)],
);

/** Engine bookkeeping that must survive a crash (stop mode, plateau counters, …). Keys are namespaced `opt:<id>:…`. */
export const engineKv = sqliteTable('engine_kv', {
  key: text('key').primaryKey(),
  value: text('value').notNull(),
});

export type SandboxRow = typeof sandboxes.$inferSelect;
export type OptimizationRow = typeof optimizations.$inferSelect;
export type BaselineRow = typeof baselines.$inferSelect;
export type AgentRow = typeof agents.$inferSelect;
export type CandidateRow = typeof candidates.$inferSelect;
export type BenchResultRow = typeof benchResults.$inferSelect;
export type RatchetMoveRow = typeof ratchetMoves.$inferSelect;
export type SpendEventRow = typeof spendEvents.$inferSelect;
export type AgentEventRow = typeof agentEvents.$inferSelect;
