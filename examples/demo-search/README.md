# @ratchet/demo-search

A small HTTP search service over a deterministic in-memory catalog of 50,000
generated products, used as the first optimization target for Ratchet. It has
planted performance problems (listed only in the protected `SOLUTIONS.md`), a
pinned API contract, and a benchmark that reports `http_req_duration.p95`.

```sh
pnpm build && PORT=3000 pnpm start
curl 'localhost:3000/search?q=sleek+kettle&filters=price:<100+AND+instock:true'
```

- `GET /search?q&filters&limit&offset`, `GET /health`: see `openapi.yaml`.
- Tests: `pnpm test` (unit in `tests/unit`, contract in `tests/contract`; the
  contract suite validates responses against `openapi.yaml` and pins result
  ordering in `golden.json`, regenerate with `node tests/contract/golden.mjs`).
- Brief for agents: `RATCHET.md`. Repo contract: `ratchet.yaml`.

## Benchmark

| | |
|---|---|
| `bench/k6/search.js` | k6, 500 rps constant arrival, 60 s. `RESULT_FILE=out.json k6 run bench/k6/search.js` |
| `bench/local.ts` | pure-Node equivalent: `pnpm bench:local [-- --rate 500 --duration 60 --warmup 5 --out r.json]`. Starts the built server itself and adds `peak_rss_mb` |
| `bench/query-mix.json` | 400 queries (seed 1) replayed in order by both tools |
| `bench/traces/generate.ts` | `pnpm bench:mix -- --seed 99 --out bench/traces/hidden-mix.json` makes a different mix from the same distribution (point either tool at it with `--mix` / `QUERY_MIX`) |

Output: `{ "http_req_duration": { "p50", "p95", "p99" }, "rps", "error_rate", ... }`, in ms. The local bench also
reports `span`: where the time of a p95 request went. The server sends its own stage timings in a `Server-Timing`
header (`parse`, `search`, `serialize`); the bench averages them over the requests ranked p94 to p96 and adds `wait`
(measured latency minus those stages: event-loop queueing and the network). Ratchet's Result screen compares these
spans between the baseline and the best verified commit. They are self-reported by the service and informational only:
the ratchet judges `http_req_duration.p95`, never the spans.
The k6 script is untested in this environment (see below).

### Baseline (local bench, 4-vCPU container, server and load generator on the same host)

Five consecutive 60 s runs (+5 s warmup) at 500 rps, 0 errors:

| run | p50 ms | p95 ms | p99 ms | peak RSS MB |
|---|---|---|---|---|
| 1 | 1.73 | 20.27 | 26.64 | 189 |
| 2 | 1.30 | 16.06 | 22.31 | 206 |
| 3 | 1.37 | 16.31 | 23.06 | 193 |
| 4 | 1.72 | 20.08 | 27.91 | 191 |
| 5 | 1.87 | 19.78 | 26.48 | 187 |

p95 mean 18.5 ms, range 16.1 to 20.3 ms (about ±12%, coefficient of variation
about 11%). That is far above `min_gain: 0.03`, so on shared hardware a single run cannot
confirm a small win; the pinned-hardware runner with `repeats: 5` is what the
rule relies on. p95 is dominated by queueing behind occasional multi-ms
requests, which is why it is noisier than p50.

Removing one planted problem (the defensive JSON clone) took p95 from 18.5 ms to
about 5.8 ms (6.5, 5.3, 5.7) with tests green.
