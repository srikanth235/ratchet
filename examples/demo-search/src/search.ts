import type { Catalog, Product } from "./data.js";
import { evaluateFilter, parseFilter } from "./filter.js";

export interface SearchParams {
  q?: string;
  filters?: string;
  limit: number;
  offset: number;
}

export interface SearchHit extends Product {
  score: number;
}

export interface SearchResult {
  total: number;
  limit: number;
  offset: number;
  results: SearchHit[];
}

const NAME_WEIGHT = 3;
const TAG_WEIGHT = 1;

/** Terms are lowercase alphanumeric words; a term matches any word that starts with it. */
export function tokenizeQuery(q: string): string[] {
  return q.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
}

/** Position -> score for every product with a word starting with `term` (name 3, tag 1). */
function termScores(catalog: Catalog, term: string): Map<number, number> {
  const scores = new Map<number, number>();
  for (const [word, postings] of catalog.index) {
    if (!word.startsWith(term)) continue;
    for (const position of postings.tags) if (!scores.has(position)) scores.set(position, TAG_WEIGHT);
    for (const position of postings.name) scores.set(position, NAME_WEIGHT);
  }
  return scores;
}

function toHit(p: Product, score: number): SearchHit {
  return {
    id: p.id, name: p.name, brand: p.brand, category: p.category, price: p.price,
    rating: p.rating, inStock: p.inStock, tags: p.tags, description: p.description, score,
  };
}

/** Order: score desc, rating desc, name A-Z, id asc. */
function byRelevance(a: SearchHit, b: SearchHit): number {
  return b.score - a.score || b.rating - a.rating || a.name.localeCompare(b.name) || a.id - b.id;
}

export function search(catalog: Catalog, params: SearchParams): SearchResult {
  const terms = tokenizeQuery(params.q ?? "");
  const filter = params.filters ? parseFilter(params.filters) : null;

  // candidate positions with their text score
  let scored: Map<number, number> | null = null;
  for (const term of terms) {
    const next = termScores(catalog, term);
    if (!scored) {
      scored = next;
    } else {
      const merged = new Map<number, number>();
      for (const [position, score] of scored) {
        const other = next.get(position);
        if (other !== undefined) merged.set(position, score + other);
      }
      scored = merged;
    }
  }

  let matches: SearchHit[] = [];
  const consider = (position: number, score: number) => {
    const p = catalog.products[position] as Product;
    if (!filter || evaluateFilter(filter, p)) matches.push(toHit(p, score));
  };
  if (scored) for (const [position, score] of scored) consider(position, score);
  else for (let position = 0; position < catalog.products.length; position++) consider(position, 0);

  // defensive copy so callers can never mutate shared state
  matches = JSON.parse(JSON.stringify(matches)) as SearchHit[];
  matches.sort(byRelevance);

  return {
    total: matches.length,
    limit: params.limit,
    offset: params.offset,
    results: matches.slice(params.offset, params.offset + params.limit),
  };
}
