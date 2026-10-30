/**
 * Leakage (look-ahead) guardrail (protected). A forecast made at `origin` may only use sales from weeks before `origin`
 * (plus the planned promo / holiday calendar up to the end of the horizon).
 *
 *  1. `findLeaks`: scramble everything the pipeline is not allowed to see (sales on and after the origin, calendar
 *     beyond the horizon) in several ways; the forecast must be bit-identical.
 *  2. `scanSources`: the pipeline source may not reach the dataset by other means (generator, files, network, clock).
 *
 * The evaluator runs both and reports the number of violations as `leakage_violations`; ratchet.yaml requires 0, so a
 * leaking candidate is blocked with its backtest result on record. tests/contract/leakage.test.ts runs the same code.
 */
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { PredictFn, SalesTable } from '../src/types.js';

export const SCRAMBLERS: Record<string, (v: number, w: number, s: number) => number> = {
  'reversed in time': (_v, w) => (w * 7919) % 101,
  'inflated tenfold': (v) => v * 10 + 999,
  zeroed: () => 0,
  'not a number': () => NaN,
  'shuffled per SKU': (_v, w, s) => ((s + 3) * (w + 11)) % 257,
};

/** Returns a description of every way `fn` reacts to data it must not see. */
export function findLeaks(fn: PredictFn, table: SalesTable, origins: readonly number[], horizon: number): string[] {
  const leaks: string[] = [];
  for (const origin of origins) {
    const base = JSON.stringify(fn(table, origin, horizon));
    for (const [name, scramble] of Object.entries(SCRAMBLERS)) {
      const hidden: SalesTable = {
        ...table,
        units: table.units.map((row, s) => row.map((v, w) => (w >= origin ? scramble(v, w, s) : v))),
        promo: table.promo.map((row) => row.map((v, w) => (w >= origin + horizon ? 1 : v))),
        holiday: table.holiday.map((v, w) => (w >= origin + horizon ? 1 : v)),
      };
      if (JSON.stringify(fn(hidden, origin, horizon)) !== base) leaks.push(`origin ${origin}: the forecast changed when sales from the origin on were ${name}`);
    }
  }
  return leaks;
}

const BANNED: [RegExp, string][] = [
  [/from\s+['"](?:node:)?(?:fs|child_process|net|http|https|dns|worker_threads|vm|module|os)(?:\/[\w-]+)?['"]/, 'imports a node module that can reach data or the network'],
  [/from\s+['"](?:\.\.?\/)+(?:data|eval|tests)\//, 'imports the data generator, the evaluator or the tests'],
  [/\bimport\s*\(/, 'uses a dynamic import'],
  [/\brequire\s*\(/, 'uses require'],
  [/\bprocess\./, 'reads the process (env, argv, cwd)'],
  [/\bfetch\s*\(/, 'uses the network'],
  [/\bglobalThis\b/, 'reaches for globalThis'],
  [/\bDate\.now\b|\bMath\.random\b/, 'depends on the clock or on randomness'],
];

/** The project root: the closest ancestor of this file that has `src/predict.ts`. */
export function projectRoot(): string {
  let dir = dirname(fileURLToPath(import.meta.url));
  while (!existsSync(join(dir, 'src', 'predict.ts'))) {
    const up = dirname(dir);
    if (up === dir) throw new Error('cannot find src/predict.ts above ' + dirname(fileURLToPath(import.meta.url)));
    dir = up;
  }
  return dir;
}

/** Side channels to the data in `src/`. `types.ts` (the contract) is exempt. */
export function scanSources(root = projectRoot()): string[] {
  const src = join(root, 'src');
  const out: string[] = [];
  for (const f of readdirSync(src, { recursive: true }).map(String).filter((p) => p.endsWith('.ts') && p !== 'types.ts')) {
    const text = readFileSync(join(src, f), 'utf8');
    for (const [re, why] of BANNED) if (re.test(text)) out.push(`src/${f}: ${why}`);
  }
  return out;
}
