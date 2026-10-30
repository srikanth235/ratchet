import { describe, expect, it } from 'vitest';
import { CALIBRATION, calibrate, explainCalibration, formatPercent, niceGain } from './calibrate.js';

/** `n` samples around 100 with the given relative standard deviation (deterministic, exact sample stddev). */
function samplesWithCv(cv: number, n = 5): number[] {
  // alternate +-1 around the mean: sample stddev of +-d with n even is d*sqrt(n/(n-1)); scale so the CV is exact
  const raw = Array.from({ length: n }, (_, i) => (i % 2 === 0 ? 1 : -1));
  if (n % 2 === 1) raw[n - 1] = 0;
  const mean = raw.reduce((a, b) => a + b, 0) / n;
  const sd = Math.sqrt(raw.reduce((a, b) => a + (b - mean) ** 2, 0) / (n - 1));
  return raw.map((r) => 100 + (r / sd) * 100 * cv);
}

describe('niceGain / formatPercent', () => {
  it('rounds up to readable steps and never below the floor', () => {
    expect(niceGain(0.01)).toBe(0.01);
    expect(niceGain(0.0101)).toBe(0.015);
    expect(niceGain(0.0862)).toBe(0.09);
    expect(niceGain(0.2)).toBe(0.2);
    expect(niceGain(0.21)).toBe(0.25);
    expect(niceGain(3)).toBe(1);
    expect(niceGain(0)).toBe(CALIBRATION.floorGain);
    expect(niceGain(Number.NaN)).toBe(CALIBRATION.floorGain);
  });
  it('formats fractions as short percentages', () => {
    expect(formatPercent(0.09)).toBe('9%');
    expect(formatPercent(0.0086)).toBe('0.9%');
    expect(formatPercent(0.015)).toBe('1.5%');
    expect(formatPercent(0.123)).toBe('12%');
    expect(formatPercent(0.01)).toBe('1%');
  });
});

describe('calibrate: nothing set in ratchet.yaml', () => {
  it('a stable metric needs the minimum repeats and a 1% threshold', () => {
    const c = calibrate({ samples: samplesWithCv(0.001) });
    expect(c.cv).toBeCloseTo(0.001, 6);
    expect(c.repeats).toBe(CALIBRATION.minRepeats);
    expect(c.minGain).toBe(0.01);
    expect(c.repeatsSource).toBe('auto');
    expect(c.minGainSource).toBe('auto');
    expect(c.repeatsCapped).toBe(false);
    expect(c.explanation).toMatch(/^this metric is stable \(±0\.2%\); wins must beat 1%; each candidate is measured 3 times$/);
  });

  it('a noisy metric raises the threshold to max(2 x CV, 1%) and the repeats to resolve it', () => {
    const c = calibrate({ samples: samplesWithCv(0.043) });
    // 2 x CV = 8.6% -> 9%; n = ceil(18 cv^2 / g^2) with g = 8.6% -> 5 runs
    expect(c.noise).toBeCloseTo(0.086, 6);
    expect(c.minGain).toBe(0.09);
    expect(c.repeats).toBe(5);
    expect(c.explanation).toBe('this metric is noisy (±9%); wins must beat 9%; each candidate is measured 5 times');
  });

  it('threshold is never below what the chosen repeats can resolve', () => {
    for (const cv of [0.002, 0.006, 0.01, 0.03, 0.08, 0.2]) {
      const c = calibrate({ samples: samplesWithCv(cv) });
      expect(c.minGain).toBeGreaterThanOrEqual(Math.max(2 * cv, 0.01) - 1e-9);
      expect(c.minGain).toBeGreaterThanOrEqual(c.resolvable! - 1e-9);
      expect(c.repeats).toBeGreaterThanOrEqual(CALIBRATION.minRepeats);
      expect(c.repeats).toBeLessThanOrEqual(CALIBRATION.maxRepeats);
    }
  });

  it('is monotone: a noisier baseline never gets a lower threshold', () => {
    let prev = 0;
    for (const cv of [0.001, 0.004, 0.01, 0.02, 0.05, 0.1, 0.3]) {
      const g = calibrate({ samples: samplesWithCv(cv) }).minGain;
      expect(g).toBeGreaterThanOrEqual(prev);
      prev = g;
    }
  });

  it('a constant baseline (a deterministic metric) has CV 0', () => {
    const c = calibrate({ samples: [43.5, 43.5, 43.5] });
    expect(c.cv).toBe(0);
    expect(c.minGain).toBe(0.01);
    expect(c.repeats).toBe(3);
  });
});

