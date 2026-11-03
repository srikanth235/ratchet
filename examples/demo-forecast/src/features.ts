/** Feature extraction: what the model gets to see for each SKU at one forecast origin. */
import type { SalesTable } from './types.js';

/** One SKU as the model sees it at the forecast origin. */
export interface Series {
  sku: number;
  category: number;
  /** Sales for weeks 0 .. origin-1 (never anything later). */
  y: number[];
  /** Planned promotions for weeks 0 .. origin+horizon-1 (a plan, known in advance). */
  promo: number[];
  /** Holiday intensity for weeks 0 .. origin+horizon-1 (known in advance). */
  holiday: number[];
}

/** The only place the pipeline reads `table.units`, and only before `origin`. */
export function buildSeries(table: SalesTable, origin: number, horizon: number): Series[] {
  return table.skus.map((s) => ({
    sku: s.id,
    category: s.category,
    y: table.units[s.id]!.slice(0, origin),
    promo: table.promo[s.id]!.slice(0, origin + horizon),
    holiday: table.holiday.slice(0, origin + horizon),
  }));
}

export function mean(xs: readonly number[]): number {
  return xs.length === 0 ? 0 : xs.reduce((a, b) => a + b, 0) / xs.length;
}

/** The last `n` observations. */
export function tail(xs: readonly number[], n: number): number[] {
  return xs.slice(Math.max(0, xs.length - n));
}
