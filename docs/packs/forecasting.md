# Pack: forecasting (demand and sales forecasts)

The second pack, shipped as `examples/demo-forecast`. It proves the core is generic: the same engine, ratchet rule,
protected paths and ledger, with a different evaluator, holdout and guardrails. Agents improve a forecasting pipeline;
a change is kept only when a rolling-origin backtest improves **and** the pipeline scores better on a sealed, more
recent period it never saw, **and** nothing looked into the future.

| | |
|---|---|
| `pack` | `forecasting` |
| Objective | `wape` (percent), minimize; `mape` and `bias` are reported alongside |
| Evaluator `kind` | `backtest`: rolling-origin over the training period, prints `{wape, mape, bias, train_ms, leakage_violations}` |
| Holdout | the most recent 12 weeks, materialized outside the repo and scored only in the confirm phase |
| Guardrails | unit tests, interface contract, **leakage (look-ahead) probe** (`leakage_violations == 0`), `train_ms` cap |
| Noise | none for WAPE (deterministic data and code); `train_ms` jitters |
| Value model | $ per WAPE point per year (stockouts and inventory carrying cost) |

## Evaluator design

`node dist/eval/backtest.js` builds the training table (200 SKUs x 144 weeks), then for each of 7 forecast origins
(weeks 96, 102, ..., 132) calls `predict(table, origin, 12)` and compares the 12 forecast weeks with what was sold.
WAPE is `sum|actual - forecast| / sum actual` over all folds, in percent: volume-weighted, so the fast sellers dominate,
which is what inventory money follows. MAPE (over points with sales) and the signed `bias` are printed too: MAPE is
dominated by slow SKUs and is a poor objective, bias explains *why* a model is wrong (a stockout-polluted history shows
as -5%). `train_ms` is the mean wall time of one `predict()` (fit + forecast of all SKUs).

- **Why rolling origin.** One train/test split would let an agent tune to a single window; seven origins spanning a
  year of seasons make that harder (not impossible: see the holdout). Origins are spaced every 6 weeks so folds overlap
  by half; the folds are not independent, which is why a backtest gain alone is never enough.
- **Determinism.** Data and code are deterministic, so repeated runs give the identical WAPE. The noise floor is 0 and
  `min_gain` (3% relative) alone decides. With stochastic models (random initialization, bagging) set `repeats` > 1
  and seed from `RATCHET_RUN`; the floor then comes from the baseline repeats as in any pack.
- **The interface.** `predict(table, origin, horizon) -> [sku][h]`, finite and non-negative; `src/types.ts` is protected.
  The table deliberately also contains the sales *after* `origin` (the backtest scores against them). That is the
  realistic hazard: customer pipelines load a whole table and filter by date, and a missing filter is the classic leak.

## Holdout strategy

The training data ends at week 143. The holdout is the 12 weeks after it. `eval/holdout.js --seed <hidden> --out <path>`
draws them from a seed the agents do not have and writes them **outside the repository**; the engine exports the path as
`RATCHET_TRACE` in the confirm phase only, and the same `backtest.js` then fits on all the training weeks, forecasts
weeks 144 to 155 and scores them. The table handed to `predict` in that run holds no sales at or after the origin
(only the planned calendar), so a pipeline that reads the future gets nothing.

A candidate must beat the best's *own* holdout WAPE (`confirmBest`) by any margin; a gain that exists only on the 7
backtest windows (constants fitted to them) is a `failed_confirm`. The holdout is also the *most recent* period
(Oct to Dec, holiday-heavy), so it tests the deployment condition: forecasting the future from the past.

Design choices to keep: (1) time-ordered, never random; (2) one-shot per candidate (it is the *confirm* run, not a
dev set; if agents learn the holdout through verdict feedback it is spent and must be rotated forward); (3) the
holdout's calendar (promotions, holidays) is given, because a real planner knows it at forecast time, but its sales
are not.

## Leakage guardrails

Leakage is the main way a forecasting optimizer cheats, so it gets a dedicated guardrail (`eval/leakage.ts`, protected).
It is part of the **trusted evaluator**, not a test in the candidate's tree: `backtest.js` runs it and reports
`leakage_violations`, and `ratchet.yaml` has the threshold guardrail `leakage_violations: "== 0"`. A leaking candidate
therefore finishes its bench run (its too-good WAPE is on record) and is **blocked** with a reason naming the guardrail.

