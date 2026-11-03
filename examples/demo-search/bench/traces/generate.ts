/**
 * Query-mix generator. The committed bench/query-mix.json comes from seed 1;
 * a different seed gives a "hidden" mix from the same distribution family:
 *
 *   node dist/bench/traces/generate.js --seed 99 --out bench/traces/hidden-mix.json
 */
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { createRng, pick } from "../../src/rng.js";
import { ADJECTIVES, BRANDS, CATEGORIES, NOUNS_BY_CATEGORY, TAGS } from "../../src/vocab.js";

export interface MixQuery {
  q?: string;
  filters?: string;
  limit?: number;
  offset?: number;
}

const NOUNS = [...new Set(Object.values(NOUNS_BY_CATEGORY).flat())];
const PRICE_CAPS = [25, 50, 75, 100, 150, 200, 300];
const PRICE_RANGES = [[10, 50], [25, 100], [50, 150], [100, 300], [20, 80]] as const;
const RATINGS = [3, 3.5, 4, 4.5];

export function generateMix(seed: number, count: number): MixQuery[] {
  const rng = createRng(seed);
  // Zipf-like popularity: low ranks are picked far more often than high ones.
  const zipf = <T>(items: readonly T[]): T => items[Math.min(items.length - 1, Math.floor(items.length * rng() ** 1.2))] as T;
  const term = (): string => {
    const r = rng();
    const word = r < 0.5 ? zipf(NOUNS) : r < 0.8 ? zipf(BRANDS) : r < 0.95 ? zipf(ADJECTIVES) : zipf(TAGS);
    return rng() < 0.3 && word.length > 4 ? word.slice(0, 3 + Math.floor(rng() * (word.length - 4))) : word;
  };
  const text = (): string => (rng() < 0.3 ? term() : `${term()} ${term()}`);
  // narrow: filter-only queries always pin a brand so result sets stay reasonably small
  const filter = (narrow = false): string => {
    const c = zipf(CATEGORIES);
    const b = zipf(BRANDS);
    const b2 = pick(rng, BRANDS);
    const cap = pick(rng, PRICE_CAPS);
    const [lo, hi] = pick(rng, PRICE_RANGES);
    const rating = pick(rng, RATINGS);
    switch (narrow ? 1 + 2 * Math.floor(rng() * 2) : Math.floor(rng() * 5)) {
      case 0: return `category:${c} AND price:<${cap}`;
      case 1: return `brand:${b} AND rating:>=${rating} AND price:<${cap}`;
      case 2: return `(category:${c} OR category:${zipf(CATEGORIES)}) AND price:${lo}-${hi} AND inStock:true`;
      case 3: return `tag:${zipf(TAGS)} AND brand:${b2} AND price:<=${cap}`;
      default: return `category:${c} AND (brand:${b} OR brand:${b2}) AND rating:>=${rating} AND price:${lo}-${hi}`;
    }
  };

  const queries: MixQuery[] = [];
  for (let i = 0; i < count; i++) {
    const r = rng();
    const query: MixQuery = r < 0.4 ? { q: text() } : r < 0.96 ? { q: text(), filters: filter() } : { filters: filter(true) };
    const p = rng();
    if (p < 0.1) query.limit = 50;
    else if (p < 0.2) query.offset = 20;
    queries.push(query);
  }
  return queries;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const { values } = parseArgs({
    options: {
      seed: { type: "string", default: "99" },
      count: { type: "string", default: "400" },
      out: { type: "string", default: "bench/traces/hidden-mix.json" },
    },
  });
  const seed = Number(values.seed);
  const mix = { seed, queries: generateMix(seed, Number(values.count)) };
  writeFileSync(values.out as string, JSON.stringify(mix, null, 1) + "\n");
  console.log(`wrote ${mix.queries.length} queries (seed ${seed}) to ${values.out}`);
}
