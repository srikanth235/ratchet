/**
 * Pure-Node open-loop load generator mirroring bench/k6/search.js:
 * constant arrival rate, query mix replayed in order, JSON summary out.
 *
 *   node dist/bench/local.js [--rate 500] [--duration 60] [--warmup 5]
 *        [--mix bench/query-mix.json] [--out result.json] [--url http://host:port]
 *
 * Without --url it starts the built server (dist/src/main.js) itself and also
 * reports its peak RSS. Output: { http_req_duration: {p50,p95,p99}, rps, error_rate, ... }
 * (durations in ms).
 */
import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import http from "node:http";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import type { MixQuery } from "./traces/generate.js";

const { values } = parseArgs({
  options: {
    rate: { type: "string", default: "500" },
    duration: { type: "string", default: "60" },
    warmup: { type: "string", default: "5" },
    mix: { type: "string", default: fileURLToPath(new URL("../../bench/query-mix.json", import.meta.url)) },
    out: { type: "string" },
    url: { type: "string" },
    port: { type: "string", default: "3999" },
  },
});

const rate = Number(values.rate);
const durationS = Number(values.duration);
const warmupS = Number(values.warmup);
const mix = (JSON.parse(readFileSync(values.mix as string, "utf8")) as { queries: MixQuery[] }).queries;

function pathFor(q: MixQuery): string {
  const sp = new URLSearchParams();
  if (q.q) sp.set("q", q.q);
  if (q.filters) sp.set("filters", q.filters);
  if (q.limit) sp.set("limit", String(q.limit));
  if (q.offset) sp.set("offset", String(q.offset));
  return `/search?${sp}`;
}
const paths = mix.map(pathFor);

let server: ChildProcess | undefined;
let base: URL;
if (values.url) {
  base = new URL(values.url);
} else {
  const serverJs = fileURLToPath(new URL("../src/main.js", import.meta.url));
  // On a live bench instance the evaluator runs as a different OS user than the candidate. The candidate's
  // server is then started through the launcher ($RATCHET_CANDIDATE_EXEC), which runs it as the candidate
  // user in the candidate's checkout with a clean environment: it cannot open the holdout this process
  // reads (--mix) and never sees RATCHET_TRACE. Without the launcher (simulation, a developer machine)
  // everything runs as the current user, as before. The server's stdout is not ours: our stdout carries the result.
  const launcher = process.env.RATCHET_CANDIDATE_EXEC;
  server = launcher
    ? spawn(launcher, [`PORT=${values.port}`, "node", serverJs], { stdio: ["ignore", "ignore", "inherit"] })
    : spawn(process.execPath, [serverJs], { env: { ...process.env, PORT: values.port }, stdio: "inherit" });
  base = new URL(`http://127.0.0.1:${values.port}`);
}

const agent = new http.Agent({ keepAlive: true, maxSockets: 256 });
const isHealthy = () =>
  new Promise<boolean>((resolve) => {
    http.get({ host: base.hostname, port: base.port, path: "/health", agent }, (res) => {
      res.resume();
      resolve(res.statusCode === 200);
    }).on("error", () => resolve(false));
  });
for (let i = 0; ; i++) {
  if (await isHealthy()) break;
  if (i > 100) throw new Error("server did not become healthy");
  await sleep(100);
}

interface Sample { ms: number; ok: boolean }

/** Fire `total` requests at `rate`/s, request i uses paths[i % n]; latency is measured from send. */
async function run(total: number, startIndex: number): Promise<{ samples: Sample[]; seconds: number }> {
  const samples: Sample[] = [];
  const t0 = performance.now();
  const interval = 1000 / rate;
  let sent = 0;
  let pending = 0;
  const fire = (i: number) => {
    const start = performance.now();
    pending++;
    let settled = false;
    const done = (ok: boolean) => {
      if (settled) return;
      settled = true;
      pending--;
      samples.push({ ms: performance.now() - start, ok });
    };
    const req = http.get(
      { host: base.hostname, port: base.port, path: paths[(startIndex + i) % paths.length], agent, timeout: 10_000 },
      (res) => {
        res.resume();
        res.on("end", () => done(res.statusCode === 200));
        res.on("error", () => done(false));
      },
    );
    req.on("timeout", () => { req.destroy(); done(false); });
    req.on("error", () => done(false));
  };
  while (sent < total) {
    const due = Math.min(total, Math.floor((performance.now() - t0) / interval) + 1);
    while (sent < due) fire(sent++);
    await sleep(1);
  }
  while (pending > 0) await sleep(5);
  return { samples, seconds: (performance.now() - t0) / 1000 };
}

function peakRssMb(pid: number): number | undefined {
  try {
    const m = /VmHWM:\s+(\d+) kB/.exec(readFileSync(`/proc/${pid}/status`, "utf8"));
    return m ? Math.round(Number(m[1]) / 1024) : undefined;
  } catch {
    return undefined; // not Linux
  }
}

const warmupTotal = Math.round(warmupS * rate);
if (warmupTotal > 0) await run(warmupTotal, 0);
const total = Math.round(durationS * rate);
const { samples, seconds } = await run(total, warmupTotal);

const sorted = samples.map((s) => s.ms).sort((a, b) => a - b);
const pct = (p: number) => sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)] as number;
const round = (n: number) => Math.round(n * 1000) / 1000;
const result = {
  http_req_duration: { p50: round(pct(50)), p95: round(pct(95)), p99: round(pct(99)) },
  rps: round(samples.length / seconds),
  error_rate: samples.filter((s) => !s.ok).length / samples.length,
  requests: samples.length,
  ...(server?.pid ? { peak_rss_mb: peakRssMb(server.pid) } : {}),
};

server?.kill();
agent.destroy();
const json = JSON.stringify(result, null, 2);
if (values.out) writeFileSync(values.out, json + "\n");
console.log(json);
