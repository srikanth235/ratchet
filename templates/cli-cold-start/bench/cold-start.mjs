#!/usr/bin/env node
// CLI cold-start evaluator (protected: bench/**). Times whole invocations of your command line, start to exit, and
// prints one JSON object on stdout:
//   { cold_start_ms: { p50, p95, mean, min, max }, invocations, runs, error_rate }
// Uses hyperfine (https://github.com/sharkdp/hyperfine) when it is installed, else spawns the commands itself.
//
// Configure it in bench/cold-start.config.json:
//   commands   shell commands to time; the VISIBLE set agents may study (build first if needed: see `prepare`)
//   prepare    optional shell command run once before timing (e.g. "npm run build")
//   runs       timed runs per command;  warmup: untimed runs per command
//   expect_exit  exit code that counts as success (default 0)
// RATCHET_TRACE, when set, names the sealed holdout invocations: a JSON file { "commands": [...] } (or a bare array)
// that replaces `commands`. Env overrides: BENCH_COMMAND (one command), BENCH_RUNS, BENCH_WARMUP, BENCH_NO_HYPERFINE=1.
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const cfg = JSON.parse(readFileSync(join(here, 'cold-start.config.json'), 'utf8'));
const runs = Number(process.env.BENCH_RUNS ?? cfg.runs ?? 10);
const warmup = Number(process.env.BENCH_WARMUP ?? cfg.warmup ?? 2);
const expectExit = cfg.expect_exit ?? 0;

let commands = process.env.BENCH_COMMAND ? [process.env.BENCH_COMMAND] : cfg.commands;
if (process.env.RATCHET_TRACE) {
  const trace = JSON.parse(readFileSync(process.env.RATCHET_TRACE, 'utf8'));
  commands = Array.isArray(trace) ? trace : trace.commands;
}
if (!Array.isArray(commands) || commands.length === 0) throw new Error('no commands to time (bench/cold-start.config.json "commands")');

// On a live bench instance the candidate runs as another OS user, through $RATCHET_CANDIDATE_EXEC. It runs
// `<launcher> sh -c <command>`; elsewhere the command runs as the current user.
const launcher = process.env.RATCHET_CANDIDATE_EXEC;
const childEnv = { ...process.env };
delete childEnv.RATCHET_TRACE;
const quote = (s) => `'${s.replace(/'/g, `'\\''`)}'`;
const wrap = (command) => (launcher ? `${launcher} sh -c ${quote(command)}` : command);

if (cfg.prepare) {
  const r = spawnSync(wrap(cfg.prepare), { shell: true, stdio: ['ignore', 'ignore', 'inherit'], env: childEnv });
  if (r.status !== 0) throw new Error(`prepare failed: ${cfg.prepare}`);
}

const round = (x) => Math.round(x * 1000) / 1000;
function summarize(times, failures) {
  const sorted = [...times].sort((a, b) => a - b);
  const pick = (q) => (sorted.length ? sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)] : 0);
  const total = times.length + failures;
  return {
    cold_start_ms: {
      p50: round(pick(0.5)),
      p95: round(pick(0.95)),
      mean: round(times.reduce((a, b) => a + b, 0) / Math.max(1, times.length)),
      min: round(sorted[0] ?? 0),
      max: round(sorted.at(-1) ?? 0),
    },
    invocations: commands.length,
    runs: total,
    error_rate: total ? failures / total : 1,
  };
}

function viaHyperfine() {
  if (process.env.BENCH_NO_HYPERFINE) return null;
  if (spawnSync('hyperfine', ['--version'], { stdio: 'ignore' }).status !== 0) return null;
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-hf-'));
  try {
    const out = join(dir, 'hyperfine.json');
    const args = ['--runs', String(runs), '--warmup', String(warmup), '--ignore-failure', '--export-json', out, ...commands.flatMap((c) => [wrap(c)])];
    const r = spawnSync('hyperfine', args, { stdio: ['ignore', 'ignore', 'inherit'], env: childEnv });
    if (r.status !== 0) return null;
    const times = [];
    let failures = 0;
    for (const res of JSON.parse(readFileSync(out, 'utf8')).results) {
      for (const t of res.times) times.push(t * 1000);
      for (const code of res.exit_codes) if (code !== expectExit) failures++;
    }
    return summarize(times, failures);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function once(command) {
  return new Promise((resolve) => {
    const t0 = process.hrtime.bigint();
    const child = spawn(wrap(command), { shell: true, stdio: 'ignore', env: childEnv });
    child.on('error', () => resolve({ ms: 0, ok: false }));
    child.on('close', (code) => resolve({ ms: Number(process.hrtime.bigint() - t0) / 1e6, ok: code === expectExit }));
  });
}

async function viaSpawn() {
  const times = [];
  let failures = 0;
  for (const command of commands) {
    for (let i = 0; i < warmup; i++) await once(command);
    for (let i = 0; i < runs; i++) {
      const r = await once(command);
      if (r.ok) times.push(r.ms);
      else failures++;
    }
  }
  return summarize(times, failures);
}

console.log(JSON.stringify(viaHyperfine() ?? (await viaSpawn())));
