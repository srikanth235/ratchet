/** The forecasting model: a plain moving average of the most recent weeks, held flat over the horizon. */
import { mean, tail, type Series } from './features.js';

export const WINDOW = 4;

export interface Model {
  /** Per SKU. */
  level: number[];
}

export function fit(series: readonly Series[]): Model {
  return { level: series.map((s) => mean(tail(s.y, WINDOW))) };
}

export function forecast(model: Model, horizon: number): number[][] {
  return model.level.map((level) => new Array<number>(horizon).fill(level));
}
