# demo-search scripts for `ScriptedAgent`

Real patches against `examples/demo-search` (paths relative to its root; apply with `git apply`, or from the repo root
with `--directory=examples/demo-search`), and two scripts that play them as agent attempts. Regenerate the scripts with
`node scripts/demo-search/build-scripts.mjs`; load them with `loadDemoSearchScript('cc' | 'cx')`.

| Patch | SOLUTIONS.md item | Needs |
|---|---|---|
| `01-search-no-deep-clone` | 1. JSON deep-clone; hits built only for the returned page | none |
| `02-search-topk-collation-rank` | 2. bounded top-k + precomputed `localeCompare` rank (ordering unchanged) | 01 |
| `03-filter-memoize-parse` | 3. memoize `parseFilter` | none |
| `04-search-filter-indexes` | 4. category / brand / tag position lists for filter-only queries | 01, 02 |
| `05-search-prefix-binary-search` | 4 (related). sorted vocabulary + binary search instead of scanning every word | none |
| `06-filter-tokenizer-no-regex` | 3 (related). tokenizer without per-char `RegExp` / array copies | none |

Patches are written with `-U1` context so independent ones apply in any order (checked for several orderings).
All six together keep `pnpm --filter @ratchet/demo-search test` green (65 tests).

Measured effect on the local bench (4-vCPU container, server and load generator on the same host, 500 rps,
`--duration 20 --warmup 5`, 3 runs per step, cumulative in the order 01, 03, 02, 04, 05, 06; shared hardware, so
run-to-run spread is about ±10%):

| After applying | p95 ms (median of 3) | range | p50 ms | peak RSS MB |
|---|---|---|---|---|
| baseline | 21.2 | 19.0 – 22.1 | 2.14 | 174 |
| + 01 deep-clone | 8.1 | 6.7 – 8.4 | 0.91 | 144 |
| + 03 memoize parse | 7.0 | 6.2 – 7.2 | 0.80 | 140 |
| + 02 top-k + rank | 5.6 | 4.9 – 5.8 | 0.73 | 145 |
| + 04 filter indexes | 2.3 | 2.1 – 2.3 | 0.72 | 152 |
| + 05 prefix search | 2.2 | 2.0 – 2.6 | 0.70 | 148 |
| + 06 tokenizer | 2.2 | 2.1 – 2.4 | 0.69 | 148 |

05 and 06 move p95 by under 2% each here, below `min_gain: 0.03`, so a trusted run is expected to **reject** them as
noise-level (the test engine in `test/demo-engine.ts` is simpler and verifies them).

## Scripts

Each script is a list of attempts (one per agent turn). `cc` and `cx` use the same fixes in different orders, interleaved
with deliberate bad attempts, each submitted as a normal candidate:

- protected-path edit (`bench/local.ts` in `cc`, `bench/k6/search.js` in `cx`: lowers the load rate) combined with a good patch: expected verdict **blocked**
- contract break (`TAG_WEIGHT = 0`, changes result ordering): **blocked** by the contract tests (2 golden-order tests fail)
- noise-level no-op (`.filter(Boolean)` -> `.filter((t) => t.length > 0)`): **rejected**

A bad attempt is followed by `git reset --hard refs/ratchet/accepted` in the next attempt, which is how a rejected
candidate is dropped. Steps are idempotent (`skip` markers), and an attempt whose patches already arrived with the
ratchet best is skipped within the turn, so two agents can share a best branch.
