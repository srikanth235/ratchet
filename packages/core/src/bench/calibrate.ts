import { summarize } from './stats.js';

/**
 * Auto-calibrated statistics (docs/SIMPLIFIED_MODEL.md: "no knobs the system can derive").
 *
 * From the baseline's coefficient of variation (CV = stddev / mean of the baseline runs) Ratchet derives what the
 * user used to type:
 *
 *   resolution to detect   g      = max(2 x CV, 1%)           the smallest gain worth resolving
 *   repeats per candidate  n      = ceil(2 z^2 CV^2 / g^2)    z = 3, clamped to [3, 15]
 *   win threshold          minGain = ceil_to_step(max(g, z sqrt(2) CV / sqrt(n)))
 *
 * `z sqrt(2) CV / sqrt(n)` is the gain at which the difference of two n-run medians stands z standard errors above
 * zero, so `n` is "enough repeats that noise cannot fake a win of g". When the cap is hit, `minGain` rises to what
 * the capped repeats can actually resolve instead of pretending. Any of `repeats` / `minGain` / `noiseFactor` set
 * in ratchet.yaml is respected as-is and the other is derived around it.
 */

/** Tunables of the derivation. Exported so tests and docs have one place to look. */
export const CALIBRATION = {
  /** Repeats used for the baseline itself when ratchet.yaml gives none; also the sample the CV is measured on. */
  baselineRepeats: 5,
  minRepeats: 3,
  maxRepeats: 15,
  /** Smallest gain Ratchet ever tries to resolve (a fraction). */
  floorGain: 0.01,
  /** Standard errors a win must stand above zero. */
  z: 3,
  /** `2 x CV` is the "noise" shown to the user: the band that holds ~95% of single runs. */
  noiseSigmas: 2,
  /** Used when the baseline is too small or degenerate to estimate noise from. */
  fallbackGain: 0.03,
  /** The default k of the noise floor `k x stddev`. */
  noiseFactor: 2,
} as const;

export interface CalibrationInput {
  /** The baseline's bench samples (the objective metric of each run). */
  samples: readonly number[];
  /** `evaluator.repeats` from ratchet.yaml, when set. */
  repeats?: number | undefined;
  /** `objective.min_gain` from ratchet.yaml (a fraction), when set. */
  minGain?: number | undefined;
  /** `evaluator.noise_factor` from ratchet.yaml, when set. */
  noiseFactor?: number | undefined;
}

export interface Calibration {
  /** Schema version of this record (it is stored as JSON). */
  version: 1;
  /** Number of baseline runs the noise was measured on. */
  baselineRuns: number;
  /** Coefficient of variation of the baseline (fraction); null when it cannot be estimated (fewer than 3 runs, or a zero mean). */
  cv: number | null;
  /** `2 x CV` as a fraction: "this metric moves by about +/- noise from run to run". Null with `cv`. */
  noise: number | null;
  /** Runs per candidate / confirm run. */
  repeats: number;
  repeatsSource: 'config' | 'auto';
  /** True when the derivation wanted more than the cap allows. */
  repeatsCapped: boolean;
  /** The win threshold as a fraction of the current best. */
  minGain: number;
  minGainSource: 'config' | 'auto';
  /** The gain the chosen repeats can resolve with confidence (a fraction); `minGain` is never below it when auto. */
  resolvable: number | null;
  /** The k of the noise floor `k x stddev(baseline)`. */
  noiseFactor: number;
  noiseFactorSource: 'config' | 'default';
  /** Plain words for the UI, e.g. "this metric is noisy (±9%); wins must beat 9%; each candidate is measured 7 times". */
  explanation: string;
}

/** Round a fraction up to a readable step: 0.5% below 2%, 1% below 20%, 5% above (never above 100%). */
export function niceGain(x: number): number {
  if (!Number.isFinite(x) || x <= 0) return CALIBRATION.floorGain;
  const step = x <= 0.02 ? 0.005 : x <= 0.2 ? 0.01 : 0.05;
  const v = Math.ceil(x / step - 1e-9) * step;
  return Math.min(1, Number(v.toFixed(6)));
}

/** "9%", "0.9%", "1.5%": whole percent from 5% up, one decimal below, trailing ".0" dropped. */
export function formatPercent(fraction: number): string {
  const p = fraction * 100;
  if (p >= 5) return `${Math.round(p)}%`;
  return `${Number(p.toFixed(1))}%`;
}

