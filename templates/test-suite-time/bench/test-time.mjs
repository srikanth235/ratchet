#!/usr/bin/env node
// Test-suite time evaluator (protected: bench/**). Runs your test command and prints one JSON object on stdout:
//   { test_suite_ms, error_rate, exit_code, runs }
// `test_suite_ms` is the wall time of the test command (the median when `runs` > 1). `error_rate` is the share of runs
// whose exit code was not 0: a faster suite that stops passing is blocked by the `error_rate` guardrail.
//
// Configure it in bench/test-time.config.json:
//   command      the test command, e.g. "npm test --silent" or "pnpm -r test"
//   runs         how many times to run it (the suite is long; 1 is usual, Ratchet repeats the whole evaluator itself)
//   timeout_s    kill a run after this many seconds (counts as a failure)
// Env overrides: BENCH_TEST_COMMAND, BENCH_RUNS.
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const cfg = JSON.parse(readFileSync(join(here, 'test-time.config.json'), 'utf8'));
const command = process.env.BENCH_TEST_COMMAND ?? cfg.command;
const runs = Number(process.env.BENCH_RUNS ?? cfg.runs ?? 1);
const timeoutMs = Number(cfg.timeout_s ?? 900) * 1000;
if (!command) throw new Error('no test command (bench/test-time.config.json "command")');

// On a live bench instance the candidate's code runs as another OS user, through $RATCHET_CANDIDATE_EXEC.
const launcher = process.env.RATCHET_CANDIDATE_EXEC;
const quote = (s) => `'${s.replace(/'/g, `'\\''`)}'`;
const full = launcher ? `${launcher} sh -c ${quote(command)}` : command;
const env = { ...process.env, CI: 'true' };
delete env.RATCHET_TRACE;

function once() {
  return new Promise((resolve) => {
    const t0 = process.hrtime.bigint();
    const child = spawn(full, { shell: true, stdio: ['ignore', 'ignore', 'inherit'], env, detached: true });
    const timer = setTimeout(() => {
      try {
        process.kill(-child.pid, 'SIGKILL');
      } catch {
        child.kill('SIGKILL');
      }
    }, timeoutMs);
    const done = (code) => {
      clearTimeout(timer);
      resolve({ ms: Number(process.hrtime.bigint() - t0) / 1e6, code: code ?? 1 });
    };
    child.on('error', () => done(1));
    child.on('close', done);
  });
}

const results = [];
for (let i = 0; i < runs; i++) results.push(await once());
const sorted = results.map((r) => r.ms).sort((a, b) => a - b);
const mid = sorted.length >> 1;
const median = sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
const failures = results.filter((r) => r.code !== 0).length;
console.log(
  JSON.stringify({
    test_suite_ms: Math.round(median * 1000) / 1000,
    error_rate: failures / results.length,
    exit_code: results.at(-1).code,
    runs: results.length,
  }),
);
