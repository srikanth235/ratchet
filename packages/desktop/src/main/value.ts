import { estimateValue, returnOnSpend, type OptimizationRow } from '@ratchet/core';
import type { ValueVM } from '../shared/types.js';

/** Improvement of `value` over `baseline` in percent, positive = better in the objective's direction. */
export function gainVsBaseline(direction: 'minimize' | 'maximize', baseline: number, value: number): number {
  if (baseline === 0) return 0;
  const delta = direction === 'minimize' ? baseline - value : value - baseline;
  return (delta / Math.abs(baseline)) * 100;
}

/** The optimization's `objective.value` (ledger columns) as the config shape core's `estimateValue` reads. */
function valueConfig(row: OptimizationRow) {
  if (row.valueUsdPerUnit === null) return null;
  return {
    objective: {
      metric: row.metric,
      direction: row.direction,
      min_gain: row.minGain,
      value: {
        usd_per_unit: row.valueUsdPerUnit,
        period: row.valuePeriod ?? 'year',
        basis: row.valueBasis ?? 'estimated',
        ...(row.valueUnit ? { unit: row.valueUnit } : {}),
      },
    },
  };
}

/** Yearly dollar value of moving the objective from `from` to `to`; null without a value model. */
export function valueOfMove(row: OptimizationRow, from: number, to: number): number | null {
  const cfg = valueConfig(row);
  const est = cfg ? estimateValue(cfg, from, to) : null;
  return est ? est.usdPerYear : null;
}

/** Value of the improvement baseline → best and its return on `spendUsd`; null without a model or an improvement. */
export function valueOf(row: OptimizationRow, baseline: number | null, best: number | null, spendUsd: number): ValueVM | null {
  const cfg = valueConfig(row);
  if (!cfg || baseline === null || best === null) return null;
  const est = estimateValue(cfg, baseline, best);
  if (!est || !(est.improvement > 0)) return null; // nothing gained, nothing to value
  return {
    usdPerUnit: cfg.objective.value.usd_per_unit,
    unit: row.valueUnit,
    period: est.period,
    basis: est.basis,
    usdPerYear: est.usdPerYear,
    returnOnSpend: returnOnSpend(est.usdPerYear, spendUsd),
  };
}
