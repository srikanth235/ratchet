import { describe, expect, it } from 'vitest';
import { estimateValue, returnOnSpend } from './value.js';

const cfg = (direction: 'minimize' | 'maximize', value?: object) =>
  ({ objective: { metric: 'm', direction, min_gain: 0.03, ...(value ? { value } : {}) } }) as never;

describe('estimateValue', () => {
  it('returns null without objective.value', () => {
    expect(estimateValue(cfg('minimize'), 182, 124)).toBeNull();
  });
  it('values a minimize improvement per year', () => {
    const v = estimateValue(cfg('minimize', { usd_per_unit: 1000, period: 'year', basis: 'estimated', unit: 'ms' }), 182, 124)!;
    expect(v.improvement).toBe(58);
    expect(v.usdPerYear).toBe(58_000);
    expect(v.basis).toBe('estimated');
  });
  it('annualizes monthly values and handles maximize', () => {
    const v = estimateValue(cfg('maximize', { usd_per_unit: 10, period: 'month', basis: 'measured' }), 0.91, 0.93)!;
    expect(v.usdPerPeriod).toBeCloseTo(0.2);
    expect(v.usdPerYear).toBeCloseTo(2.4);
  });
  it('never reports negative value for a regression', () => {
    expect(estimateValue(cfg('minimize', { usd_per_unit: 5, period: 'year', basis: 'estimated' }), 100, 120)!.usdPerYear).toBe(0);
  });
});

describe('returnOnSpend', () => {
  it('divides yearly value by spend', () => {
    expect(returnOnSpend(58_000, 268)).toBeCloseTo(216.4, 1);
    expect(returnOnSpend(1, 0)).toBeNull();
  });
});
