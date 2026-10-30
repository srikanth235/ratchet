/**
 * Pure-Node cold-start benchmark, the fallback for machines without hyperfine. It prints the same JSON as
 * bench/hyperfine.ts:
 *
 *   node dist/bench/spawn.js [--mix bench/invocations.json] [--runs 4] [--warmup 1] [--keep 1] [--out result.json]
 *
 * Every invocation of the mix is started `runs` times as a fresh `node dist/src/main.js ...` process in the
 * generated workspace (bench/workspace.ts), round-robin so slow drift of the machine hits all of them alike.
 * HOME, TMPDIR and the cache dir are emptied before each start, so no run can profit from what an earlier one left.
 * `--keep 0.67` keeps only the fastest two thirds of each invocation's runs before the statistics: on a shared,
 * loaded machine slowdowns come from other processes, not from the CLI, and the fast runs are the clean ones.
 * Leave it at 1 on pinned hardware.
 * Output: { cold_start_ms: { p50, p95, mean, min, max }, invocations, runs, error_rate }.
 */
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { CLI_PATH, VISIBLE_MIX, loadMix, summarize } from './mix.js';
import { buildWorkspace, cliEnv, resetScratch } from './workspace.js';

const { values } = parseArgs({
  options: {
    mix: { type: 'string', default: process.env.RATCHET_TRACE ?? VISIBLE_MIX },
    runs: { type: 'string', default: '4' },
    warmup: { type: 'string', default: '1' },
    keep: { type: 'string', default: '1' },
    out: { type: 'string' },
  },
});

const mix = loadMix(values.mix as string);
const runs = Number(values.runs);
const warmup = Number(values.warmup);
const keep = Number(values.keep);
if (!(keep > 0 && keep <= 1)) throw new Error('--keep must be in (0, 1]');
const ws = buildWorkspace();
const env = cliEnv(ws);

/** Start the CLI once; returns wall milliseconds and whether it ended as the mix says it should. */
function startOnce(args: string[], expect: number): { ms: number; ok: boolean } {
  resetScratch(ws);
  const t0 = performance.now();
  const r = spawnSync(process.execPath, [CLI_PATH, ...args], { cwd: ws.cwd, env, encoding: 'utf8', maxBuffer: 64 << 20 });
  const ms = performance.now() - t0;
  const printed = expect === 0 ? r.stdout.length > 0 : r.stderr.length > 0;
  return { ms, ok: r.status === expect && printed };
}

try {
  for (let w = 0; w < warmup; w++) for (const inv of mix.invocations) startOnce(inv.args, inv.expect ?? 0);
  const perInvocation: number[][] = mix.invocations.map(() => []);
  let failures = 0;
  for (let r = 0; r < runs; r++) {
    mix.invocations.forEach((inv, i) => {
      const { ms, ok } = startOnce(inv.args, inv.expect ?? 0);
      perInvocation[i]!.push(ms);
      if (!ok) failures++;
    });
  }
  const times = perInvocation.flatMap((t) => t.sort((a, b) => a - b).slice(0, Math.max(1, Math.ceil(t.length * keep))));
  const json = JSON.stringify(summarize(times, failures, mix.invocations.length), null, 2);
  if (values.out) writeFileSync(values.out, json + '\n');
  console.log(json);
} finally {
  ws.cleanup();
}
