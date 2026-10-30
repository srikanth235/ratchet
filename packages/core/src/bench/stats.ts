/** Summary statistics for repeated benchmark runs. All functions are pure. */

export interface Summary {
  n: number;
  mean: number;
  median: number;
  /** Sample standard deviation (n - 1 denominator); 0 when n === 1. */
  stddev: number;
  /** Coefficient of variation: stddev / |mean|. 0 when stddev is 0; Infinity when mean is 0 but stddev is not. */
  cv: number;
  min: number;
  max: number;
}

function assertSamples(samples: readonly number[]): void {
  if (samples.length === 0) throw new RangeError('at least one sample is required');
  for (const s of samples) {
    if (!Number.isFinite(s)) throw new RangeError(`samples must be finite numbers, got ${s}`);
  }
}

export function mean(samples: readonly number[]): number {
  assertSamples(samples);
  let sum = 0;
  for (const s of samples) sum += s;
  return sum / samples.length;
}

export function median(samples: readonly number[]): number {
  assertSamples(samples);
  const sorted = [...samples].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 === 1 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
}

/** Sample standard deviation (Bessel-corrected). Returns 0 for a single sample. */
export function stddev(samples: readonly number[]): number {
  assertSamples(samples);
  if (samples.length === 1) return 0;
  const m = mean(samples);
  let ss = 0;
  for (const s of samples) ss += (s - m) ** 2;
  return Math.sqrt(ss / (samples.length - 1));
}

export function coefficientOfVariation(samples: readonly number[]): number {
  const sd = stddev(samples);
  if (sd === 0) return 0;
  const m = Math.abs(mean(samples));
  return m === 0 ? Infinity : sd / m;
}

export function summarize(samples: readonly number[]): Summary {
  assertSamples(samples);
  return {
    n: samples.length,
    mean: mean(samples),
    median: median(samples),
    stddev: stddev(samples),
    cv: coefficientOfVariation(samples),
    min: Math.min(...samples),
    max: Math.max(...samples),
  };
}

export interface NoiseFloorOptions {
  /** Multiplier on the baseline standard deviation. Default 2. */
  k?: number;
  /** Absolute lower bound for the floor, in metric units. Default 0. */
  minAbsolute?: number;
}

/**
 * Noise floor derived from baseline runs, in the metric's own units:
 *
 *     noise_floor = max(k * stddev(baseline), minAbsolute)      (k defaults to 2)
 *
 * A candidate's improvement (|best - candidate|) must exceed this to be
 * distinguishable from run-to-run variance. With one baseline run the stddev
 * is 0, so the floor degenerates to `minAbsolute`; use repeats >= 3.
 */
export function noiseFloor(baselineSamples: readonly number[], opts: NoiseFloorOptions = {}): number {
  const k = opts.k ?? 2;
  const minAbsolute = opts.minAbsolute ?? 0;
  if (!(k >= 0) || !Number.isFinite(k)) throw new RangeError('k must be a non-negative finite number');
  if (!(minAbsolute >= 0) || !Number.isFinite(minAbsolute)) {
    throw new RangeError('minAbsolute must be a non-negative finite number');
  }
  return Math.max(k * stddev(baselineSamples), minAbsolute);
}
