/**
 * Leakage guardrail, development-time view. The same checks run inside the trusted evaluator (eval/backtest.ts reports
 * `leakage_violations`, which ratchet.yaml requires to be 0); here they also fail `pnpm test` with the reasons.
 */
import { describe, expect, it } from 'vitest';
import { findLeaks, scanSources } from '../../eval/leakage.js';
import { predict } from '../../src/predict.js';
import type { PredictFn } from '../../src/types.js';
import { trainSales } from '../../data/generate.js';

const HORIZON = 12;
const ORIGINS = [96, 110, 132];

describe('leakage guardrail', () => {
  const table = trainSales();

  it('the pipeline ignores sales on and after the forecast origin', () => {
    expect(findLeaks(predict, table, ORIGINS, HORIZON)).toEqual([]);
  });

  it('would catch a pipeline that peeks at the future (the detector is not vacuous)', () => {
    const peeker: PredictFn = (t, origin, horizon) => t.skus.map((s) => Array.from({ length: horizon }, (_, h) => t.units[s.id]![origin + h] ?? 0));
    const oneWeekAhead: PredictFn = (t, origin, horizon) => t.skus.map((s) => Array.from({ length: horizon }, () => t.units[s.id]![origin] ?? 0));
    const blend: PredictFn = (t, origin, horizon) => predict(t, origin, horizon).map((row, s) => row.map((v, h) => 0.7 * v + 0.3 * (t.units[s]![origin + h] ?? v)));
    for (const fn of [peeker, oneWeekAhead, blend]) expect(findLeaks(fn, table, ORIGINS, HORIZON).length).toBeGreaterThan(0);
  });

  it('the pipeline source has no side channel to the data', () => {
    expect(scanSources()).toEqual([]);
  });
});
