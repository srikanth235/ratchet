#!/usr/bin/env node
// Peak memory evaluator (protected: bench/**). Runs your workload command and prints one JSON object on stdout:
//   { peak_rss_mb, peak_rss_mb_runs: [...], wall_ms, error_rate, runs }
// `peak_rss_mb` is the largest resident set size the workload's process tree reached (the median over `runs`).
// RSS is sampled every few milliseconds from /proc (Linux, the bench machines) or `ps` (macOS, local runs), summed
// over the command and every process it started.
//
// Configure it in bench/memory.config.json:
//   command      the workload, e.g. "node dist/import-large-file.js"; $WORKLOAD is the workload input path (below)
//   workload     the VISIBLE workload input agents may study (default bench/workload.json)
//   runs         how many times to run it
// RATCHET_TRACE, when set, names the sealed holdout workload and is exported to the command as $WORKLOAD instead.
// Env overrides: BENCH_COMMAND, BENCH_RUNS, BENCH_SAMPLE_MS.
import { spawn, spawnSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const cfg = JSON.parse(readFileSync(join(here, 'memory.config.json'), 'utf8'));
const command = process.env.BENCH_COMMAND ?? cfg.command;
const runs = Number(process.env.BENCH_RUNS ?? cfg.runs ?? 3);
const sampleMs = Number(process.env.BENCH_SAMPLE_MS ?? 5);
if (!command) throw new Error('no workload command (bench/memory.config.json "command")');
const workload = process.env.RATCHET_TRACE ?? resolve(here, '..', cfg.workload ?? 'bench/workload.json');

// On a live bench instance the candidate runs as another OS user, through $RATCHET_CANDIDATE_EXEC. The holdout
// workload is handed over as a path the candidate can read once the harness makes it readable to it.
const launcher = process.env.RATCHET_CANDIDATE_EXEC;
const quote = (s) => `'${s.replace(/'/g, `'\\''`)}'`;
const full = launcher ? `${launcher} WORKLOAD=${quote(workload)} sh -c ${quote(command)}` : command;
const env = { ...process.env, WORKLOAD: workload };
delete env.RATCHET_TRACE;

const isLinux = process.platform === 'linux';

/** pid -> { ppid, rssKb } for every process (Linux /proc, or `ps` elsewhere). */
function snapshot() {
  const table = new Map();
  if (isLinux) {
    for (const name of readdirSync('/proc')) {
      if (!/^\d+$/.test(name)) continue;
      try {
        const stat = readFileSync(`/proc/${name}/stat`, 'utf8');
        const ppid = Number(stat.slice(stat.lastIndexOf(')') + 2).split(' ')[1]);
        const status = readFileSync(`/proc/${name}/status`, 'utf8');
        const hwm = /VmHWM:\s+(\d+) kB/.exec(status) ?? /VmRSS:\s+(\d+) kB/.exec(status);
        table.set(Number(name), { ppid, rssKb: hwm ? Number(hwm[1]) : 0 });
      } catch {
        // the process exited while we looked
      }
    }
  } else {
    const ps = spawnSync('ps', ['-A', '-o', 'pid=,ppid=,rss='], { encoding: 'utf8' });
    for (const line of ps.stdout.split('\n')) {
      const [pid, ppid, rss] = line.trim().split(/\s+/).map(Number);
      if (Number.isFinite(pid)) table.set(pid, { ppid, rssKb: rss });
    }
  }
  return table;
}

function treeRssKb(rootPid) {
  const table = snapshot();
  const children = new Map();
  for (const [pid, p] of table) children.set(p.ppid, [...(children.get(p.ppid) ?? []), pid]);
  let sum = 0;
  const stack = [rootPid];
  while (stack.length) {
    const pid = stack.pop();
    sum += table.get(pid)?.rssKb ?? 0;
    stack.push(...(children.get(pid) ?? []));
  }
  return sum;
}

function once() {
  return new Promise((resolveRun) => {
    const t0 = process.hrtime.bigint();
    const child = spawn(full, { shell: true, stdio: ['ignore', 'ignore', 'inherit'], env });
    let peakKb = 0;
    const timer = setInterval(() => {
      peakKb = Math.max(peakKb, treeRssKb(child.pid));
    }, sampleMs);
    const done = (code) => {
      clearInterval(timer);
      resolveRun({ mb: peakKb / 1024, ms: Number(process.hrtime.bigint() - t0) / 1e6, ok: code === 0 });
    };
    child.on('error', () => done(1));
    child.on('close', done);
  });
}

const results = [];
for (let i = 0; i < runs; i++) results.push(await once());
const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const round = (x) => Math.round(x * 1000) / 1000;
console.log(
  JSON.stringify({
    peak_rss_mb: round(median(results.map((r) => r.mb))),
    peak_rss_mb_runs: results.map((r) => round(r.mb)),
    wall_ms: round(median(results.map((r) => r.ms))),
    error_rate: results.filter((r) => !r.ok).length / results.length,
    runs: results.length,
  }),
);