describe('calibrate: explicit settings are respected', () => {
  it('repeats set: used as-is, threshold covers what they can resolve', () => {
    const c = calibrate({ samples: samplesWithCv(0.043), repeats: 3 });
    expect(c.repeats).toBe(3);
    expect(c.repeatsSource).toBe('config');
    // z sqrt(2) cv / sqrt(3) = 10.5% > 2 x CV
    expect(c.resolvable).toBeCloseTo((3 * Math.SQRT2 * 0.043) / Math.sqrt(3), 6);
    expect(c.minGain).toBe(0.11);
  });

  it('min_gain set: used as-is, repeats are derived to resolve it, capped', () => {
    const c = calibrate({ samples: samplesWithCv(0.05), minGain: 0.03 });
    expect(c.minGain).toBe(0.03);
    expect(c.minGainSource).toBe('config');
    expect(c.repeats).toBe(CALIBRATION.maxRepeats); // 18 x 0.0025 / 0.0009 = 50 -> cap
    expect(c.repeatsCapped).toBe(true);
    expect(c.explanation).toContain('(set in ratchet.yaml)');
    expect(c.explanation).toContain('15 runs is the cap, so gains under');
  });

  it('min_gain set and loose: fewer repeats suffice', () => {
    const c = calibrate({ samples: samplesWithCv(0.01), minGain: 0.1 });
    expect(c.repeats).toBe(3);
    expect(c.repeatsCapped).toBe(false);
  });

  it('both set: nothing is derived, but a too-optimistic pair is called out', () => {
    const c = calibrate({ samples: samplesWithCv(0.05), minGain: 0.03, repeats: 5 });
    expect(c.repeats).toBe(5);
    expect(c.minGain).toBe(0.03);
    expect(c.explanation).toMatch(/with 5 runs gains under \d+(\.\d)?% may still be noise/);
  });

  it('noise_factor is carried through', () => {
    expect(calibrate({ samples: [1, 1.1, 0.9], noiseFactor: 3 })).toMatchObject({ noiseFactor: 3, noiseFactorSource: 'config' });
    expect(calibrate({ samples: [1, 1.1, 0.9] })).toMatchObject({ noiseFactor: 2, noiseFactorSource: 'default' });
  });
});

describe('calibrate: degenerate baselines', () => {
  it('fewer than 3 runs cannot measure noise: falls back to 3% and says so', () => {
    const c = calibrate({ samples: [10, 11] });
    expect(c.cv).toBeNull();
    expect(c.noise).toBeNull();
    expect(c.minGain).toBe(CALIBRATION.fallbackGain);
    expect(c.explanation).toContain('could not be measured');
    expect(c.explanation).toContain('wins must beat 3%');
  });

  it('a zero mean (CV undefined) falls back the same way', () => {
    const c = calibrate({ samples: [-1, 0, 1] });
    expect(c.cv).toBeNull();
    expect(c.minGain).toBe(CALIBRATION.fallbackGain);
  });

  it('a single run with repeats set keeps them', () => {
    const c = calibrate({ samples: [5], repeats: 7, minGain: 0.02 });
    expect(c).toMatchObject({ repeats: 7, minGain: 0.02, cv: null });
  });

  it('a maximize-style metric near zero stays finite and capped', () => {
    const c = calibrate({ samples: [0.01, 0.03, 0.02, 0.05, 0.005] });
    expect(Number.isFinite(c.minGain)).toBe(true);
    expect(c.minGain).toBeLessThanOrEqual(1);
    expect(c.repeats).toBeLessThanOrEqual(CALIBRATION.maxRepeats);
  });
});

describe('explainCalibration', () => {
  it('names the noise level in words', () => {
    expect(calibrate({ samples: samplesWithCv(0.015) }).explanation).toMatch(/slightly noisy \(±3%\)/);
    expect(calibrate({ samples: samplesWithCv(0.2) }).explanation).toMatch(/very noisy \(±40%\)/);
  });
  it('is stable under a round trip through JSON (it is stored as JSON)', () => {
    const c = calibrate({ samples: samplesWithCv(0.043) });
    const back = JSON.parse(JSON.stringify(c));
    expect(explainCalibration(back)).toBe(c.explanation);
  });
});
