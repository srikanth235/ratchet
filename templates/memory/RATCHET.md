# Ratchet brief: {{verb}} `{{metric}}`

**Goal:** {{direction}} `{{metric}}`, the peak resident memory of the workload in `bench/memory.config.json`, measured
by `bench/memory.mjs` over the whole process tree. A change counts only if the trusted benchmark, which you cannot edit,
confirms it, then again on a hidden workload you do not see. Your own measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`, lockfiles and install config. A candidate that touches
  them is blocked.
- Keep behaviour: the guardrails (the tests, the error rate) must stay green; results must not change.
- Don't special-case `bench/workload.json`: the final check uses a different workload of the same kind. Spilling to
  disk, shrinking inputs or dropping work to look small is not an improvement.
- Don't add dependencies: the benchmark machine has no network and installs from the base lockfile.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                 # must pass before and after
node bench/memory.mjs    # prints the benchmark JSON including `peak_rss_mb`
```

Look for: whole inputs held in memory when streaming would do, caches without a bound, duplicated copies of large
buffers or strings, retained references, and object shapes that are bigger than they need to be.

TODO: describe the workload (what it reads, what it keeps) and the allocation hot spots you know of.
