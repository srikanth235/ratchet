/**
 * Cold-start benchmark with hyperfine (https://github.com/sharkdp/hyperfine): the real evaluator.
 *
 *   node dist/bench/hyperfine.js [--mix bench/invocations.json] [--runs 4] [--warmup 1] [--out result.json]
 *
 * One hyperfine command per invocation of the mix, run in the generated workspace (bench/workspace.ts) with
 * `--export-json`; the per-run times of all commands are pooled and reduced to the JSON bench/spawn.ts prints:
 * { cold_start_ms: { p50, p95, mean, min, max }, invocations, runs, error_rate }. `--prepare` empties the cache
 * directory before every timed run, `--ignore-failure` keeps going so a crashing CLI shows up as `error_rate`.
 */
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { CLI_PATH, VISIBLE_MIX, loadMix, summarize, type Mix } from './mix.js';
import { buildWorkspace, cliEnv } from './workspace.js';

interface HyperfineExport {
  results: { times: number[]; exit_codes: (number | null)[] }[];
}

/** Pool the runs of every command (seconds in the export, milliseconds out) and count unexpected exit codes. */
export function reduceExport(exported: HyperfineExport, mix: Mix) {
  const times: number[] = [];
  let failures = 0;
  exported.results.forEach((r, i) => {
    const expect = mix.invocations[i]?.expect ?? 0;
    for (const t of r.times) times.push(t * 1000);
    for (const code of r.exit_codes) if (code !== expect) failures++;
  });
  return summarize(times, failures, mix.invocations.length);
}

/** A word for hyperfine's command-line parser: single-quoted unless it is plainly safe. */
export function quote(word: string): string {
  return /^[\w@%+=:,./-]+$/.test(word) ? word : `'${word.replace(/'/g, `'\\''`)}'`;
}

const isMain = process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  const { values } = parseArgs({
    options: {
      mix: { type: 'string', default: process.env.RATCHET_TRACE ?? VISIBLE_MIX },
      runs: { type: 'string', default: '4' },
      warmup: { type: 'string', default: '1' },
      out: { type: 'string' },
    },
  });
  const probe = spawnSync('hyperfine', ['--version'], { encoding: 'utf8' });
  if (probe.error || probe.status !== 0) {
    console.error('hyperfine is not installed or not on PATH (https://github.com/sharkdp/hyperfine); use `node dist/bench/spawn.js`, which prints the same JSON.');
    process.exit(2);
  }
  const mix = loadMix(values.mix as string);
  const ws = buildWorkspace();
  try {
    const exportFile = join(ws.base, 'hyperfine.json');
    const commands = mix.invocations.map((inv) => [process.execPath, CLI_PATH, ...inv.args].map(quote).join(' '));
    const r = spawnSync(
      'hyperfine',
      ['-N', '--ignore-failure', '--style', 'none', '--warmup', String(values.warmup), '--runs', String(values.runs), '--prepare', `rm -rf ${quote(ws.scratch)}`, '--export-json', exportFile, ...commands],
      { cwd: ws.cwd, env: { ...cliEnv(ws), PATH: process.env.PATH ?? '' }, stdio: ['ignore', 'ignore', 'inherit'] },
    );
    if (r.status !== 0) throw new Error(`hyperfine exited with ${r.status}`);
    const json = JSON.stringify(reduceExport(JSON.parse(readFileSync(exportFile, 'utf8')) as HyperfineExport, mix), null, 2);
    if (values.out) writeFileSync(values.out, json + '\n');
    console.log(json);
  } finally {
    ws.cleanup();
  }
}
