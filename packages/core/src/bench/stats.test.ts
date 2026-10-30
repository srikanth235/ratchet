import { describe, expect, it } from 'vitest';
import {
  coefficientOfVariation,
  mean,
  median,
  noiseFloor,
  stddev,
  summarize,
} from './stats.js';

describe('stats', () => {
  it('mean', () => {
    expect(mean([1, 2, 3, 4])).toBe(2.5);
    expect(mean([7])).toBe(7);
  });

  it('median, odd/even/unsorted, without mutating input', () => {
    const xs = [5, 1, 3];
    expect(median(xs)).toBe(3);
    expect(xs).toEqual([5, 1, 3]);
    expect(median([4, 1, 3, 2])).toBe(2.5);
    expect(median([9])).toBe(9);
  });

  it('stddev is the sample (n-1) deviation', () => {
    // classic example: mean 5, sum of squares 32 => sqrt(32/7)
    expect(stddev([2, 4, 4, 4, 5, 5, 7, 9])).toBeCloseTo(Math.sqrt(32 / 7), 12);
    expect(stddev([3, 3, 3])).toBe(0);
    expect(stddev([42])).toBe(0);
  });

  it('coefficient of variation', () => {
    expect(coefficientOfVariation([10, 10, 10])).toBe(0);
    expect(coefficientOfVariation([9, 11])).toBeCloseTo(Math.SQRT2 / 10, 12);
    expect(coefficientOfVariation([-1, 1])).toBe(Infinity); // mean 0, stddev > 0
    expect(coefficientOfVariation([0, 0])).toBe(0);
    expect(coefficientOfVariation([-9, -11])).toBeGreaterThan(0); // uses |mean|
  });

  it('summarize', () => {
    const s = summarize([10, 12, 11, 13, 9]);
    expect(s).toMatchObject({ n: 5, mean: 11, median: 11, min: 9, max: 13 });
    expect(s.stddev).toBeCloseTo(Math.sqrt(2.5), 12);
    expect(s.cv).toBeCloseTo(Math.sqrt(2.5) / 11, 12);
  });

  it('rejects empty and non-finite samples', () => {
    expect(() => mean([])).toThrow(RangeError);
    expect(() => median([])).toThrow(RangeError);
    expect(() => summarize([])).toThrow(RangeError);
    expect(() => stddev([1, NaN])).toThrow(RangeError);
    expect(() => mean([1, Infinity])).toThrow(RangeError);
  });
});

describe('noiseFloor', () => {
  it('is 2 x stddev by default', () => {
    const xs = [100, 102, 98, 101, 99];
    expect(noiseFloor(xs)).toBeCloseTo(2 * stddev(xs), 12);
  });

  it('honours k and the absolute minimum', () => {
    const xs = [100, 102, 98, 101, 99];
    expect(noiseFloor(xs, { k: 3 })).toBeCloseTo(3 * stddev(xs), 12);
    expect(noiseFloor(xs, { minAbsolute: 50 })).toBe(50);
    expect(noiseFloor([5, 5, 5], { minAbsolute: 0.25 })).toBe(0.25);
  });

  it('is 0 for perfectly stable runs and a single run', () => {
    expect(noiseFloor([5, 5, 5])).toBe(0);
    expect(noiseFloor([5])).toBe(0);
  });

  it('validates options', () => {
    expect(() => noiseFloor([1, 2], { k: -1 })).toThrow(RangeError);
    expect(() => noiseFloor([1, 2], { minAbsolute: -1 })).toThrow(RangeError);
    expect(() => noiseFloor([])).toThrow(RangeError);
  });
});