/** How many runs of n it takes for the median-vs-median difference to stand z standard errors above `gain`. */
function repeatsFor(cv: number, gain: number): number {
  const { z, minRepeats } = CALIBRATION;
  const n = Math.ceil((2 * z * z * cv * cv) / (gain * gain) - 1e-9);
  return Math.max(minRepeats, n);
}

/** The gain n runs per side resolve at z standard errors. */
function resolvableGain(cv: number, n: number): number {
  return (CALIBRATION.z * Math.SQRT2 * cv) / Math.sqrt(n);
}

function noiseWord(noise: number): string {
  if (noise < 0.01) return 'stable';
  if (noise < 0.04) return 'slightly noisy';
  if (noise < 0.1) return 'noisy';
  return 'very noisy';
}

/**
 * Derive repeats, win threshold and noise factor from the baseline runs. Pure. Explicit settings win; the rest is
 * derived. With fewer than 3 baseline runs (or a zero mean) there is nothing to measure noise on: the old default of
 * 3% is used for the threshold and the explanation says so.
 */
export function calibrate(input: CalibrationInput): Calibration {
  const { samples } = input;
  const baselineRuns = samples.length;
  const noiseFactor = input.noiseFactor ?? CALIBRATION.noiseFactor;
  const noiseFactorSource = input.noiseFactor === undefined ? 'default' : 'config';
  const repeatsSource = input.repeats === undefined ? 'auto' : 'config';
  const minGainSource = input.minGain === undefined ? 'auto' : 'config';

  const sum = baselineRuns > 0 ? summarize(samples) : undefined;
  const measurable = sum !== undefined && baselineRuns >= CALIBRATION.minRepeats && sum.mean !== 0 && Number.isFinite(sum.cv);
  const cv = measurable ? sum.cv : null;

  let repeats: number;
  let capped = false;
  let minGain: number;
  let resolvable: number | null = null;

  if (cv === null) {
    repeats = input.repeats ?? CALIBRATION.minRepeats + 2; // 5: the old default
    minGain = input.minGain ?? CALIBRATION.fallbackGain;
  } else {
    const target = input.minGain ?? Math.max(CALIBRATION.noiseSigmas * cv, CALIBRATION.floorGain);
    if (input.repeats === undefined) {
      const wanted = repeatsFor(cv, Math.max(target, 1e-9));
      capped = wanted > CALIBRATION.maxRepeats;
      repeats = Math.min(wanted, CALIBRATION.maxRepeats);
    } else {
      repeats = input.repeats;
    }
    resolvable = resolvableGain(cv, repeats);
    minGain = input.minGain ?? niceGain(Math.max(target, resolvable));
  }

  const noise = cv === null ? null : CALIBRATION.noiseSigmas * cv;
  const result: Calibration = {
    version: 1,
    baselineRuns,
    cv,
    noise,
    repeats,
    repeatsSource,
    repeatsCapped: capped,
    minGain,
    minGainSource,
    resolvable,
    noiseFactor,
    noiseFactorSource,
    explanation: '',
  };
  result.explanation = explainCalibration(result);
  return result;
}

/** The one-line, plain-words summary the UI shows next to the metric. */
export function explainCalibration(c: Calibration): string {
  const parts: string[] = [];
  if (c.noise === null) {
    parts.push(
      `the noise of this metric could not be measured (${c.baselineRuns} baseline run${c.baselineRuns === 1 ? '' : 's'}, at least ${CALIBRATION.minRepeats} needed)`,
      `wins must beat ${formatPercent(c.minGain)}${c.minGainSource === 'config' ? ' (set in ratchet.yaml)' : ''}`,
    );
  } else {
    parts.push(
      `this metric is ${noiseWord(c.noise)} (±${formatPercent(c.noise)})`,
      `wins must beat ${formatPercent(c.minGain)}${c.minGainSource === 'config' ? ' (set in ratchet.yaml)' : ''}`,
    );
  }
  parts.push(`each candidate is measured ${c.repeats} times${c.repeatsSource === 'config' ? ' (set in ratchet.yaml)' : ''}`);
  if (c.repeatsCapped && c.resolvable !== null) {
    parts.push(`${CALIBRATION.maxRepeats} runs is the cap, so gains under ${formatPercent(c.resolvable)} cannot be told from noise`);
  } else if (c.minGainSource === 'config' && c.resolvable !== null && c.resolvable > c.minGain + 1e-9) {
    parts.push(`with ${c.repeats} runs gains under ${formatPercent(c.resolvable)} may still be noise`);
  }
  return parts.join('; ');
}
