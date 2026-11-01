import { describe, expect, it } from 'vitest';
import { CATEGORIES, HOLDOUT_WEEKS, SKU_COUNT, TRAIN_WEEKS, WEEKS_PER_YEAR, generateSales, holidayIntensity, trainSales } from '../../data/generate.js';

describe('generateSales', () => {
  const t = trainSales();

  it('has the documented shape', () => {
    expect(t.skus).toHaveLength(SKU_COUNT);
    expect(t.units).toHaveLength(SKU_COUNT);
    expect(t.promo).toHaveLength(SKU_COUNT);
    expect(t.holiday).toHaveLength(TRAIN_WEEKS);
    for (const row of [...t.units, ...t.promo]) expect(row).toHaveLength(TRAIN_WEEKS);
    expect(new Set(t.skus.map((s) => s.category)).size).toBe(CATEGORIES);
  });

  it('is deterministic', () => {
    expect(generateSales({ weeks: 60 })).toEqual(generateSales({ weeks: 60 }));
  });

  it('does not depend on how many weeks are generated (training weeks are a stable prefix)', () => {
    const short = generateSales({ weeks: 80 });
    const full = generateSales({ weeks: TRAIN_WEEKS + HOLDOUT_WEEKS, futureSeed: 5 });
    expect(full.units.map((r) => r.slice(0, 80))).toEqual(short.units);
    expect(full.units.map((r) => r.slice(0, TRAIN_WEEKS))).toEqual(t.units);
  });

  it('draws the sealed weeks from the future seed only', () => {
    const a = generateSales({ weeks: TRAIN_WEEKS + HOLDOUT_WEEKS, futureSeed: 1 });
    const b = generateSales({ weeks: TRAIN_WEEKS + HOLDOUT_WEEKS, futureSeed: 2 });
    expect(a.units.map((r) => r.slice(0, TRAIN_WEEKS))).toEqual(b.units.map((r) => r.slice(0, TRAIN_WEEKS)));
    expect(a.units.map((r) => r.slice(TRAIN_WEEKS))).not.toEqual(b.units.map((r) => r.slice(TRAIN_WEEKS)));
    expect(() => generateSales({ weeks: TRAIN_WEEKS + 1 })).toThrow(/futureSeed/);
  });

  it('sells non-negative whole units and plans promotions in about a tenth of the weeks', () => {
    for (const row of t.units) for (const v of row) expect(Number.isInteger(v) && v >= 0).toBe(true);
    const promoShare = t.promo.flat().reduce((a, b) => a + b, 0) / (SKU_COUNT * TRAIN_WEEKS);
    expect(promoShare).toBeGreaterThan(0.05);
    expect(promoShare).toBeLessThan(0.2);
  });

  it('has yearly seasonality, stockouts and promotion lift', () => {
    const total = (w: number) => t.units.reduce((a, r) => a + r[w]!, 0);
    const byWeekOfYear = Array.from({ length: WEEKS_PER_YEAR }, (_, j) => total(j) + total(j + WEEKS_PER_YEAR));
    expect(Math.max(...byWeekOfYear) / Math.min(...byWeekOfYear)).toBeGreaterThan(1.3);
    const ordinary: number[] = [];
    const promoted: number[] = [];
    t.units.forEach((row, s) => row.forEach((v, w) => (t.holiday[w] === 0 ? (t.promo[s]![w] ? promoted : ordinary).push(v) : undefined)));
    const avg = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;
    expect(avg(promoted)).toBeGreaterThan(1.3 * avg(ordinary));
  });

  it('moves Easter between years but not Christmas', () => {
    const easter = [0, 1, 2].map((y) => Array.from({ length: WEEKS_PER_YEAR }, (_, j) => holidayIntensity(y * WEEKS_PER_YEAR + j)).indexOf(0.8));
    expect(new Set(easter).size).toBe(3);
    expect([0, 1, 2].map((y) => holidayIntensity(y * WEEKS_PER_YEAR + 51))).toEqual([1, 1, 1]);
  });
});
