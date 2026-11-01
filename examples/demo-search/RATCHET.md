# Ratchet brief: make demo-search faster

**Goal:** lower `http_req_duration.p95` of `GET /search` under a constant 500 rps
load. A change counts only if a trusted benchmark, which you cannot see or edit,
confirms it. Your own measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: `bench/**`, `ratchet.yaml`,
  `tests/contract/**`, `SOLUTIONS.md`. A candidate that touches them is blocked.
- Keep the HTTP API identical: same routes, parameters, response shapes, error
  codes and **result ordering** (see `openapi.yaml`). The contract tests pin this.
- The dataset (`src/data.ts`, `src/vocab.ts`) and its seed must keep producing
  the same products.
- Don't add dependencies that do the work for you (caching proxies, native
  search engines); improve the code itself. Memory may grow by at most ~20%.
- Don't special-case the benchmark queries. The final check uses a different,
  hidden query mix from the same distribution.
- One idea per commit, with a message saying what you measured. To submit a
  candidate, add the trailer `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
pnpm install
pnpm test                     # unit + contract tests; must pass before and after
pnpm build
pnpm bench:local              # ~65 s: starts the server, replays bench/query-mix.json at 500 rps
node dist/bench/local.js --duration 20 --out /tmp/r.json   # quicker, noisier
node --cpu-prof dist/src/main.js   # profile the server (PORT=3000)
```

`bench:local` prints `{ http_req_duration: { p50, p95, p99 }, rps, error_rate, peak_rss_mb }`
(ms). Expect a few percent run-to-run noise (see README.md); repeat a run before
trusting a small difference. Profile first, then fix what the profile shows.
