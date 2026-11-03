# Ratchet brief: {{verb}} `{{metric}}`

**Goal:** {{direction}} `{{metric}}`, reported by `bench/build-size.mjs` after it runs the build command in
`bench/build-size.config.json`: the size of what the build ships (`bundle_kb`, also `bundle_gzip_kb`) and how long it
takes (`build_seconds`). A change counts only if the trusted benchmark, which you cannot edit, confirms it. Your own
measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`, lockfiles and install config. A candidate that touches
  them is blocked.
- The product must keep working: the guardrails (the tests, the build succeeding) must stay green. Shrinking the bundle
  by dropping features, or by moving code out of `output_dir` to be fetched at run time, is not an improvement.
- Don't add dependencies: the benchmark machine has no network and installs from the base lockfile.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                    # must pass before and after
node bench/build-size.mjs   # builds, then prints the benchmark JSON including `bundle_kb`
```

Look for: dead code and unused exports, duplicated dependencies, heavy imports that could be lazy or lighter, source
maps or fixtures that leak into the output, and slow build steps that can be cached or skipped.

TODO: say which output counts (`output_dir` in the config) and what must stay in the bundle.
