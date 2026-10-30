import { describe, expect, it } from 'vitest';
import { evaluateRatchet, improvementOf, type RatchetInput } from './rule.js';

const ok = { name: 'pytest -q', passed: true };
const bad = { name: 'pytest -q', passed: false, detail: 'exited with status 1' };

/** Minimize, best 100, min_gain 3% (=3), noise floor 1  => required gain 3. */
const base: RatchetInput = {
  direction: 'minimize',
  minGain: 0.03,
  noiseFloor: 1,
  currentBest: 100,
  candidate: [90, 90, 90],
  guardrails: [ok],
  protectedViolations: [],
};
const run = (over: Partial<RatchetInput>) => evaluateRatchet({ ...base, ...over });

describe('improvementOf', () => {
  it('is positive when better in the objective direction', () => {
    expect(improvementOf('minimize', 100, 90)).toBe(10);
    expect(improvementOf('minimize', 100, 110)).toBe(-10);
    expect(improvementOf('maximize', 100, 110)).toBe(10);
    expect(improvementOf('maximize', 100, 90)).toBe(-10);
  });
});

describe('evaluateRatchet: minimize', () => {
  it('pending_confirm when the bench wins and there is no confirm run', () => {
    const r = run({});
    expect(r.verdict).toBe('pending_confirm');
    expect(r.improvement).toBe(10);
    expect(r.requiredGain).toBe(3);
    expect(r.candidateValue).toBe(90);
    expect(r.reasons.join(' ')).toMatch(/not completed/);
  });

  it('treats an empty confirm array as missing', () => {
    expect(run({ confirm: [] }).verdict).toBe('pending_confirm');
  });

  it('verified when the confirm run also beats the best', () => {
    const r = run({ confirm: [92, 91, 93] });
    expect(r.verdict).toBe('verified');
    expect(r.confirmImprovement).toBe(8);
  });

  it('failed_confirm when the confirm run does not beat the best', () => {
    const r = run({ confirm: [105, 104, 106] });
    expect(r.verdict).toBe('failed_confirm');
    expect(r.confirmImprovement).toBe(-5);
    expect(r.reasons.join(' ')).toMatch(/hidden trace/);
  });

  it('failed_confirm on a tie with the best on the hidden trace', () => {
    expect(run({ confirm: [100] }).verdict).toBe('failed_confirm');
  });

  it('confirmBest overrides currentBest for the confirm comparison', () => {
    // hidden trace is on a different scale: best scored 500 there
    expect(run({ confirm: [480], confirmBest: 500 }).verdict).toBe('verified');
    expect(run({ confirm: [480] }).verdict).toBe('failed_confirm'); // would wrongly compare to 100
    expect(run({ confirm: [510], confirmBest: 500 }).verdict).toBe('failed_confirm');
  });

  it('rejected when the candidate is worse or equal', () => {
    const worse = run({ candidate: [110] });
    expect(worse.verdict).toBe('rejected');
    expect(worse.improvement).toBe(-10);
    expect(worse.reasons[0]).toMatch(/not better/);
    expect(run({ candidate: [100] }).verdict).toBe('rejected');
  });

  it('rejected when better but within the required gain; ties at the threshold are rejected', () => {
    expect(run({ candidate: [98] }).verdict).toBe('rejected'); // 2 < 3
    expect(run({ candidate: [97] }).verdict).toBe('rejected'); // exactly 3 == required: not strictly greater
    expect(run({ candidate: [96.9] }).verdict).toBe('pending_confirm'); // 3.1 > 3
    expect(run({ candidate: [98] }).reasons[0]).toMatch(/does not exceed required gain/);
  });

  it('uses the median of candidate samples, so one outlier run does not decide', () => {
    expect(run({ candidate: [90, 90, 200] }).verdict).toBe('pending_confirm'); // median 90
    expect(run({ candidate: [90, 200, 200] }).verdict).toBe('rejected'); // median 200
  });
});

