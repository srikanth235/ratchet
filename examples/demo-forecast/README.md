# @ratchet/demo-forecast

A weekly sales-forecasting pipeline over a deterministic synthetic dataset (200 SKUs x 3 years), used as the first
target of Ratchet's **forecasting pack**. The pipeline is deliberately naive (a 4-week moving average), its weaknesses are
listed only in the protected `SOLUTIONS.md`, the interface and the no-look-ahead rule are pinned by contract tests, and
the evaluator is a rolling-origin backtest that reports `wape`. TypeScript/Node only, no dependencies.

```sh
pnpm build && pnpm backtest          # {"wape":43.5043,"mape":120.6385,"bias":2.1125,"train_ms":...,"leakage_violations":0}
pnpm test                            # unit + contract (incl. leakage) tests
```

- Brief for agents: `RATCHET.md`. Repo contract: `ratchet.yaml` (`pack: forecasting`). Pack docs: `docs/packs/forecasting.md`.
- Pipeline: `src/features.ts` (what the model sees: `buildSeries`), `src/model.ts` (`fit` / `forecast`), `src/predict.ts`
  (`predict(table, origin, horizon) -> [sku][h]`, pinned), `src/types.ts` (the data contract, protected).
- Data: `data/generate.ts` (protected). Per SKU: trend, yearly seasonality shared within 8 categories (with per-SKU
  jitter), planned promotions (~10% of weeks), holidays (Thanksgiving, Christmas, July 4, and Easter, which moves between
  years), stockouts (1 to 3 weeks of near-zero sales) and lognormal noise. Every draw is a pure function of
  `(seed, sku, week)`, so the 144 training weeks are a stable prefix of any longer series.
- Tests: `tests/unit` (data, features, model behaviour, metrics, evaluator) and `tests/contract`: `predict.test.ts` (the
  `predict()` interface, the `test:contract` guardrail) and `leakage.test.ts` (`pnpm test:leakage`, the dev-time view of
  `eval/leakage.ts`: scrambles all sales on and after the forecast origin and requires an identical forecast, and bans side
  channels to the data).

## Evaluator

| | |
|---|---|
| `eval/backtest.ts` | `node dist/eval/backtest.js`: 7 forecast origins (weeks 96, 102, ..., 132), horizon 12. Prints `{ "wape", "mape", "bias", "train_ms", "leakage_violations" }` (percent; `train_ms` = mean ms of one `predict()` call; `leakage_violations` = look-ahead probe + source scan, must be 0) |
| `eval/leakage.ts` | the leakage guardrail: scrambles sales from the origin on and calendar beyond the horizon at 3 origins, 5 ways, and counts forecasts that change; scans `src/` for side channels |
| `RATCHET_TRACE=<file>` | scores the sealed holdout instead: fit on all 144 training weeks, forecast weeks 144 to 155, compare with the sealed sales |
| `eval/holdout.ts` | `node dist/eval/holdout.js --seed <hidden> --out <path>` writes the holdout (the 12 weeks after training, drawn from the hidden seed) **outside the repo** |
| `eval/metrics.ts` | WAPE `sum|a-f| / sum a`, MAPE over points with sales, signed bias `sum(f-a) / sum a` |

### Baseline (4-week moving average)

| | WAPE | MAPE | bias |
|---|---|---|---|
| backtest | 43.50 | 120.64 | +2.11 |
| sealed holdout (seed 424242) | 39.69 | 78.00 | -22.43 |

(The holdout is Oct to Dec: a flat average under-forecasts the holiday weeks by a fifth. MAPE is huge on both because slow
SKUs sell 0 to 5 units a week; WAPE is the objective.)

### Run-to-run variance

`wape`, `mape` and `bias` are **exactly reproducible**: the data and the pipeline are deterministic, so five consecutive
runs print the same 43.5043, and the noise floor the ratchet derives from the baseline repeats is 0; `min_gain: 0.03` alone
(3% of the best, 1.3 WAPE points at the baseline) decides. Only `train_ms` varies: 0.3 to 0.6 ms for the baseline between
runs on a quiet machine (with an occasional outlier of a few ms on a cold start), roughly 40 to 60 ms for the fully
improved pipeline. The guardrail `train_ms: "< 2000"` is absolute and far above that jitter.

A model with randomness (bagging, random init) would add run-to-run variance: seed it from `RATCHET_RUN` and raise
`evaluator.repeats`; the baseline repeats then set the noise floor as in any other pack.

### What the five improvements are worth

Applied cumulatively (`packages/agents/scripts/demo-forecast/*.patch`; details in the protected `SOLUTIONS.md`):

| | backtest WAPE | holdout WAPE |
|---|---|---|
| baseline | 43.50 | 39.69 |
| + seasonal naive | 39.60 | 38.83 |
| + smoothed level x multi-year seasonal indexes | 32.57 | 29.55 |
| + promo and holiday effects | 28.61 | 26.57 |
| + pooled seasonal profiles | 26.05 | 25.04 |
| + stockout bias correction | 24.06 | 23.09 |

## Simulating a run

```sh
pnpm build
node packages/runtime/dist/bin/simulate.js --project forecast   # two scripted agents, real evaluator, mock cloud
```

`ratchet.sim.yaml` is the overlay the simulator merges over `ratchet.yaml` (local holdout file, 2 repeats); it is not part of
the customer-facing contract.
