#!/usr/bin/env node
// HTTP latency evaluator (protected: bench/**). Starts your service, drives it with an open-loop load (a constant
// arrival rate, so a slow response cannot hide behind a slower client), and prints one JSON object on stdout:
//   { http_req_duration: { p50, p95, p99, mean, max }, rps, error_rate, requests }   (durations in ms)
//
// Configure it in bench/http-latency.config.json:
//   start        shell command that starts the service (it must listen on $PORT)
//   port         port the service listens on (passed as PORT)
//   ready_path   GET path that answers 2xx once the service is up
//   requests     [{ method?, path, headers?, body? }, ...] replayed in order; the VISIBLE mix agents may study
//   rate         requests per second;  duration_s / warmup_s: measured / discarded seconds
//   Put `error_rate: "< 0.001"` under guardrails in ratchet.yaml: a run with failing requests must not count as a win.
// RATCHET_TRACE, when set, names the sealed holdout mix: a JSON file { "requests": [...] } (or a bare array) that
// replaces `requests`. Env overrides for quick local runs: BENCH_RATE, BENCH_DURATION_S, BENCH_WARMUP_S, BENCH_URL
// (skip starting the service and hit an already running one).
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const here = dirname(fileURLToPath(import.meta.url));
const cfg = JSON.parse(readFileSync(join(here, 'http-latency.config.json'), 'utf8'));
const rate = Number(process.env.BENCH_RATE ?? cfg.rate ?? 100);
const durationS = Number(process.env.BENCH_DURATION_S ?? cfg.duration_s ?? 20);
const warmupS = Number(process.env.BENCH_WARMUP_S ?? cfg.warmup_s ?? 3);
const port = Number(cfg.port ?? 3000);
const base = new URL(process.env.BENCH_URL ?? `http://127.0.0.1:${port}`);

let requests = cfg.requests;
if (process.env.RATCHET_TRACE) {
  const trace = JSON.parse(readFileSync(process.env.RATCHET_TRACE, 'utf8'));
  requests = Array.isArray(trace) ? trace : trace.requests;
}
if (!Array.isArray(requests) || requests.length === 0) throw new Error('no requests to replay (bench/http-latency.config.json "requests")');

// On a live bench instance the candidate runs as another OS user, started through $RATCHET_CANDIDATE_EXEC, so it can
// neither read the holdout this process reads nor see RATCHET_TRACE. Elsewhere it simply runs as the current user.
let server;
if (!process.env.BENCH_URL) {
  const launcher = process.env.RATCHET_CANDIDATE_EXEC;
  const env = { ...process.env, PORT: String(port) };
  delete env.RATCHET_TRACE;
  server = launcher
    ? spawn(launcher, [`PORT=${port}`, 'sh', '-c', cfg.start], { stdio: ['ignore', 'ignore', 'inherit'], detached: true })
    : spawn(cfg.start, { shell: true, env, stdio: ['ignore', 'ignore', 'inherit'], detached: true });
}
// The service runs in its own process group so that stopping it also stops whatever the shell started.
const stop = () => {
  if (!server?.pid) return;
  try {
    process.kill(-server.pid, 'SIGTERM');
  } catch {
    server.kill('SIGTERM');
  }
};
process.on('exit', stop);

async function waitReady() {
  for (let i = 0; i < 300; i++) {
    try {
      const res = await fetch(new URL(cfg.ready_path ?? '/', base));
      await res.arrayBuffer();
      if (res.status < 500) return;
    } catch {
      // not up yet
    }
    await sleep(100);
  }
  throw new Error(`service did not become ready at ${base} within 30 s`);
}

async function one(spec, record) {
  const t0 = performance.now();
  let ok = false;
  try {
    const res = await fetch(new URL(spec.path, base), {
      method: spec.method ?? 'GET',
      headers: spec.headers,
      body: spec.body === undefined ? undefined : typeof spec.body === 'string' ? spec.body : JSON.stringify(spec.body),
    });
    await res.arrayBuffer();
    ok = res.status < 400;
  } catch {
    ok = false;
  }
  if (record) record.push({ ms: performance.now() - t0, ok });
}

try {
  await waitReady();
  const samples = [];
  const total = Math.round((warmupS + durationS) * rate);
  const warmupN = Math.round(warmupS * rate);
  const started = performance.now();
  const inflight = new Set();
  for (let i = 0; i < total; i++) {
    const due = started + (i * 1000) / rate;
    const wait = due - performance.now();
    if (wait > 1) await sleep(wait);
    const p = one(requests[i % requests.length], i >= warmupN ? samples : null).finally(() => inflight.delete(p));
    inflight.add(p);
  }
  await Promise.all(inflight);
  const measuredS = (performance.now() - started) / 1000 - warmupS;

  const times = samples.filter((s) => s.ok).map((s) => s.ms).sort((a, b) => a - b);
  const pick = (q) => (times.length ? times[Math.min(times.length - 1, Math.ceil(q * times.length) - 1)] : 0);
  const round = (x) => Math.round(x * 1000) / 1000;
  const failed = samples.length - times.length;
  console.log(
    JSON.stringify({
      http_req_duration: {
        p50: round(pick(0.5)),
        p95: round(pick(0.95)),
        p99: round(pick(0.99)),
        mean: round(times.reduce((a, b) => a + b, 0) / Math.max(1, times.length)),
        max: round(times.at(-1) ?? 0),
      },
      rps: round(samples.length / Math.max(0.001, measuredS)),
      error_rate: samples.length ? failed / samples.length : 1,
      requests: samples.length,
    }),
  );
} finally {
  stop();
}
