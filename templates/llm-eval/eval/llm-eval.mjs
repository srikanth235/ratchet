#!/usr/bin/env node
// LLM eval evaluator (protected: eval/**). Runs `run(input)` from src/task.mjs over every case of an eval set, scores
// the answers and prints one JSON object on stdout:
//   { score, cost_per_call_usd, tokens_per_call, error_rate, p95_latency_ms, n }
// `score` is the mean case score in [0, 1]. `cost_per_call_usd` and `tokens_per_call` are what the task reports.
//
// The eval set is eval/dataset.jsonl, one JSON object per line:
//   { "input": ..., "expected": ..., "match": "exact" | "contains" | "regex" | "number" }   (match defaults to "exact")
// Matching is deterministic on purpose (a judge model would be one more thing that drifts between runs). To score with a
// judge, wrap it in a scorer here and keep its model, prompt and temperature pinned: this file is protected, so
// candidates cannot change how they are graded.
// RATCHET_TRACE, when set, names the sealed holdout eval set (same format) and replaces the visible one.
// Env overrides: EVAL_LIMIT (first N cases), EVAL_CONCURRENCY (default 4).
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../src/task.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const file = process.env.RATCHET_TRACE ?? join(here, 'dataset.jsonl');
let cases = readFileSync(file, 'utf8')
  .split('\n')
  .filter((l) => l.trim())
  .map((l) => JSON.parse(l));
if (process.env.EVAL_LIMIT) cases = cases.slice(0, Number(process.env.EVAL_LIMIT));
if (cases.length === 0) throw new Error(`eval set ${file} has no cases`);
const concurrency = Number(process.env.EVAL_CONCURRENCY ?? 4);

const norm = (v) => String(v ?? '').trim().toLowerCase();
const SCORERS = {
  exact: (out, exp) => (norm(out) === norm(exp) ? 1 : 0),
  contains: (out, exp) => (norm(out).includes(norm(exp)) ? 1 : 0),
  regex: (out, exp) => (new RegExp(String(exp), 'i').test(String(out ?? '')) ? 1 : 0),
  // Within 1% (or 1e-9) of the expected number, for answers that are quantities: the last number in the output.
  number: (out, exp) => {
    const a = Number(String(out).match(/-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g)?.at(-1));
    const b = Number(exp);
    return Number.isFinite(a) && Math.abs(a - b) <= Math.max(1e-9, Math.abs(b) * 0.01) ? 1 : 0;
  },
};

async function evaluate(c) {
  const t0 = performance.now();
  try {
    const res = await run(c.input);
    const output = res !== null && typeof res === 'object' && 'output' in res ? res.output : res;
    const scorer = SCORERS[c.match ?? 'exact'];
    if (!scorer) throw new Error(`unknown match type ${c.match}`);
    return {
      score: scorer(output, c.expected),
      cost: Number(res?.cost_usd ?? 0),
      tokens: Number(res?.tokens ?? 0),
      ms: performance.now() - t0,
      error: false,
    };
  } catch (err) {
    console.error(`case failed: ${err.message}`);
    return { score: 0, cost: 0, tokens: 0, ms: performance.now() - t0, error: true };
  }
}

const results = new Array(cases.length);
let next = 0;
await Promise.all(
  Array.from({ length: Math.min(concurrency, cases.length) }, async () => {
    while (next < cases.length) {
      const i = next++;
      results[i] = await evaluate(cases[i]);
    }
  }),
);

const sum = (f) => results.reduce((a, r) => a + f(r), 0);
const ms = results.map((r) => r.ms).sort((a, b) => a - b);
const round = (x, d = 4) => Math.round(x * 10 ** d) / 10 ** d;
console.log(
  JSON.stringify({
    score: round(sum((r) => r.score) / results.length),
    cost_per_call_usd: round(sum((r) => r.cost) / results.length, 6),
    tokens_per_call: round(sum((r) => r.tokens) / results.length, 2),
    error_rate: sum((r) => (r.error ? 1 : 0)) / results.length,
    p95_latency_ms: round(ms[Math.min(ms.length - 1, Math.ceil(0.95 * ms.length) - 1)], 2),
    n: results.length,
  }),
);
