/**
 * Invocation-mix generator. The committed bench/invocations.json comes from seed 1; a different seed gives a
 * "hidden" mix from the same distribution (the same kinds of command, other arguments):
 *
 *   node dist/bench/gen-invocations.js --seed 99 --out hidden-mix.json
 *
 * A mix has eleven invocations: two `list`, `show`, `stats`, two `search`, `export`, two `due`, `config`
 * and a mistyped command (which must fail with exit code 2). Arguments are drawn from the workspace's vocabulary
 * (bench/workspace.ts), so almost no invocation repeats across seeds.
 */
import { writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { createRng, pick } from '../src/rng.js';
import type { Invocation, Mix } from './mix.js';
import { ASSIGNEES, BENCH_SPEC, LABELS, TITLE_WORDS, ZONES } from './workspace.js';

const STATUSES = ['todo', 'doing', 'blocked', 'done'];
const TYPOS = ['lst', 'shw', 'staats', 'serch', 'exprot', 'confg', 'hlep', 'verison', 'ls', 'dew'];

export function generateMix(seed: number, tasks = BENCH_SPEC.tasks): Mix {
  const rng = createRng(seed);
  const int = (lo: number, hi: number) => lo + Math.floor(rng() * (hi - lo + 1));
  const some = <T>(items: readonly T[], p: number): T | undefined => (rng() < p ? pick(rng, items) : undefined);
  const word = () => pick(rng, TITLE_WORDS);

  const list = (): Invocation => {
    const args = ['list'];
    const status = some(STATUSES, 0.6);
    const assignee = some(ASSIGNEES, 0.5);
    const label = some(LABELS, 0.4);
    if (status) args.push('--status', status);
    if (assignee) args.push('--assignee', assignee);
    if (label) args.push('--label', label);
    args.push('--sort', pick(rng, ['id', 'priority', 'due', 'estimate']), '--limit', String(int(5, 40)));
    return { args };
  };
  const search = (): Invocation => ({ args: ['search', ...(rng() < 0.5 ? [word()] : [word(), word()]), '--limit', String(int(3, 20))] });
  const exportJob = (): Invocation => {
    const format = pick(rng, ['csv', 'json', 'md']);
    const args = ['export', '--format', format, '--limit', String(int(40, 400))];
    const status = some(STATUSES, 0.6);
    if (status) args.push('--status', status);
    return { args };
  };
  const due = (): Invocation => {
    const args = ['due', '--within', String(int(3, 60))];
    const zone = some(ZONES, 0.7);
    if (zone) args.push('--tz', zone);
    return { args };
  };

  return {
    seed,
    invocations: [
      list(),
      { args: ['show', String(int(1, tasks))] },
      stats(rng),
      search(),
      exportJob(),
      due(),
      list(),
      search(),
      due(),
      { args: ['config', ...(rng() < 0.5 ? ['--format', 'json'] : [])] },
      { args: [pick(rng, TYPOS), ...(rng() < 0.5 ? ['--limit', '5'] : [])], expect: 2 },
    ],
  };
}

function stats(rng: () => number): Invocation {
  return { args: ['stats', '--by', pick(rng, ['status', 'assignee', 'label', 'priority']), '--top', String(5 + Math.floor(rng() * 10))] };
}

const isMain = process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  const { values } = parseArgs({ options: { seed: { type: 'string', default: '1' }, out: { type: 'string' }, tasks: { type: 'string', default: String(BENCH_SPEC.tasks) } } });
  const mix = generateMix(Number(values.seed), Number(values.tasks));
  const json = JSON.stringify(mix, null, 1) + '\n';
  if (values.out) writeFileSync(values.out, json);
  else process.stdout.write(json);
}
