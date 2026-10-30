# Planted inefficiencies (protected: hidden from agents)

All four are fixable without changing the API, the dataset, or result ordering.
Locations are in `src/`. Order is roughly by impact on the benchmark mix.

## 1. Defensive JSON deep-clone of every match (`search.ts`)

`search()` builds a full `SearchHit` (including `description` and `tags`) for
**every** match and then does `JSON.parse(JSON.stringify(matches))` "so callers
can't mutate shared state". Cost is linear in the number of matches (hundreds to
thousands per query) even though only `limit` (default 20) rows are returned, and
it generates lots of garbage. The catalog is never mutated downstream.
**Fix:** drop the copy; build hits lazily only for the returned page (select the
page first, then call `toHit`).

## 2. Full sort of all matches with a `localeCompare` tie-break (`search.ts`)

`byRelevance` sorts the whole match set although only `offset + limit` rows are
needed. Ties on score and rating are very common (ratings have one decimal), so
the comparator falls through to `a.name.localeCompare(b.name)` often, which is
slow. **Fix:** top-k selection (bounded heap / partial selection of
`offset + limit` items) and/or a precomputed collation key (or compare names with
a shared `Intl.Collator`; ordering must stay identical, so keep the
`localeCompare` semantics, e.g. precompute a sort rank for each product once at
load).

## 3. Filter expression re-parsed on every request (`filter.ts`)

`parseFilter` is a hand-written tokenizer/parser that builds a `RegExp` per
character, copies the token array on each push (`[...tokens, t]`), re-slices the
token list on every `peek`, and then, as a "self-check", prints the AST and
parses it again. It runs on every request that has `filters`, although the
benchmark (and any real client) sends a small set of distinct expressions.
**Fix:** memoize `parseFilter` by the filter string (small LRU / Map; the AST is
immutable), and/or rewrite the tokenizer without per-char `RegExp`s and drop the
redundant round-trip. A cache is the cheap win; the rewrite protects against a
hidden mix with a long tail of distinct filters.

## 4. No index for filters: filter-only queries scan and evaluate all 50k products (`search.ts`)

Text queries use the inverted index (`catalog.index`), but when `q` is empty the
code walks all 50,000 products and interprets the filter AST on each. The AST
evaluation is tree-walking with string-keyed field access. **Fix:** add secondary
indexes (position lists by `category`, `brand`, `tag`) at load time and start from
the most selective `AND` clause, or compile the filter AST to a closure /
use column arrays. Related smaller wins: `termScores` scans the whole vocabulary
with `startsWith` per query term (a sorted word list + binary search or a prefix
map avoids it), and builds throwaway `Map`s for scoring.

## Verifying a fix

`pnpm test` must stay green (golden orderings in `tests/contract/golden.json`).
Typical effect on the local bench is in `README.md`.
