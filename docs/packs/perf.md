# Pack: perf (code performance)

The first pack, shipped as `examples/demo-search`. It makes a service faster (or cheaper) and keeps a change only
when a trusted benchmark, on pinned hardware, confirms it on a workload the agents never saw.

| | |
|---|---|
| `pack` | `perf` |
| Objective | `http_req_duration.p95` (ms), minimize; any benchmark metric works (p50/p99, throughput, `$ per request`) |
| Evaluator `kind` | `benchmark`: k6 (constant arrival rate), criterion, hyperfine, a custom script that prints a JSON object |
| Holdout | a hidden traffic trace / query mix drawn from the same distribution but a different seed |
| Guardrails | unit tests, API contract (OpenAPI + golden result orderings), `error_rate`, `peak_rss` |
| Noise | run-to-run variance of a load test (about ±12% for p95 on shared hardware) |
| Value model | $ per ms of p95 per year (infra spend + conversion) |

## Evaluator design

`evaluator.run` prints a JSON object; the last one on stdout is parsed. `objective.metric` is a dotted path into it and
every other numeric leaf is kept as a metric (`peak_rss_mb`, `error_rate`), so a guardrail can reference it without
extra plumbing. The bench pipeline (shared by the local and the remote runner) is: checkout candidate, overlay the
protected files from `evaluator.ref`, setup, command guardrails (fail fast), `evaluator.run` x `repeats`, threshold
guardrails over the per-metric medians.

- **Pinned hardware.** `hardware: c7i.2xlarge` on a dedicated host; the load generator is separate from the server.
  Shared machines (the local simulation) are for plumbing, not for believing small wins.
- **Repeats and the noise floor.** The baseline is measured `repeats` times before any agent starts; the standard
  deviation of those runs is the noise floor. A candidate must beat the best by more than
  `max(noise_floor, min_gain x best)` on the median of its own `repeats` runs. p95 is dominated by queueing behind
  occasional slow requests, so it is the noisiest latency statistic; `min_gain: 0.03` is only meaningful with
  repeats on pinned hardware.

## Holdout strategy

The agents can run the benchmark on `bench/query-mix.json` as often as they like. The confirm phase replays a hidden mix
(`bench/traces/generate.ts --seed <hidden>`, kept in a sealed bucket, exported as `RATCHET_TRACE` for the confirm run
only) on a fresh instance. A win must also beat the best's own score on that mix, so special-casing the visible queries
(a cache keyed on the benchmark's strings, a precomputed answer table) shows up as a `failed_confirm`.

## Guardrails

- `pnpm test:unit`, `pnpm test:contract`: the contract suite validates responses against `openapi.yaml` and pins result
  *ordering* in `golden.json` (a faster but differently ordered search is a different product). They live in
  `tests/contract/**`, which is protected.
- `error_rate: "< 0.001"`: a faster server that sheds load or returns 500s is not faster.
- `peak_rss: "< 1.2x"`: relative to the baseline, so caching cannot buy latency with unbounded memory.
- `protected`: `bench/**`, `ratchet.yaml`, `tests/contract/**`, `SOLUTIONS.md`. Touching one blocks the candidate, and
  the trusted copies are re-applied from `evaluator.ref` before every step, so a test that rewrites the bench changes
  nothing.

## Value model

`objective.value.usd_per_unit: 1200` is dollars per ms of p95 improvement per year (infrastructure that no longer has to
be provisioned plus the conversion uplift of a faster page). `estimateValue(config, baseline, best)` multiplies the
improvement by it; `returnOnSpend` divides the yearly value by the run's spend. It is labelled `estimated` until measured
in production (`basis: measured`).

## What to verify in a real customer repo

1. The benchmark reproduces production behaviour: representative request mix, data size, warm-up, a constant *arrival*
   rate (not closed-loop), the same runtime flags and instance family.
2. Baseline noise: 5 baseline runs on the pinned host; coefficient of variation well under `min_gain`. If not, raise
   `repeats` or move to a quieter metric.
3. Contract coverage is the real limit on what agents may change: golden outputs for every endpoint they could touch.
4. The holdout is genuinely hidden (separate bucket, not readable by the agent identity) and really a different sample.
5. The `$ per ms` figure is the customer's own, not the demo's.
