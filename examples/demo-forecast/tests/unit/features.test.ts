import { describe, expect, it } from 'vitest';
import { buildSeries, mean, tail } from '../../src/features.js';
import { generateSales } from '../../data/generate.js';

describe('buildSeries', () => {
  const table = generateSales({ weeks: 60 });

  it('hands the model sales strictly before the origin and the calendar through the horizon', () => {
    const series = buildSeries(table, 40, 12);
    expect(series).toHaveLength(table.skus.length);
    for (const s of series) {
      expect(s.y).toHaveLength(40);
      expect(s.promo).toHaveLength(52);
      expect(s.holiday).toHaveLength(52);
    }
    expect(series[3]!.y).toEqual(table.units[3]!.slice(0, 40));
    expect(series[3]!.category).toBe(table.skus[3]!.category);
  });
});

describe('helpers', () => {
  it('mean and tail', () => {
    expect(mean([])).toBe(0);
    expect(mean([1, 2, 6])).toBe(3);
    expect(tail([1, 2, 3, 4, 5], 2)).toEqual([4, 5]);
    expect(tail([1], 5)).toEqual([1]);
  });
});
