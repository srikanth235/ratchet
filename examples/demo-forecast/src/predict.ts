/** Entry point of the pipeline. The signature and output shape are pinned by tests/contract. */
import { buildSeries } from './features.js';
import { fit, forecast } from './model.js';
import type { Forecast, SalesTable } from './types.js';

export function predict(table: SalesTable, origin: number, horizon: number): Forecast {
  const series = buildSeries(table, origin, horizon);
  const model = fit(series);
  return forecast(model, horizon);
}
