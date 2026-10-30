import type { RatchetConfig } from './config/schema.js';

export interface ValueEstimate {
  /** Metric improvement in the objective's direction (positive = better), in metric units. */
  improvement: number;
  /** USD per `period` implied by the improvement. */
  usdPerPeriod: number;
  period: 'month' | 'year';
  /** Normalized to a year, for ROI. */
  usdPerYear: number;
  basis: 'estimated' | 'measured';
  unit?: string;
}

/** Dollar value of moving the objective from `baseline` to `best`, or null when the config has no `objective.value`. */
export function estimateValue(config: Pick<RatchetConfig, 'objective'>, baseline: number, best: number): ValueEstimate | null {
  const v = config.objective.value;
  if (!v || !Number.isFinite(baseline) || !Number.isFinite(best)) return null;
  const improvement = config.objective.direction === 'minimize' ? baseline - best : best - baseline;
  const usdPerPeriod = Math.max(0, improvement) * v.usd_per_unit;
  return {
    improvement,
    usdPerPeriod,
    period: v.period,
    usdPerYear: v.period === 'month' ? usdPerPeriod * 12 : usdPerPeriod,
    basis: v.basis,
    ...(v.unit ? { unit: v.unit } : {}),
  };
}

/** Return on spend: yearly value created per dollar spent (null when nothing was spent). */
export function returnOnSpend(usdPerYear: number, spendUsd: number): number | null {
  return spendUsd > 0 ? usdPerYear / spendUsd : null;
}
