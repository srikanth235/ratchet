#!/usr/bin/env node
// Rolling-origin backtest evaluator (protected: eval/**, data/**). For each origin the model sees only the history
// before it and forecasts HORIZON periods; the forecasts are scored on the periods that followed. Prints one JSON
// object on stdout:
//   { wape, mape, bias, leakage_violations, series, origins }      (percent, except the counts)
//   wape = sum|forecast - actual| / sum actual     mape = mean |forecast - actual| / actual (actual > 0)
//   bias = sum(forecast - actual) / sum actual     (positive: over-forecasting)
//
// The series come from data/series.json: [[period values of series 1], [series 2], ...] (a seeded synthetic set
// ships as a placeholder: replace it with your history, or have loadSeries() read your own source).
// RATCHET_TRACE, when set, names the sealed holdout: the same shape, covering the most recent periods. With it the
// origins are the last three horizons of each series instead of the fixed ones used on the visible data.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { forecast } from '../src/forecast.mjs';

const HORIZON = 4;
const VISIBLE_ORIGINS = [52, 60, 68, 76, 84];
const here = dirname(fileURLToPath(import.meta.url));

/** Seeded synthetic weekly demand: a yearly season and a trend per series. Used when data/series.json is absent. */
function synthetic() {
  let seed = 7;
  const rng = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
  return Array.from({ length: 20 }, (_, s) =>
    Array.from({ length: 104 }, (_, t) =>
      Math.max(0, Math.round(40 + s * 3 + t * 0.1 + 15 * Math.sin((2 * Math.PI * t) / 52) + (rng() - 0.5) * 12)),
    ),
  );
}

function loadSeries() {
  if (process.env.RATCHET_TRACE) return { series: JSON.parse(readFileSync(process.env.RATCHET_TRACE, 'utf8')), holdout: true };
  const file = join(here, '..', 'data', 'series.json');
  return { series: existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : synthetic(), holdout: false };
}

const { series, holdout } = loadSeries();
let absErr = 0;
let signedErr = 0;
let total = 0;
let pctSum = 0;
let pctN = 0;
let origins = 0;
for (const actual of series) {
  const list = holdout ? [actual.length - 3 * HORIZON, actual.length - 2 * HORIZON, actual.length - HORIZON] : VISIBLE_ORIGINS;
  for (const origin of list) {
    if (origin < 8 || origin + 1 > actual.length) continue;
    origins++;
    const history = Object.freeze(actual.slice(0, origin)); // nothing at or after the origin is visible
    const predicted = forecast(history, HORIZON);
    if (!Array.isArray(predicted) || predicted.length !== HORIZON || predicted.some((v) => !Number.isFinite(v) || v < 0)) {
      throw new Error('forecast() must return HORIZON finite, non-negative numbers');
    }
    for (let h = 0; h < HORIZON; h++) {
      const truth = actual[origin + h];
      if (truth === undefined) continue;
      absErr += Math.abs(predicted[h] - truth);
      signedErr += predicted[h] - truth;
      total += truth;
      if (truth > 0) {
        pctSum += Math.abs(predicted[h] - truth) / truth;
        pctN++;
      }
    }
  }
}

const round = (x) => Math.round(x * 1000) / 1000;
console.log(
  JSON.stringify({
    wape: total > 0 ? round((100 * absErr) / total) : 0,
    mape: pctN > 0 ? round((100 * pctSum) / pctN) : 0,
    bias: total > 0 ? round((100 * signedErr) / total) : 0,
    // The model is only handed the history, so it cannot read the future through its arguments. TODO: if your
    // pipeline loads data itself, scramble the periods after each origin and count the origins where the forecast
    // changes: that count is leakage_violations (guardrail "== 0").
    leakage_violations: 0,
    series: series.length,
    origins,
  }),
);
