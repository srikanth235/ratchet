/**
 * Materializes the sealed holdout (protected): the 12 weeks after the training period, drawn from a seed the agents
 * never see, written OUTSIDE the repository (the engine passes the path as RATCHET_TRACE in the confirm phase).
 *
 *   node dist/eval/holdout.js --seed <hidden> --out <path>
 */
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { HOLDOUT_WEEKS, TRAIN_WEEKS, generateSales } from '../data/generate.js';
import type { HoldoutFile } from './backtest.js';

export function materializeHoldout(seed: number): HoldoutFile {
  const all = generateSales({ weeks: TRAIN_WEEKS + HOLDOUT_WEEKS, futureSeed: seed });
  return {
    version: 1,
    firstWeek: TRAIN_WEEKS,
    units: all.units.map((r) => r.slice(TRAIN_WEEKS)),
    promo: all.promo.map((r) => r.slice(TRAIN_WEEKS)),
    holiday: all.holiday.slice(TRAIN_WEEKS),
  };
}

const flag = (name: string) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const isMain = process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href;
if (isMain) {
  const out = flag('out');
  const seed = Number(flag('seed') ?? process.env.RATCHET_HOLDOUT_SEED);
  if (!out || !Number.isFinite(seed)) {
    console.error('usage: node dist/eval/holdout.js --seed <number> --out <path>');
    process.exit(2);
  }
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, JSON.stringify(materializeHoldout(seed)));
  console.log(`wrote holdout (seed ${seed}) to ${out}`);
}
