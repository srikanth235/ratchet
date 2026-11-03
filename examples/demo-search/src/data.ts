import { createRng, pick } from "./rng.js";
import {
  ADJECTIVES, BRANDS, CATEGORIES, CATEGORY_WEIGHTS, DESCRIPTION_WORDS, NOUNS_BY_CATEGORY, TAGS,
} from "./vocab.js";

export interface Product {
  id: number;
  name: string;
  brand: string;
  category: string;
  price: number;
  rating: number;
  inStock: boolean;
  tags: string[];
  description: string;
}

/** Postings: positions in Catalog.products of products having the word in their name / tags. */
export interface Postings {
  name: number[];
  tags: number[];
}

export interface Catalog {
  products: Product[];
  /** word -> postings, built once at load; lets text queries avoid scanning every product. */
  index: Map<string, Postings>;
}

export const DEFAULT_SEED = 42;
export const DEFAULT_COUNT = 50_000;

const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

export function buildIndex(products: Product[]): Map<string, Postings> {
  const index = new Map<string, Postings>();
  const add = (word: string, field: keyof Postings, position: number) => {
    let postings = index.get(word);
    if (!postings) index.set(word, (postings = { name: [], tags: [] }));
    postings[field].push(position);
  };
  products.forEach((p, position) => {
    for (const word of new Set(p.name.toLowerCase().split(" "))) add(word, "name", position);
    for (const tag of p.tags) add(tag, "tags", position);
  });
  return index;
}

export function generateCatalog(count = DEFAULT_COUNT, seed = DEFAULT_SEED): Catalog {
  const rng = createRng(seed);
  const totalWeight = CATEGORY_WEIGHTS.reduce((a, b) => a + b, 0);
  const products: Product[] = [];
  for (let id = 1; id <= count; id++) {
    let r = rng() * totalWeight;
    let ci = 0;
    while (ci < CATEGORIES.length - 1 && r >= (CATEGORY_WEIGHTS[ci] as number)) {
      r -= CATEGORY_WEIGHTS[ci] as number;
      ci++;
    }
    const category = CATEGORIES[ci] as (typeof CATEGORIES)[number];
    const brand = pick(rng, BRANDS);
    const adjective = pick(rng, ADJECTIVES);
    const noun = pick(rng, NOUNS_BY_CATEGORY[category]);
    const name = `${capitalize(brand)} ${capitalize(adjective)} ${capitalize(noun)}`;

    const tagCount = 2 + Math.floor(rng() * 3);
    const tags: string[] = [];
    while (tags.length < tagCount) {
      const t = pick(rng, TAGS);
      if (!tags.includes(t)) tags.push(t);
    }
    const words: string[] = [];
    const wordCount = 6 + Math.floor(rng() * 4);
    for (let i = 0; i < wordCount; i++) words.push(pick(rng, DESCRIPTION_WORDS));
    const description = `${capitalize(words.join(" "))}.`;

    const price = Math.round((5 + Math.pow(rng(), 3) * 995) * 100) / 100;
    const rating = Math.round((2 + 3 * Math.pow(rng(), 0.6)) * 10) / 10;
    const inStock = rng() < 0.85;

    products.push({
      id, name, brand, category, price, rating, inStock, tags, description,
    });
  }
  return { products, index: buildIndex(products) };
}

let shared: Catalog | undefined;
/** The process-wide catalog; size can be overridden with PRODUCT_COUNT for quick experiments. */
export function getCatalog(): Catalog {
  shared ??= generateCatalog(Number(process.env.PRODUCT_COUNT) || DEFAULT_COUNT);
  return shared;
}
