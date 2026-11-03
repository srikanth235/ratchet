# Ratchet brief: make `taskr` start faster

**Goal:** lower `cold_start_ms.p50`, the median wall-clock time (ms) of one
`node dist/src/main.js <args>` process, from `exec` to exit, over a fixed set of
invocations (`list`, `show`, `stats`, `search`, `export`, `due`, `config`, ...) in
a monorepo-shaped workspace. A change counts only if a trusted benchmark, which
you cannot see or edit, confirms it, on a **different, hidden set of
invocations**. Your own measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`, `package.json`,
  `tests/contract/**`, `tests/golden/**`, `SOLUTIONS.md`. A candidate that
  touches them is blocked.
- Output must stay **byte-identical**: `tests/golden/*.out` records what every
  command prints (stdout, stderr and exit code). `pnpm test:contract` checks the
  built CLI against it. Do not change what a command prints, or which config
  file, plugin or message applies.
- Keep behaviour, not just output: config from the nearest `.taskrrc.json` /
  `package.json` `taskr` key wins, a `"root": true` file ends the search, plugins
  are the `taskr-plugin-*` / `@taskr/plugin-*` packages found above the working
  directory, `TASKR_NOW` fixes the clock. The unit tests pin these.
- The message catalogs in `data/messages/` belong to the localisation team: do
  not delete messages to make them load faster.
- Don't add dependencies, bundlers or native code; improve the code itself.
- Don't special-case the benchmark invocations. The final check runs another
  mix of the same kinds of command, and caching results between runs does not
  help: the benchmark starts every run with empty `HOME`, `TMPDIR` and cache
  directories. A fast path for exact argument lists is an overfit and is
  rejected on the hidden mix.
- One idea per commit, with a message saying what you measured. To submit a
  candidate, add the trailer `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
pnpm install
pnpm test                     # builds, then unit + contract tests; must pass before and after
pnpm bench:spawn              # builds, then times the visible invocations (default: 4 starts each)
node dist/bench/spawn.js --runs 8 --keep 0.67   # noisier machine: keep the fastest two thirds of each invocation's runs
node --cpu-prof dist/src/main.js list --limit 5   # profile one start (run it in a workspace: see bench/workspace.ts)
```

`bench:spawn` prints `{ cold_start_ms: { p50, p95, mean, min, max }, invocations, runs, error_rate }`.
The real evaluator uses [hyperfine](https://github.com/sharkdp/hyperfine) on pinned hardware
(`node dist/bench/hyperfine.js`, same JSON); `spawn.js` is its pure-Node stand-in. Cold starts on a shared
machine vary by tens of percent: repeat a measurement before trusting a small difference, and **profile
first**: a start is mostly node booting, then module loading, then whatever your code does before the
command's own work.
