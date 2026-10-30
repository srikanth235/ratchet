import { median } from '../bench/stats.js';
import type { GuardrailResult } from '../config/guardrails.js';
import type { ProtectedViolation } from '../protected.js';

export type Verdict =
  | 'verified' //        all gates passed; the ratchet may advance
  | 'rejected' //        guardrails ok, but no gain beyond the threshold
  | 'failed_confirm' //  bench win that did not hold on the hidden trace
  | 'blocked' //         a guardrail failed or a protected path was touched
  | 'pending_confirm'; // bench win, waiting for the confirm run

export interface RatchetInput {
  direction: 'minimize' | 'maximize';
  /** Fraction of the current best, e.g. 0.03 for 3%. */
  minGain: number;
  /** Absolute noise floor in metric units (see `noiseFloor` in bench/stats). */
  noiseFloor: number;
  /** Current best verified value on the bench (e.g. median of the best's bench runs). */
  currentBest: number;
  /** The candidate's N bench samples. */
  candidate: readonly number[];
  /** Samples from the confirm run on a fresh instance against the hidden trace. Absent/empty => not yet run. */
  confirm?: readonly number[] | undefined;
  /**
   * The current best's value on the hidden trace. Defaults to `currentBest`, which is only right
   * when bench and hidden trace share a scale.
   */
  confirmBest?: number | undefined;
  guardrails: readonly GuardrailResult[];
  protectedViolations: readonly ProtectedViolation[];
}

export interface RatchetOutcome {
  verdict: Verdict;
  reasons: string[];
  /** Candidate's central value (median of its bench samples). */
  candidateValue: number;
  /** Signed improvement over currentBest in the objective's direction (positive = better). */
  improvement: number;
  /** max(noiseFloor, minGain * |currentBest|). Improvement must be strictly greater. */
  requiredGain: number;
  /** Signed improvement of the confirm run over confirmBest, when a confirm run exists. */
  confirmImprovement?: number;
}

/** Improvement in the direction of the objective: positive means better. */
export function improvementOf(direction: 'minimize' | 'maximize', best: number, value: number): number {
  return direction === 'minimize' ? best - value : value - best;
}

function checkFinite(name: string, v: number): void {
  if (!Number.isFinite(v)) throw new RangeError(`${name} must be a finite number, got ${v}`);
}

/**
 * The Ratchet rule (V0_ARCHITECTURE.md section 3), as a pure function.
 *
 * Gates, in precedence order:
 *  1. blocked: any failed guardrail or any protected-path violation. This wins over everything,
 *     including a spectacular bench result. All blocking reasons are listed.
 *  2. rejected: improvement of the candidate's median over currentBest is not STRICTLY greater than
 *     requiredGain = max(noiseFloor, minGain * |currentBest|). Ties at the threshold are rejected.
 *  3. pending_confirm: bench win, but no confirm samples yet.
 *  4. failed_confirm: confirm median does not strictly beat confirmBest (default: currentBest).
 *  5. verified.
 *
 * Central value of a sample set is its median (robust to a single outlier run).
 * Throws RangeError on empty candidate samples or non-finite inputs; those are caller bugs.
 */
export function evaluateRatchet(input: RatchetInput): RatchetOutcome {
  const { direction, minGain, noiseFloor, currentBest } = input;
  checkFinite('currentBest', currentBest);
  if (!(minGain >= 0) || !Number.isFinite(minGain)) throw new RangeError('minGain must be a finite number >= 0');
  if (!(noiseFloor >= 0) || !Number.isFinite(noiseFloor)) {
    throw new RangeError('noiseFloor must be a finite number >= 0');
  }
  if (input.candidate.length === 0) throw new RangeError('candidate must have at least one sample');

  const candidateValue = median(input.candidate);
  const improvement = improvementOf(direction, currentBest, candidateValue);
  const requiredGain = Math.max(noiseFloor, minGain * Math.abs(currentBest));
  const base = { candidateValue, improvement, requiredGain };

  const blockers: string[] = [];
  for (const g of input.guardrails) {
    if (!g.passed) blockers.push(`guardrail failed: ${g.name}${g.detail ? ` (${g.detail})` : ''}`);
  }
  for (const v of input.protectedViolations) {
    blockers.push(`protected path changed: ${v.path} (matches ${v.pattern})`);
  }
  if (blockers.length > 0) return { ...base, verdict: 'blocked', reasons: blockers };

  const fmt = (n: number) => String(Number(n.toPrecision(6)));
  if (!(improvement > requiredGain)) {
    const why =
      improvement <= 0
        ? `candidate ${fmt(candidateValue)} is not better than current best ${fmt(currentBest)} (${direction})`
        : `improvement ${fmt(improvement)} does not exceed required gain ${fmt(requiredGain)} ` +
          `(max of noise floor ${fmt(noiseFloor)} and min_gain ${fmt(minGain * Math.abs(currentBest))})`;
    return { ...base, verdict: 'rejected', reasons: [why] };
  }

  const win = `bench improvement ${fmt(improvement)} exceeds required gain ${fmt(requiredGain)}`;
  if (!input.confirm || input.confirm.length === 0) {
    return { ...base, verdict: 'pending_confirm', reasons: [win, 'confirm run on the hidden trace has not completed'] };
  }

  const confirmBest = input.confirmBest ?? currentBest;
  checkFinite('confirmBest', confirmBest);
  const confirmValue = median(input.confirm);
  const confirmImprovement = improvementOf(direction, confirmBest, confirmValue);
  if (!(confirmImprovement > 0)) {
    return {
      ...base,
      confirmImprovement,
      verdict: 'failed_confirm',
      reasons: [
        win,
        `confirm run ${fmt(confirmValue)} does not beat current best ${fmt(confirmBest)} on the hidden trace (${direction})`,
      ],
    };
  }
  return {
    ...base,
    confirmImprovement,
    verdict: 'verified',
    reasons: [win, `confirm run ${fmt(confirmValue)} beats current best ${fmt(confirmBest)} on the hidden trace`],
  };
}
