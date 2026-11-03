# Ratchet brief: make the sales forecast more accurate

**Goal:** lower `wape` (weighted absolute percentage error, in percent) of the
weekly SKU demand forecast. A change counts only if a trusted evaluator, which
you cannot see or edit, confirms it: a rolling-origin backtest over the training
period (7 forecast origins, 12-week horizon), then the same pipeline scored on a
sealed holdout, the 12 most recent weeks, which you never see. Your own
measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `eval/**`, `data/**`, `ratchet.yaml`,
  `package.json`, `src/types.ts`, `tests/contract/**`, `SOLUTIONS.md`. A
  candidate that touches them is blocked.
- Keep the interface: `predict(table, origin, horizon)` in `src/predict.ts`
  returns `[sku][h]`, finite and non-negative. The contract tests pin it.
- **No look-ahead.** A forecast made at `origin` may use `table.units` only for
  weeks **before** `origin`. The table you are handed also contains later weeks
  (the backtest scores against them); touching them is leakage. The promo and
  holiday calendars are plans, known in advance, so they may be used through
  `origin + horizon - 1`. The evaluator scrambles everything you may not see
  and requires the forecast not to change (it reports `leakage_violations`,
  which must be 0; `pnpm test:leakage` runs the same check); a candidate that
  fails it is blocked, however good its backtest looks. Don't read the data
  generator, files or the clock from `src/` either.
- Don't memorize the backtest. The final check is on weeks you have not seen. A
  gain that exists only on the backtest origins (constants fitted to them,
  special cases for particular weeks) is rejected.
- Don't add dependencies. Keep a fit + forecast of all 200 SKUs under 2 s
  (`train_ms`); the baseline takes under a millisecond.
- One idea per commit, with a message saying what you measured. To submit a
  candidate, add the trailer `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
pnpm install
pnpm test                    # unit + contract tests; must pass before and after
pnpm backtest                # builds, then prints {"wape", "mape", "bias", "train_ms", "leakage_violations"}
```

`wape` is deterministic (same code, same number), so any difference is real, but
a single backtest has only 7 origins: prefer ideas that have a reason to work
over tuning constants. `bias` (signed, percent) shows whether you over- or
under-forecast; the pipeline is `src/features.ts` (what the model sees),
`src/model.ts` (the model), `src/predict.ts` (glue). The data generator in
`data/generate.ts` documents what drives the sales; read it to understand the
problem, not to reproduce data you are not given.
