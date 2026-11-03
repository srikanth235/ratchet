/**
 * Trusted evaluator (protected): rolling-origin backtest of `predict()` over the training period.
 *
 *   node dist/eval/backtest.js            # scores the backtest folds
 *   RATCHET_TRACE=<holdout.json> node dist/eval/backtest.js   # confirm phase: scores the sealed holdout instead
 *
 * Prints one JSON object: { wape, mape, bias, train_ms, leakage_violations } (percentages; train_ms = mean wall time
 * of one predict() call, i.e. one fit + forecast of all SKUs; leakage_violations = look-ahead and side-channel findings
 * of eval/leakage.ts, which ratchet.yaml requires to be 0).
 */
import { readFileSync } from 'node:fs';
import { predict } from '../src/predict.js';
import type { SalesTable } from '../src/types.js';
import { HOLDOUT_WEEKS, TRAIN_WEEKS, trainSales } from '../data/generate.js';
import { findLeaks, scanSources } from './leakage.js';
import { accuracy } from './metrics.js';

export const HORIZON = HOLDOUT_WEEKS;
/** Forecast origins: every 6 weeks over the second half of the training period (the last fold ends at the last training week). */
export const ORIGINS = [96, 102, 108, 114, 120, 126, 132];

export interface HoldoutFile {
  version: 1;
  firstWeek: number;
  /** [sku][h] actual sales of the sealed weeks. */
  units: number[][];
  /** [sku][h] planned promotions of the sealed weeks (known in advance). */
  promo: number[][];
  /** [h] */
  holiday: number[];
}

export interface Fold {
  table: SalesTable;
  origin: number;
  actual: number[][];
}

export function backtestFolds(train: SalesTable): Fold[] {
  return ORIGINS.map((origin) => ({
    table: train,
    origin,
    actual: train.units.map((row) => row.slice(origin, origin + HORIZON)),
  }));
}

/** Origins at which the evaluator probes for look-ahead (a subset of ORIGINS: each probe is several extra predict calls). */
export const LEAK_PROBE_ORIGINS = [96, 114, 132];

/** Sealed holdout: the table holds the training sales, extended with the planned calendar of the sealed weeks only. */
export function holdoutFolds(train: SalesTable, h: HoldoutFile): Fold[] {
  if (h.firstWeek !== TRAIN_WEEKS) throw new Error(`holdout starts at week ${h.firstWeek}, expected ${TRAIN_WEEKS}`);
  const table: SalesTable = {
    skus: train.skus,
    units: train.units,
    promo: train.promo.map((row, s) => [...row, ...h.promo[s]!]),
    holiday: [...train.holiday, ...h.holiday],
  };
  return [{ table, origin: TRAIN_WEEKS, actual: h.units }];
}

export function score(folds: Fold[]) {
  const actual: number[][] = folds[0]!.actual.map(() => []);
  const forecast: number[][] = folds[0]!.actual.map(() => []);
  let ms = 0;
  for (const f of folds) {
    const t0 = performance.now();
    const fc = predict(f.table, f.origin, HORIZON);
    ms += performance.now() - t0;
    f.actual.forEach((row, s) => {
      actual[s]!.push(...row);
      forecast[s]!.push(...fc[s]!);
    });
  }
  const a = accuracy(actual, forecast);
  const round = (x: number, d: number) => Number(x.toFixed(d));
  return { wape: round(a.wape, 4), mape: round(a.mape, 4), bias: round(a.bias, 4), train_ms: round(ms / folds.length, 2) };
}

export function leakageViolations(train: SalesTable): number {
  return findLeaks(predict, train, LEAK_PROBE_ORIGINS, HORIZON).length + scanSources().length;
}

const isMain = process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href;
if (isMain) {
  const train = trainSales();
  const holdout = process.env.RATCHET_TRACE;
  const folds = holdout ? holdoutFolds(train, JSON.parse(readFileSync(holdout, 'utf8')) as HoldoutFile) : backtestFolds(train);
  console.log(JSON.stringify({ ...score(folds), leakage_violations: leakageViolations(train) }));
}