describe('evaluateRatchet: maximize', () => {
  const max = { direction: 'maximize' as const };

  it('higher is better', () => {
    const r = run({ ...max, candidate: [110] });
    expect(r.verdict).toBe('pending_confirm');
    expect(r.improvement).toBe(10);
  });

  it('lower candidate is rejected', () => {
    expect(run({ ...max, candidate: [90] }).verdict).toBe('rejected');
  });

  it('ties at the threshold', () => {
    expect(run({ ...max, candidate: [103] }).verdict).toBe('rejected');
    expect(run({ ...max, candidate: [103.1] }).verdict).toBe('pending_confirm');
  });

  it('confirm must be higher than best', () => {
    expect(run({ ...max, candidate: [110], confirm: [111] }).verdict).toBe('verified');
    expect(run({ ...max, candidate: [110], confirm: [99] }).verdict).toBe('failed_confirm');
  });
});

describe('evaluateRatchet: threshold = max(noise_floor, min_gain x best)', () => {
  it('noise larger than min_gain dominates', () => {
    // min_gain*best = 3, noise = 8 => required 8
    const noisy = { noiseFloor: 8 };
    expect(run({ ...noisy, candidate: [93] }).verdict).toBe('rejected'); // 7 < 8
    expect(run({ ...noisy, candidate: [92] }).verdict).toBe('rejected'); // tie at 8
    expect(run({ ...noisy, candidate: [91] }).verdict).toBe('pending_confirm');
    expect(run({ ...noisy }).requiredGain).toBe(8);
  });

  it('min_gain dominates when noise is small', () => {
    expect(run({ noiseFloor: 0.1 }).requiredGain).toBe(3);
  });

  it('zero noise and zero min_gain: any strict improvement counts', () => {
    expect(run({ noiseFloor: 0, minGain: 0, candidate: [99.9] }).verdict).toBe('pending_confirm');
    expect(run({ noiseFloor: 0, minGain: 0, candidate: [100] }).verdict).toBe('rejected');
  });

  it('uses |best| for the relative gain when best is negative', () => {
    const r = run({ currentBest: -100, candidate: [-110], noiseFloor: 0 });
    expect(r.requiredGain).toBe(3);
    expect(r.verdict).toBe('pending_confirm');
  });
});

describe('evaluateRatchet: blocked takes precedence', () => {
  it('a failed guardrail blocks even a great, confirmed result', () => {
    const r = run({ candidate: [50], confirm: [50], guardrails: [ok, bad] });
    expect(r.verdict).toBe('blocked');
    expect(r.reasons).toEqual(['guardrail failed: pytest -q (exited with status 1)']);
  });

  it('a protected path violation blocks', () => {
    const r = run({ protectedViolations: [{ path: 'bench/x.js', pattern: 'bench/**' }] });
    expect(r.verdict).toBe('blocked');
    expect(r.reasons[0]).toMatch(/protected path changed: bench\/x\.js \(matches bench\/\*\*\)/);
  });

  it('blocked even when there is no gain (not rejected)', () => {
    expect(run({ candidate: [120], guardrails: [bad] }).verdict).toBe('blocked');
  });

  it('lists all blocking reasons', () => {
    const r = run({
      guardrails: [bad, { name: 'error_rate < 0.001', passed: false }],
      protectedViolations: [{ path: 'ratchet.yaml', pattern: 'ratchet.yaml' }],
    });
    expect(r.reasons).toHaveLength(3);
  });

  it('passing guardrails and no violations do not block', () => {
    expect(run({ guardrails: [] }).verdict).toBe('pending_confirm');
  });
});

describe('evaluateRatchet: invalid input', () => {
  it('throws on empty candidate samples', () => {
    expect(() => run({ candidate: [] })).toThrow(RangeError);
  });
  it('throws on non-finite or negative parameters', () => {
    expect(() => run({ currentBest: NaN })).toThrow(RangeError);
    expect(() => run({ noiseFloor: -1 })).toThrow(RangeError);
    expect(() => run({ minGain: -0.1 })).toThrow(RangeError);
    expect(() => run({ candidate: [NaN] })).toThrow(RangeError);
  });
});
