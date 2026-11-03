import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/** One CLI invocation of the benchmark: arguments after `taskr`, and the exit code it must end with. */
export interface Invocation {
  args: string[];
  expect?: number;
}

export interface Mix {
  seed: number;
  invocations: Invocation[];
}

/** The first of the paths (relative to this file) that exists: this module runs from bench/ (tests) and from dist/bench/. */
function firstExisting(...relative: string[]): string {
  const urls = relative.map((r) => new URL(r, import.meta.url));
  return fileURLToPath(urls.find((u) => existsSync(u)) ?? urls[0]!);
}

export const CLI_PATH = firstExisting('../src/main.js', '../dist/src/main.js');
export const VISIBLE_MIX = firstExisting('../../bench/invocations.json', '../bench/invocations.json');

export function loadMix(file: string): Mix {
  const mix = JSON.parse(readFileSync(file, 'utf8')) as Mix;
  if (!Array.isArray(mix.invocations) || mix.invocations.length === 0) throw new Error(`${file}: no invocations`);
  return mix;
}

/** Percentile by the nearest-rank method on an unsorted sample. */
export function percentile(samples: readonly number[], p: number): number {
  const sorted = [...samples].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1))]!;
}

const round = (n: number) => Math.round(n * 1000) / 1000;

/** The JSON both benches print: wall-clock milliseconds of one cold CLI start, over every timed run. */
export function summarize(timesMs: readonly number[], failures: number, invocations: number) {
  const mean = timesMs.reduce((a, b) => a + b, 0) / timesMs.length;
  return {
    cold_start_ms: {
      p50: round(percentile(timesMs, 50)),
      p95: round(percentile(timesMs, 95)),
      mean: round(mean),
      min: round(Math.min(...timesMs)),
      max: round(Math.max(...timesMs)),
    },
    invocations,
    runs: timesMs.length,
    error_rate: failures / timesMs.length,
  };
}
