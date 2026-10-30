# @ratchet/demo-cli

`taskr`, a small task-tracker CLI (`list`, `show`, `stats`, `search`, `export`, `due`, `config`, `help`, `version`) over a
JSON task file. It is the second target of the `perf` pack and the one where the metric is **cold start**: wall-clock
ms of one `node dist/src/main.js <args>` process. Startup deliberately does far too much (planted problems, listed only
in the protected `SOLUTIONS.md`), the output is pinned byte for byte, and the benchmark runs a set of invocations in a
generated monorepo-shaped workspace.

```sh
pnpm --filter @ratchet/demo-cli build
cd examples/demo-cli && node dist/src/main.js list --limit 5     # needs a .taskrrc.json / data file: see bench/workspace.ts
```

## Tests

`pnpm test` builds, then runs

- `tests/unit`: message catalogs and ICU subset, dictionary, time zones, renderers, the workspace generator and the bench reducers;
- `tests/contract`: the built CLI against `tests/golden/*.out` (stdout, stderr and exit code, byte for byte) and the behaviours a
  startup optimization must not break (config cascade and `"root": true`, plugin discovery, `TASKR_NOW`, `COLUMNS`).
  Regenerate the golden files deliberately with `pnpm golden`; they are protected in `ratchet.yaml`.

## Benchmark

| | |
|---|---|
| `bench/invocations.json` | the visible mix: 11 invocations (`list`, `show`, `stats`, `search`, `export`, `due`, `config`, a typo, ...), each with its expected exit code |
| `bench/gen-invocations.ts` | `pnpm bench:mix -- --seed 424242 --out hidden.json`: a different mix of the same kinds of command (the sealed holdout) |
| `bench/hyperfine.ts` | the real evaluator: one [hyperfine](https://github.com/sharkdp/hyperfine) command per invocation with `--export-json`, pooled |
| `bench/spawn.ts` | pure-Node stand-in, same JSON: starts each invocation N times, `--keep 0.67` drops the slowest third on a loaded box |

Output: `{ "cold_start_ms": { "p50", "p95", "mean", "min", "max" }, "invocations", "runs", "error_rate" }`. Every run starts
with empty `HOME`, `TMPDIR` and cache directories, so nothing can be cached between runs.

Unloaded 4-vCPU box, 11 visible invocations: about 350 ms at baseline, about 85 ms with all seven planted problems fixed
(node itself is ~35 ms). On a busy shared machine the same numbers are 30 to 60% higher and noisy; `ratchet.sim.yaml`
(used by `node packages/runtime/dist/bin/simulate.js --project cli`) repeats each start and keeps the fastest two thirds.

## For agents

`RATCHET.md` is the brief. `ratchet.yaml` is the contract: objective `cold_start_ms.p50`, minimize, guardrails
`pnpm test:unit`, `pnpm test:contract` and `error_rate`, protected `bench/**`, `ratchet.yaml`, `package.json`,
`tests/contract/**`, `tests/golden/**`, `SOLUTIONS.md`.
