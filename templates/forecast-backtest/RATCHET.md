# Ratchet brief: {{verb}} `{{metric}}` of the forecast

**Goal:** {{direction}} `{{metric}}`, reported by the rolling-origin backtest in `eval/backtest.mjs`: the forecast is made
at several past origins and scored on the weeks that followed. A change counts only if the trusted evaluator, which you
cannot edit, confirms it, then again on a sealed holdout (the most recent weeks), which you never see.

## Rules

- Do **not** modify protected paths: `eval/**`, `data/**`, `ratchet.yaml`, lockfiles and install config. A candidate that
  touches them is blocked.
- **No look-ahead.** `forecast(history, horizon)` in `src/forecast.mjs` may use only the history it is handed. The evaluator
  reports `leakage_violations`, which must be 0.
- Don't memorize the backtest: constants fitted to particular origins are rejected on the holdout.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                # must pass before and after
node eval/backtest.mjs  # prints { wape, mape, bias, leakage_violations }
```

TODO: describe the data (granularity, seasonality, promotions, stockouts) and what you know drives the demand.
