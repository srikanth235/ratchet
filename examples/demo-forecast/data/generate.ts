/**
 * Deterministic synthetic weekly sales: SKUs x weeks with trend, yearly seasonality (shared per category, jittered per
 * SKU), planned promotions, holidays (one of them moves between years), stockouts (censored sales) and lognormal noise.
 *
 * Protected: agents may read this but cannot change it. Every random draw is a pure function of
 * (seed, sku, week, stream), so the first N weeks of a dataset do not depend on how many weeks are generated, and the
 * sealed holdout weeks are drawn from a seed the agents never see.
 */
import type { SalesTable } from '../src/types.js';

export const CATALOG_SEED = 7301;
export const SKU_COUNT = 200;
export const CATEGORIES = 8;
export const WEEKS_PER_YEAR = 52;
/** Weeks the agents can see (about 2.8 years). The most recent HOLDOUT_WEEKS after them are sealed. */
export const TRAIN_WEEKS = 144;
export const HOLDOUT_WEEKS = 12;
/** Seed of the weekly randomness for the training weeks. */
export const WEEK_SEED = 424_243;

/** murmur3-style mixer: a pure function of its integer inputs. */
function mix(...parts: number[]): number {
  let h = 0x9e3779b9;
  for (const p of parts) {
    h = Math.imul(h ^ (p >>> 0), 0xcc9e2d51);
    h = (h << 15) | (h >>> 17);
    h = Math.imul(h, 0x1b873593);
    h ^= h >>> 13;
    h = Math.imul(h, 0x85ebca6b);
    h ^= h >>> 16;
  }
  return h >>> 0;
}
const u01 = (...parts: number[]) => (mix(...parts) + 0.5) / 4294967296;
function gauss(...parts: number[]): number {
  return Math.sqrt(-2 * Math.log(u01(...parts, 1))) * Math.cos(2 * Math.PI * u01(...parts, 2));
}

function createRng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface CategoryParams {
  amp1: number;
  phase1: number;
  amp2: number;
  phase2: number;
  promoLift: number;
  holidayLift: number;
}

interface SkuParams {
  category: number;
  base: number;
  /** Fractional growth per year. */
  growth: number;
  amp1: number;
  phase1: number;
  amp2: number;
  phase2: number;
  promoLift: number;
  promoRate: number;
  holidayLift: number;
  sigma: number;
  stockoutRate: number;
}

function catalog(): SkuParams[] {
  const rng = createRng(CATALOG_SEED);
  const norm = () => Math.sqrt(-2 * Math.log(1 - rng())) * Math.cos(2 * Math.PI * rng());
  const cats: CategoryParams[] = Array.from({ length: CATEGORIES }, () => ({
    amp1: 0.25 + 0.4 * rng(),
    phase1: 2 * Math.PI * rng(),
    amp2: 0.04 + 0.12 * rng(),
    phase2: 2 * Math.PI * rng(),
    promoLift: 0.45 + 0.8 * rng(),
    holidayLift: -0.25 + 1.15 * rng(),
  }));
  return Array.from({ length: SKU_COUNT }, (_, i) => {
    const category = i % CATEGORIES;
    const c = cats[category]!;
    return {
      category,
      base: Math.min(400, Math.max(5, Math.exp(3.4 + 0.9 * norm()))),
      growth: 0.05 + 0.16 * norm(),
      amp1: Math.max(0.02, c.amp1 * (1 + 0.3 * norm())),
      phase1: c.phase1 + 0.2 * norm(),
      amp2: Math.max(0, c.amp2 * (1 + 0.3 * norm())),
      phase2: c.phase2 + 0.3 * norm(),
      promoLift: Math.max(0.1, c.promoLift * (1 + 0.2 * norm())),
      promoRate: 0.05 + 0.09 * rng(),
      holidayLift: c.holidayLift * (1 + 0.2 * norm()),
      sigma: 0.1 + 0.18 * rng(),
      stockoutRate: 0.015 + 0.025 * rng(),
    };
  });
}