1. **Look-ahead invariance.** At three origins, scramble everything the pipeline is not allowed to see: sales on and after
   the origin (reversed in time, x10, zeroed, NaN, shuffled per SKU) and the calendar beyond the horizon. The forecast must be
   bit-identical. A pipeline that touches `table.units[sku][origin + h]`, blends in a little of it, or uses
   `slice(0, origin + 1)` (off by one) fails. `tests/contract/leakage.test.ts` (`pnpm test:leakage`) runs the same code for
   developers and checks that the detector catches known-leaky pipelines, so it cannot silently go vacuous.
2. **No side channels.** The pipeline source may not import the data generator, the evaluator, `fs`/network/process
   modules, use dynamic imports, `process`, `Date.now` or `Math.random`.
3. **Contract.** `tests/contract/predict.test.ts` (`pnpm test:contract`, a command guardrail): shape, finiteness,
   non-negativity, determinism, input not mutated.
4. `protected`: `eval/**`, `data/**` (the generator, so noise cannot be turned down), `ratchet.yaml`, `package.json` (it defines
   the guardrail commands), `src/types.ts`, `tests/contract/**`, `SOLUTIONS.md`.
5. `train_ms: "< 2000"` (an absolute threshold guardrail): a model may get more expensive, not unboundedly so.

Why the leakage check lives in the evaluator rather than a command guardrail: a command guardrail that fails stops the bench
before any sample exists, and today's engine does not turn a sample-less outcome into a `blocked` verdict
(see "Engine limitations found" below). A threshold guardrail on a metric the evaluator reports always yields a clean `blocked`.

What this does *not* catch: a leak that is invariant to the scramble (a constant derived from the future and pasted in as
a literal); that is an overfit, and the holdout catches it. A copy of the generator pasted into `src/` is caught only by
the holdout (the sealed weeks are not reproducible from it). In a real repo, pair the probe with data-layer enforcement: run
the candidate where only an "as of origin" extract is mounted.

## Value model

`objective.value.usd_per_unit` is dollars per WAPE point per year. The demo assumes the assortment sells about 550k units a
year, so one point is about 5.5k units/yr of forecast error, each costing about $3.6 (lost margin on stockouts plus carrying
cost and markdowns on overstock): $20,000 per point. Moving WAPE from 43.5 to 24.1 is then about $388k/yr, which
`estimateValue` reports as *estimated*; `returnOnSpend` divides it by the run's spend. Replace it with the customer's number
(see below).

## What to verify in a real customer repo

1. **Does the backtest reproduce how the forecast is used?** Same horizon and lead time (forecast made *n* days before the
   decision), same granularity (SKU x store x week), same cutoff logic. A backtest at the wrong lead time is optimistic.
