import { describe, expect, it } from 'vitest';
import { predict } from '../../src/predict.js';
import type { SalesTable } from '../../src/types.js';
import { generateSales } from '../../data/generate.js';

/** A table whose SKUs all sell exactly `level` every week, with no promotions or holidays. */
function flatTable(level: number, weeks = 120, skus = 4): SalesTable {
  return {
    skus: Array.from({ length: skus }, (_, id) => ({ id, category: id % 2 })),
    units: Array.from({ length: skus }, () => new Array<number>(weeks).fill(level)),
    promo: Array.from({ length: skus }, () => new Array<number>(weeks).fill(0)),
    holiday: new Array<number>(weeks).fill(0),
  };
}

describe('model behaviour any improvement must keep', () => {
  it('forecasts a constant series as that constant', () => {
    const fc = predict(flatTable(40), 110, 8);
    for (const row of fc) for (const v of row) expect(v).toBeCloseTo(40, 0);
  });

  it('is scale-equivariant: tripling every sale triples the forecast', () => {
    const t = generateSales({ weeks: 110 });
    const tripled: SalesTable = { ...t, units: t.units.map((r) => r.map((v) => v * 3)) };
    const a = predict(t, 104, 6);
    const b = predict(tripled, 104, 6);
    a.forEach((row, s) => row.forEach((v, h) => expect(b[s]![h]!).toBeCloseTo(3 * v, 6)));
  });

  it('never forecasts negative or non-finite sales, also for sparse and short histories', () => {
    const sparse = flatTable(0, 80);
    sparse.units[1]![69] = 1;
    for (const [table, origin] of [[sparse, 65], [generateSales({ weeks: 30 }), 20], [flatTable(5, 100), 8]] as const) {
      for (const row of predict(table, origin, 6)) for (const v of row) expect(Number.isFinite(v) && v >= 0).toBe(true);
    }
  });

  it('beats the naive last-4-weeks average on a strongly seasonal series', () => {
    const weeks = 130;
    const season = (w: number) => 100 * (1 + 0.5 * Math.sin((2 * Math.PI * w) / 52));
    const t = flatTable(0, weeks, 2);
    t.units = t.units.map(() => Array.from({ length: weeks }, (_, w) => season(w)));
    const origin = 104;
    const err = (fc: number[]) => fc.reduce((a, v, h) => a + Math.abs(v - season(origin + h)), 0);
    const naive = (row: number[]) => row.slice(origin - 4, origin).reduce((a, b) => a + b, 0) / 4;
    const fc = predict(t, origin, 12)[0]!;
    expect(err(fc)).toBeLessThan(err(new Array<number>(12).fill(naive(t.units[0]!))) + 1e-9);
  });
});
