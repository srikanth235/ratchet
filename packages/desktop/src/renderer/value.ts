import { summarizeYaml, type YamlSummary } from './config-summary';

/**
 * Dollar value of an optimization's improvement. Mirrors `estimateValue` / `returnOnSpend` in @ratchet/core
 * (core/src/value.ts): the renderer cannot import core (it pulls in the SQLite ledger), so the formula lives here
 * too and reads `objective.value` from the optimization's ratchet.yaml.
 */
export interface ValueView {
  usdPerPeriod: number;
  usdPerYear: number;
  period: 'month' | 'year';
  basis: 'estimated' | 'measured';
  unit: string | null;
  /** Yearly value per dollar spent; null when nothing was spent. */
  returnOnSpend: number | null;
}

export function valueOf(
  configYaml: string,
  direction: 'lower' | 'higher',
  baseline: number | null,
  best: number | null,
  spendUsd: number,
  summary: YamlSummary | null = summarizeYaml(configYaml),
): ValueView | null {
  const v = summary?.value;
  if (!v || baseline === null || best === null) return null;
  const improvement = direction === 'lower' ? baseline - best : best - baseline;
  if (!(improvement > 0)) return null; // nothing gained, nothing to value
  const usdPerPeriod = improvement * v.usdPerUnit;
  const usdPerYear = v.period === 'month' ? usdPerPeriod * 12 : usdPerPeriod;
  return { usdPerPeriod, usdPerYear, period: v.period, basis: v.basis, unit: v.unit, returnOnSpend: spendUsd > 0 ? usdPerYear / spendUsd : null };
}

/** `$1,284` / `$18.2k` / `$1.2M`. */
export function fmtUsdCompact(v: number): string {
  const a = Math.abs(v);
  if (a >= 1_000_000) return `$${(v / 1_000_000).toFixed(1)}M`;
  if (a >= 10_000) return `$${(v / 1000).toFixed(1)}k`;
  return `$${Math.round(v).toLocaleString('en-US')}`;
}

export const fmtRoi = (x: number): string =>
  x >= 1_000_000 ? `${(x / 1_000_000).toFixed(1)}M×` : x >= 10_000 ? `${(x / 1000).toFixed(1)}k×` : `${x >= 100 ? x.toFixed(0) : x.toFixed(1)}×`;