/** Week of the year of Easter in year 0, 1, 2 (it moves, so "same week last year" mis-places it). */
const EASTER_WEEK = [15, 12, 16];

/** Holiday intensity (0 to 1) of an absolute week index. */
export function holidayIntensity(week: number): number {
  const woy = week % WEEKS_PER_YEAR;
  const year = Math.floor(week / WEEKS_PER_YEAR);
  if (woy === (EASTER_WEEK[year % 3] ?? 15)) return 0.8;
  switch (woy) {
    case 27: return 0.4;
    case 47: return 1;
    case 48: return 0.5;
    case 50: return 0.6;
    case 51: return 1;
    default: return 0;
  }
}

export interface GenerateOptions {
  /** Number of weeks to generate; the first TRAIN_WEEKS use WEEK_SEED. */
  weeks: number;
  /** Seed of the weekly randomness for weeks >= TRAIN_WEEKS (sealed in the evaluator; agents never see it). */
  futureSeed?: number;
}

/** Every SKU's sales for weeks `0..weeks-1`, plus the (planned, hence known in advance) promo and holiday calendars. */
export function generateSales(opts: GenerateOptions): SalesTable {
  const params = catalog();
  const weeks = opts.weeks;
  if (weeks > TRAIN_WEEKS && opts.futureSeed === undefined) throw new Error('weeks beyond TRAIN_WEEKS need a futureSeed (the sealed holdout)');
  const seedOf = (w: number) => (w < TRAIN_WEEKS ? WEEK_SEED : opts.futureSeed!);
  const holiday = Array.from({ length: weeks }, (_, w) => holidayIntensity(w));
  const units: number[][] = [];
  const promo: number[][] = [];
  params.forEach((p, s) => {
    const pr = new Array<number>(weeks).fill(0);
    for (let w = 0; w < weeks; w++) {
      // a promotion starts with probability promoRate/1.5 and lasts one or two weeks
      if (u01(seedOf(w), s, w, 11) < p.promoRate / 1.5) {
        pr[w] = 1;
        if (w + 1 < weeks && u01(seedOf(w), s, w, 12) < 0.5) pr[w + 1] = 1;
      }
    }
    const row = new Array<number>(weeks).fill(0);
    let stockoutLeft = 0;
    for (let w = 0; w < weeks; w++) {
      const seed = seedOf(w);
      const trend = Math.max(0.25, 1 + (p.growth * w) / WEEKS_PER_YEAR);
      const t = (2 * Math.PI * (w % WEEKS_PER_YEAR)) / WEEKS_PER_YEAR;
      const season = Math.max(0.1, 1 + p.amp1 * Math.sin(t + p.phase1) + p.amp2 * Math.sin(2 * t + p.phase2));
      const demand =
        p.base * trend * season * (1 + p.promoLift * pr[w]!) * (1 + p.holidayLift * holiday[w]!) *
        Math.exp(p.sigma * gauss(seed, s, w, 21) - (p.sigma * p.sigma) / 2);
      if (stockoutLeft === 0 && u01(seed, s, w, 31) < p.stockoutRate) stockoutLeft = 1 + Math.floor(u01(seed, s, w, 32) * 3);
      let sold = demand;
      if (stockoutLeft > 0) {
        sold = demand * 0.15 * u01(seed, s, w, 33); // censored: shelves are empty
        stockoutLeft--;
      }
      row[w] = Math.round(sold);
    }
    units.push(row);
    promo.push(pr);
  });
  return {
    skus: params.map((p, id) => ({ id, category: p.category })),
    units,
    promo,
    holiday,
  };
}

/** The agent-visible history: TRAIN_WEEKS of sales and calendar. */
export function trainSales(): SalesTable {
  return generateSales({ weeks: TRAIN_WEEKS });
}
