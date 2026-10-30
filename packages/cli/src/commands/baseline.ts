import { spawn, type ChildProcess } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { extractMetric, formatConfigError, loadConfig, noiseFloor, summarize } from '@ratchet/core';
import type { Io } from '../io.js';
import { DEFAULT_CONFIG_PATH } from './validate.js';

export interface BaselineOptions {
  /** Number of bench runs; defaults to `bench.repeats` from the config. */
  repeats?: number | undefined;
  /** Machine-readable output on stdout (a single JSON document). */
  json?: boolean | undefined;
  /** Noise floor multiplier on stddev. Default 2. */
  k?: number | undefined;
  /** Absolute lower bound for the noise floor. Default 0. */
  minAbsolute?: number | undefined;
  /** Kill a single bench run after this many ms. Default: no timeout. */
  timeoutMs?: number | undefined;
}

export interface CommandResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

/** Run a shell command in `cwd`, capturing stdout/stderr. */
export function runShell(command: string, cwd: string, timeoutMs?: number): Promise<CommandResult> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, {
      cwd,
      shell: true,
      stdio: ['ignore', 'pipe', 'pipe'],
      // own process group, so a timeout can kill the shell AND the bench it spawned
      detached: process.platform !== 'win32',
    });
    let stdout = '';
    let stderr = '';
    let timedOut = false;
    const timer =
      timeoutMs === undefined
        ? undefined
        : setTimeout(() => {
            timedOut = true;
            killTree(child);
          }, timeoutMs);
    child.stdout.setEncoding('utf8').on('data', (d: string) => (stdout += d));
    child.stderr.setEncoding('utf8').on('data', (d: string) => (stderr += d));
    child.on('error', (err) => {
      if (timer) clearTimeout(timer);
      reject(err);
    });
    child.on('close', (code) => {
      if (timer) clearTimeout(timer);
      resolvePromise({ exitCode: code, stdout, stderr, timedOut });
    });
  });
}

function killTree(child: ChildProcess): void {
  try {
    if (process.platform !== 'win32' && child.pid !== undefined) process.kill(-child.pid, 'SIGKILL');
    else child.kill('SIGKILL');
  } catch {
    child.kill('SIGKILL');
  }
}

const tail = (s: string, n = 10) => s.trimEnd().split('\n').slice(-n).join('\n');
const fmt = (n: number) => String(Number(n.toPrecision(6)));

/**
 * `ratchet baseline [path] --repeats N`
 *
 * Runs `bench.run` N times sequentially, in the directory containing the config file (the repo
 * root). Output contract: the command prints JSON on stdout; the LAST top-level JSON object on
 * stdout is used and any other output is ignored. `objective.metric` is looked up in it by dotted
 * path, e.g. `http_req_duration.p95` -> `{"http_req_duration": {"p95": 12.3}}`.
 *
 * Prints per-run values, summary stats and the noise floor (max(k * stddev, minAbsolute), k = 2)
 * or, with `json`, one JSON document. Exit code 0 on success, 1 on any failure (invalid config, a
 * run exiting non-zero, missing metric, ...).
 */
export async function runBaseline(
  path: string | undefined,
  opts: BaselineOptions,
  io: Io,
): Promise<number> {
  const file = path ?? DEFAULT_CONFIG_PATH;
  const res = loadConfig(file);
  if (!res.ok) {
    io.err(`${file}: invalid (${res.errors.length} error${res.errors.length === 1 ? '' : 's'})\n`);
    for (const e of res.errors) io.err(`  - ${formatConfigError(e)}\n`);
    return 1;
  }
  const config = res.config;
  const repeats = opts.repeats ?? config.bench.repeats;
  if (!Number.isInteger(repeats) || repeats < 1) {
    io.err(`--repeats must be a positive integer (got ${repeats})\n`);
    return 1;
  }
  const cwd = dirname(resolve(file));
  const metric = config.objective.metric;
  const json = opts.json === true;
  const progress = (s: string) => {
    if (!json) io.out(s);
  };

  progress(`Baseline: running \`${config.bench.run}\` ${repeats}x in ${cwd}\n`);
  const samples: number[] = [];
  for (let i = 1; i <= repeats; i++) {
    let r: CommandResult;
    try {
      r = await runShell(config.bench.run, cwd, opts.timeoutMs);
    } catch (err) {
      io.err(`run ${i}/${repeats}: could not start bench command: ${(err as Error).message}\n`);
      return 1;
    }
    if (r.timedOut) {
      io.err(`run ${i}/${repeats}: timed out after ${opts.timeoutMs}ms\n`);
      return 1;
    }
    if (r.exitCode !== 0) {
      io.err(`run ${i}/${repeats}: bench command exited with status ${r.exitCode}\n`);
      if (r.stderr.trim()) io.err(tail(r.stderr) + '\n');
      return 1;
    }
    const m = extractMetric(r.stdout, metric);
    if (!m.ok) {
      io.err(`run ${i}/${repeats}: ${m.error}\n`);
      return 1;
    }
    samples.push(m.value);
    progress(`  run ${i}/${repeats}: ${metric} = ${fmt(m.value)}\n`);
  }

  const k = opts.k ?? 2;
  const minAbsolute = opts.minAbsolute ?? 0;
  let floor: number;
  try {
    floor = noiseFloor(samples, { k, minAbsolute });
  } catch (err) {
    io.err(`${(err as Error).message}\n`);
    return 1;
  }
  const summary = summarize(samples);
  const relativeGain = config.objective.min_gain * Math.abs(summary.median);
  const requiredGain = Math.max(floor, relativeGain);

  if (json) {
    io.out(
      JSON.stringify(
        {
          metric,
          direction: config.objective.direction,
          command: config.bench.run,
          repeats,
          samples,
          summary,
          noiseFloor: floor,
          noiseFloorFormula: { k, minAbsolute },
          minGain: config.objective.min_gain,
          requiredGain,
        },
        null,
        2,
      ) + '\n',
    );
    return 0;
  }

  const lines = [
    '',
    `  runs        ${summary.n}`,
    `  mean        ${fmt(summary.mean)}`,
    `  median      ${fmt(summary.median)}`,
    `  stddev      ${fmt(summary.stddev)}`,
    `  cv          ${Number.isFinite(summary.cv) ? (summary.cv * 100).toFixed(2) + '%' : 'inf'}`,
    `  min / max   ${fmt(summary.min)} / ${fmt(summary.max)}`,
    `  noise floor ${fmt(floor)}  (max(${k} x stddev, ${minAbsolute}))`,
    `  required gain ${fmt(requiredGain)}  (max(noise floor, min_gain ${config.objective.min_gain} x median = ${fmt(relativeGain)}))`,
  ];
  if (repeats < 3) lines.push('  warning: fewer than 3 runs; the noise floor is unreliable');
  io.out(lines.join('\n') + '\n');
  return 0;
}
