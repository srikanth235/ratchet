# Ratchet brief: {{verb}} `{{metric}}`

**Goal:** {{direction}} `{{metric}}`, the time from launching the command line to its exit, measured by
`bench/cold-start.mjs` over the invocations listed in `bench/cold-start.config.json`. A change counts only if the
trusted benchmark, which you cannot edit, confirms it, then again on hidden invocations you do not see. Your own
measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`, lockfiles and install config. A candidate that touches
  them is blocked.
- Keep behaviour: the guardrails (the tests, the error rate) must stay green; output must not change.
- Don't special-case the listed invocations: the final check runs different ones of the same kind.
- Don't add dependencies: the benchmark machine has no network and installs from the base lockfile.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                       # must pass before and after
node bench/cold-start.mjs      # prints the benchmark JSON including `cold_start_ms.p50`
BENCH_RUNS=30 node bench/cold-start.mjs
```

Start with what runs before `main`: module loading, config and plugin discovery, work done eagerly that could be lazy.

TODO: describe the tool (entry point, build step, what is already known to be slow).
