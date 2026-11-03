/** The interface the evaluator relies on: pinned, agents cannot change it (tests/contract/** is protected). */
import { describe, expect, it } from 'vitest';
import { predict } from '../../src/predict.js';
import { SKU_COUNT, trainSales } from '../../data/generate.js';

describe('predict() contract', () => {
  const table = trainSales();

  it('takes (table, origin, horizon)', () => {
    expect(typeof predict).toBe('function');
    expect(predict.length).toBe(3);
  });

  it.each([[96, 12], [120, 12], [132, 12], [132, 4], [100, 1]])('origin %i, horizon %i: one finite, non-negative row per SKU', (origin, horizon) => {
    const fc = predict(table, origin, horizon);
    expect(fc).toHaveLength(SKU_COUNT);
    for (const row of fc) {
      expect(row).toHaveLength(horizon);
      for (const v of row) expect(Number.isFinite(v) && v >= 0).toBe(true);
    }
  });

  it('is deterministic and does not modify the table', () => {
    const copy = structuredClone(table);
    const a = predict(table, 108, 12);
    expect(predict(table, 108, 12)).toEqual(a);
    expect(table).toEqual(copy);
  });
});
