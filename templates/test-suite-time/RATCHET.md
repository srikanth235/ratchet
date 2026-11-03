# Ratchet brief: {{verb}} `{{metric}}`

**Goal:** {{direction}} `{{metric}}`, the wall time of the test command in `bench/test-time.config.json`, measured by
`bench/test-time.mjs`. A change counts only if the trusted benchmark, which you cannot edit, confirms it. Your own
measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`, lockfiles and install config. A candidate that touches
  them is blocked.
- **The suite must still pass, and keep testing the same things.** Deleting, skipping or weakening tests makes it
  faster in a way that is worth nothing; the `error_rate` guardrail blocks a failing suite, and reviewers will read
  the diff. Parallelism, shared setup, cheaper fixtures, faster hot paths and removing duplicated work are the point.
- Don't add dependencies: the benchmark machine has no network and installs from the base lockfile.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                    # the command being timed
node bench/test-time.mjs    # prints the benchmark JSON including `test_suite_ms`
```

TODO: say where the time goes (slowest files, setup, I/O) and what must never be skipped.
