import {
  evaluateRatchet,
  summarize,
  type BenchOutcome,
  type BenchStats,
  type CandidateVerdict,
  type GuardrailResult,
  type ProtectedViolation,
  type RatchetConfig,
  type RatchetOutcome,
  type Verdict,
} from '@ratchet/core';
import { medianMetrics } from './flatten.js';

/** A bench result in the shape of `LedgerStore.recordBenchResult` (minus `candidateId`). */
export interface BenchResultRecord {
  phase: 'bench' | 'confirm';
  samples: number[];
  stats: BenchStats;
  guardrails: GuardrailResult[];
  startedAt: number;
  finishedAt: number;
}

/**
 * Stats stored with a bench result: `value` (the median, the number the ratchet compares), the
 * usual summary (`n`, `mean`, `median`, `stddev`, `cv`, `min`, `max`) and every other metric's
 * median under `metrics.<key>` (e.g. `metrics.peak_rss_mb`). Returns undefined without samples.
 */
export function benchStats(outcome: Pick<BenchOutcome, 'samples' | 'runs'>): BenchStats | undefined {
  if (outcome.samples.length === 0) return undefined;
  const s = summarize(outcome.samples);
  const stats: BenchStats = { value: s.median, n: s.n, mean: s.mean, median: s.median, stddev: s.stddev, cv: s.cv, min: s.min, max: s.max };
  for (const [k, v] of Object.entries(medianMetrics(outcome.runs))) stats[`metrics.${k}`] = v;
  // JSON cannot carry Infinity (cv with mean 0)
  for (const k of Object.keys(stats)) if (!Number.isFinite(stats[k]!)) delete stats[k];
  return stats;
}

/** Inverse of the `metrics.<key>` entries of `benchStats`: the baseline metrics to hand to `BenchRunner.run`. */
export function metricsFromStats(stats: Readonly<Record<string, number>>): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(stats)) if (k.startsWith('metrics.')) out[k.slice('metrics.'.length)] = v;
  return out;
}

export function benchResultRecord(phase: 'bench' | 'confirm', outcome: BenchOutcome): BenchResultRecord | undefined {
  const stats = benchStats(outcome);
  if (!stats) return undefined;
  return { phase, samples: [...outcome.samples], stats, guardrails: outcome.guardrails, startedAt: outcome.startedAt, finishedAt: outcome.finishedAt };
}

export interface EvaluateCandidateInput {
  config: Pick<RatchetConfig, 'objective'>;
  /** Current best verified value on the bench. */
  currentBest: number;
  /** Baseline noise floor in metric units (core's `noiseFloor`). */
  noiseFloor: number;
  /** The current best's value on the hidden trace; defaults to `currentBest` (see core's RatchetInput). */
  confirmBest?: number | undefined;
  bench: BenchOutcome;
  /** Present once the confirm run has completed. */
  confirm?: BenchOutcome | null | undefined;
  protectedViolations: readonly ProtectedViolation[];
}

export interface CandidateEvaluation {
  /** The rule's verdict, including `pending_confirm`. */
  verdict: Verdict;
  /** The value to store on the candidate row (`pending_confirm` is stored as `pending`). */
  ledgerVerdict: CandidateVerdict;
  /** True when the bench win must now be confirmed on the hidden trace on a fresh instance. */
  needsConfirm: boolean;
  /** True iff the ratchet may advance. */
  verified: boolean;
  reasons: string[];
  /** Raw rule outcome. `candidateValue`/`improvement` are NaN when the bench produced no samples. */
  outcome: RatchetOutcome;
  /** Guardrails of the bench run followed by those of the confirm run. */
  guardrails: GuardrailResult[];
  /** Rows to store with `LedgerStore.recordBenchResult` (candidateId added by the caller); absent runs are omitted. */
  results: BenchResultRecord[];
}

/**
 * Compose core's `evaluateRatchet` with bench outcomes. Any failed guardrail (from either run) or
 * protected-path violation blocks. A bench that produced no samples (it never ran because a
 * command guardrail failed, or a run crashed) is blocked by its failed guardrail without calling
 * the rule (which needs at least one sample).
 */
export function evaluateCandidate(input: EvaluateCandidateInput): CandidateEvaluation {
  const { bench, confirm } = input;
  const guardrails = [...bench.guardrails, ...(confirm?.guardrails ?? [])];
  const results = [benchResultRecord('bench', bench), confirm ? benchResultRecord('confirm', confirm) : undefined].filter(
    (r): r is BenchResultRecord => r !== undefined,
  );

  let outcome: RatchetOutcome;
  if (bench.samples.length === 0) {
    const reasons: string[] = [];
    for (const g of guardrails) if (!g.passed) reasons.push(`guardrail failed: ${g.name}${g.detail ? ` (${g.detail})` : ''}`);
    for (const v of input.protectedViolations) reasons.push(`protected path changed: ${v.path} (matches ${v.pattern})`);
    if (reasons.length === 0) reasons.push('the benchmark produced no samples');
    outcome = {
      verdict: 'blocked',
      reasons,
      candidateValue: NaN,
      improvement: NaN,
      requiredGain: Math.max(input.noiseFloor, input.config.objective.min_gain * Math.abs(input.currentBest)),
    };
  } else {
    outcome = evaluateRatchet({
      direction: input.config.objective.direction,
      minGain: input.config.objective.min_gain,
      noiseFloor: input.noiseFloor,
      currentBest: input.currentBest,
      candidate: bench.samples,
      confirm: confirm && confirm.samples.length > 0 ? confirm.samples : undefined,
      confirmBest: input.confirmBest,
      guardrails,
      protectedViolations: input.protectedViolations,
    });
  }
  const verdict = outcome.verdict;
  return {
    verdict,
    ledgerVerdict: verdict === 'pending_confirm' ? 'pending' : verdict,
    needsConfirm: verdict === 'pending_confirm',
    verified: verdict === 'verified',
    reasons: outcome.reasons,
    outcome,
    guardrails,
    results,
  };
}

