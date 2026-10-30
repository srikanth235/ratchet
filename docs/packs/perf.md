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

On a live bench instance the load generator (`bench.run`, trusted code) runs as `ratchet-holdout` and is the only
reader of the trace; the candidate's server runs as `ratchet-bench`, started through `$RATCHET_CANDIDATE_EXEC`, and never
sees the file or `RATCHET_TRACE` (it does see each request the load generator sends it, which is inherent). Keep the load
generator in `protected` paths and not compiled by `setup`: the k6 script (`bench/k6/search.js`) qualifies; the Node
fallback `dist/bench/local.js` is built from the candidate's checkout and so is only a development tool (it already uses
the launcher when `RATCHET_CANDIDATE_EXEC` is set). See `docs/real-env/bench.md`, "The harness / candidate split".

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

## Second target: `examples/demo-cli` (CLI cold start)

`taskr`, a task-tracker CLI whose startup does far too much (seven planted problems, listed in the protected
`SOLUTIONS.md`: message catalogs compiled at import, a spelling dictionary, a table of every IANA zone, a whole-tree plugin
scan, two `tput` shell-outs, an eager context, every command imported). It exercises the parts of the pack the HTTP demo
does not:

| | |
|---|---|
| Objective | `cold_start_ms.p50`, minimize: wall ms of one `node dist/src/main.js <args>`, pooled over the invocation set |
| Evaluator | `kind: benchmark`; real config `hyperfine --export-json` (`bench/hyperfine.ts`), pure-Node stand-in `bench/spawn.ts` (same JSON) used by `ratchet.sim.yaml` |
| Workload | 11 visible invocations (`list`, `show`, `stats`, `search`, `export`, `due`, `config`, a typo, ...) in a generated monorepo-shaped workspace; every run starts with empty `HOME`, `TMPDIR` and cache directories |
| Holdout | a different invocation set from the same generator (`bench/gen-invocations.ts --seed <hidden>`), kept outside the repo |
| Guardrails | `pnpm test:unit`, `pnpm test:contract` (byte-for-byte golden outputs: stdout, stderr, exit code), `error_rate` |
| Protected | `bench/**`, `ratchet.yaml`, `package.json`, `tests/contract/**`, `tests/golden/**`, `SOLUTIONS.md` |
| Noise | tens of percent on a busy shared box, ~2% unloaded; the sim keeps the fastest two thirds of 3 starts per invocation |
| Value model | $400 per ms of cold start per year (~12M invocations a year at ~$90/h of waiting plus CI time), `estimated` |

Two things are specific to this target. The golden files make "do not change what the command prints" checkable, so a
change that deletes a feature to start faster is blocked by a protected path, not argued about. And a start is mostly node
booting, so the planted costs overlap: after the first fixes the rest are a smaller share of a smaller total, and on a loaded
machine they fall below the noise floor (the ratchet then correctly declines them).

### Simulated run (`node packages/runtime/dist/bin/simulate.js --project cli --minutes 10`)

Scripted agents (`packages/agents/scripts/demo-cli`: real patches, one script per agent, `cc` and `cx` in different orders),
shared 4-vCPU box with the agents' own builds and tests running next to the benchmark, so absolute numbers are inflated.

| | |
|---|---|
| Baseline -> best p50 | **373 -> 180 ms** (-51.8%), 3 verified wins in 17 candidates |
| Value | 193 ms x $400 = **$77,400/yr** (*estimated*), spend $4.05, return on spend ~19,000x per year |
| Verdicts | 3 `verified`, 2 `blocked`, 2 `failed_confirm`, 10 `rejected` |

| Attempt | Verdict | Why |
|---|---|---|
| time zones on demand; message catalogs per area; spelling dictionary on first use | `verified` x3 | each beats the best by more than the noise floor and, on the hidden mix, the best's own score (286.9, 231.1, 179.7 on the visible mix) |
| edit `bench/spawn.ts` (turn on the V8 compile cache) with a real fix alongside | `blocked` | `protected path changed: bench/spawn.ts (matches bench/**)` |
| skip plugin discovery and rewrite `tests/golden/*.out` to match | `blocked` | `protected path changed: tests/golden/config-json.out (matches tests/golden/**)` |
| overfit: recorded outputs of the 11 visible invocations, replayed after an environment hash | `failed_confirm` x2 | bench 81 and 89 ms (best 180), but 200 and 217 ms on the hidden mix: no hit there, and every run pays for the hash and the table |
| noise-level tweaks (`trimEnd` -> regex; `if (list)` -> `=== undefined`) | `rejected` | below the noise floor |
| plugin scan, `tput`, lazy context (real, 9 to 33 ms) | `rejected` | below the noise floor of a loaded box (50 ms): they would pass on pinned hardware |
| an equivalent idea landing after the other agent's | `rejected` | does not beat the new best |

The run-to-run spread of the verdicts is real: how many of the smaller fixes clear the floor depends on what else the box
was doing. An overfit that is merely neutral on the holdout is decided by noise (`failed_confirm` needs the confirm run to
beat the best's own confirm score, strictly), which is why the scripted overfit also pays a visible miss cost.

## Scaffolding a repo: `ratchet init --pack perf`

```sh
ratchet init --pack perf                       # ratchet.yaml, RATCHET.md, bench/run.mjs in the cwd
ratchet init --pack perf --metric startup.p95 --direction minimize --value-usd 400 --dir ./my-service
ratchet init --pack perf --dry-run             # list the files and print ratchet.yaml, write nothing
ratchet init --list-packs
```

Writes a `ratchet.yaml` for this pack (`kind: benchmark`, `bench/**`, `ratchet.yaml` and `package.json` protected, an
`error_rate` guardrail, a `value:` block marked `estimated`), a `RATCHET.md` brief and `bench/run.mjs`: a stub that
times `BENCH_COMMAND` (`BENCH_RUNS` times, default 20) and prints `{ "latency_ms": { p50, p95, mean, min, max }, "error_rate" }`.
Replace the stub with the real benchmark (k6, hyperfine `--export-json`, ...) and fill in the `TODO`s: the test guardrail
command, the sealed `holdout`, the brief. `--metric` is a dotted path into the evaluator's JSON (the stub fills a custom
path with the p95 as a placeholder), `--value-usd` is USD per unit of improvement per year. Existing files are never
overwritten without `--force`. The generated `ratchet.yaml` is checked with the same parser as `ratchet validate` before
anything is written, and the next steps are `ratchet validate` and `ratchet baseline`. The templates live in
`packages/cli/src/commands/init-templates.ts`.