2. **As-of correctness of every feature.** Late-arriving data (returns, restated sales, revised prices) and "current"
   dimension tables (a product's *today* category) leak silently. The origin cut must apply to every table, and the leakage
   probe must scramble each of them.
3. **Which calendar is known when.** Promotions and prices planned in advance are fair inputs; promotions *recorded after the fact*
   are not. Encode exactly what the planner has at the origin.
4. **Censored demand.** Sales are not demand when a SKU is out of stock; check how stockouts, returns and substitutions are
   recorded before choosing the target.
5. **The metric matches the money.** WAPE for volume-weighted error; add a bias or service-level guardrail if over- and
   under-forecasting cost differently (they usually do). Hierarchy matters: a SKU-level win that worsens the category
   total can be a loss.
6. **Holdout hygiene.** Sealed, outside what the agent identity can read, long enough to cover a full seasonal pattern
   or at least the peak, and rotated forward when it has been used. Check the baseline's WAPE on it is comparable to the
   backtest's.
7. **Value per point** from the customer's finance numbers (inventory cost, lost-sales margin, expediting), per period.
8. **Compute and runtime.** `train_ms` cap matched to the real batch window; memory limits if the pipeline is wider.
9. **Variance.** Stochastic models: set `repeats`, seed from `RATCHET_RUN`, and let the baseline repeats set the floor.

## Simulated run (scripted agents, `--project forecast --repeats 2 --minutes 5`)

Two scripted agents (`cc`, `cx`) play five real improvements and four kinds of bad attempt against the real evaluator,
guardrails and ratchet rule; only AWS and the agent CLIs are mocked. Result of one run:

| | |
|---|---|
| Baseline -> best WAPE | **43.50 -> 24.06** (-44.7%), 5 verified wins in 22 candidates |
| Sealed holdout, best | 23.09 (baseline 39.69), checked by the confirm run of every win |
| Value | 19.44 WAPE points x $20,000 = **$388,878/yr** (*estimated*), spend $4.33, return on spend ~89,800x per year |

| Attempt | Verdict | Why |
|---|---|---|
| seasonal naive; smoothed level x multi-year indexes; promo + holiday effects; stockout mask; pooled profiles | `verified` x5 | each beats the best by > 3% on the backtest and by any margin on the holdout (39.60, 32.57, 28.61, 25.55, 24.06) |
| edit `eval/backtest.ts` (`ORIGINS = [132]`) / `data/generate.ts` (less noise) with a good patch | `blocked` | `protected path changed: ... (matches eval/**)` |
| leakage: blend in `table.units[s][origin + h]` (100%, and a "30% online correction") | `blocked` | `guardrail failed: leakage_violations == 0 (leakage_violations = 15)`; the 100% cheat scored backtest WAPE 0, the 30% blend 23.2 on a best of 32.6 |
| overfit: per-origin, per-SKU window means of the 7 backtest folds | `failed_confirm` | bench 20.41 (best 25.55), but 23.60 on the holdout = the best's own score: no gain at origin 144 |
| noise-level tweak (`WINDOW 4 -> 5`) | `rejected` | identical score, below `min_gain` |
| an equivalent idea landing after the other agent's | `rejected` | does not beat the new best (the ratchet is a race) |

## Engine limitations found while building this pack

1. **A failing command guardrail is not `blocked`.** Fixed since: a guardrail that fails before any evaluator run
   now blocks the candidate outright (`engine.test.ts`, "a guardrail that fails before any bench run"). The pack still
   reports `leakage_violations` from the evaluator, so the reason names the leak.
2. **Repeated identical candidates.** A scripted agent whose patch is already in the best submits a tree identical to the
   best, which lands as `rejected` ("not better than current best"). Harmless, but it inflates the candidate count.

## Scaffolding a repo: `ratchet init --pack forecasting`

```sh
ratchet init --pack forecasting                # ratchet.yaml, RATCHET.md, eval/backtest.mjs, src/forecast.mjs
ratchet init --pack forecasting --metric mape --value-usd 8000 --dir ./my-forecaster
ratchet init --pack forecasting --dry-run
```

Writes a `ratchet.yaml` for this pack (`kind: backtest`, `eval/**`, `data/**`, `ratchet.yaml` and `package.json` protected,
a `leakage_violations: "== 0"` guardrail, a `value:` block marked `estimated`), a `RATCHET.md` brief, a rolling-origin
backtest stub `eval/backtest.mjs` (5 origins, 4-period horizon; each forecast is handed only the history before its
origin; prints `{ wape, bias, leakage_violations }`; `RATCHET_TRACE` names the sealed holdout series) and the model it
scores, `src/forecast.mjs`, a mean-of-the-last-4-periods baseline to be replaced. The stub runs on a seeded synthetic
series: replace `loadSeries()` with the real data, and add a real leakage probe if the pipeline loads data itself. Options
as for `perf`: `--metric`, `--direction`, `--value-usd`, `--dir`, `--force` (without it existing files are never
overwritten), `--dry-run`, `--list-packs`. The output passes `ratchet validate`.

## Using the demo

```sh
pnpm --filter @ratchet/demo-forecast test            # 28 unit + contract tests
pnpm --filter @ratchet/demo-forecast backtest        # {"wape": 43.5043, "mape": ..., "bias": ..., "train_ms": ...}
pnpm build && node packages/runtime/dist/bin/simulate.js --project forecast   # whole run, scripted agents
```
