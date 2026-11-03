# Ratchet brief: {{verb}} `{{metric}}`

**Goal:** {{direction}} `{{metric}}`, the latency this service shows under the load test in `bench/http-latency.mjs`.
A change counts only if the trusted benchmark, which you cannot edit, confirms it, then again on a hidden request mix
you do not see. Your own measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`, lockfiles and install config. A candidate that touches
  them is blocked.
- Keep behaviour: the guardrails (the tests, the error rate) must stay green.
- Don't special-case the paths in `bench/http-latency.config.json`: the final check replays a different mix of the same kind.
- Don't add dependencies: the benchmark machine has no network and installs from the base lockfile.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                            # must pass before and after
node bench/http-latency.mjs         # starts the service, prints the benchmark JSON including `http_req_duration.p95`
BENCH_RATE=200 BENCH_DURATION_S=10 node bench/http-latency.mjs   # a quicker, heavier local run
```

TODO: describe the service (where the time goes, how to profile it, what is already cached) and the traps you know about.
