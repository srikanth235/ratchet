/**
 * The pipeline's data contract (protected: tests/contract pins it).
 *
 * `units[s][w]` is what SKU `s` sold in week `w` (weeks are absolute indexes; 52 per year). `promo` and `holiday` are
 * planned calendars, known ahead of time, so they may extend past `units`. A forecast made at `origin` may use `units`
 * only for weeks strictly before `origin`; everything else in the table is off limits (the leakage guardrail checks it).
 */
export interface SkuMeta {
  id: number;
  category: number;
}

export interface SalesTable {
  skus: SkuMeta[];
  /** [sku][week]; may hold more weeks than `origin` when evaluating a backtest. */
  units: number[][];
  /** [sku][week]: 1 when a promotion is planned. Known in advance. */
  promo: number[][];
  /** [week]: holiday intensity, 0 for none. Known in advance. */
  holiday: number[];
}

/** Forecast for weeks `origin .. origin+horizon-1`, indexed [sku][h]. Finite and >= 0. */
export type Forecast = number[][];

export type PredictFn = (table: SalesTable, origin: number, horizon: number) => Forecast;
