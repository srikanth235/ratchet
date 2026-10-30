import { median } from '@ratchet/core';
import type { BenchRun } from '@ratchet/core';

/**
 * Flatten every finite numeric leaf of a JSON value into dotted keys:
 * `{http_req_duration: {p95: 12}, rps: 3}` -> `{'http_req_duration.p95': 12, rps: 3}`.
 * Arrays, strings, booleans and nulls are skipped.
 */
export function flattenNumeric(value: unknown, prefix = '', out: Record<string, number> = {}): Record<string, number> {
  if (typeof value === 'number') {
    if (Number.isFinite(value) && prefix !== '') out[prefix] = value;
  } else if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      flattenNumeric(v, prefix === '' ? k : `${prefix}.${k}`, out);
    }
  }
  return out;
}

/** Per-metric median over runs (a metric is aggregated over the runs that report it). */
export function medianMetrics(runs: readonly Pick<BenchRun, 'metrics'>[]): Record<string, number> {
  const byKey = new Map<string, number[]>();
  for (const r of runs) {
    for (const [k, v] of Object.entries(r.metrics)) {
      const xs = byKey.get(k);
      if (xs) xs.push(v);
      else byKey.set(k, [v]);
    }
  }
  const out: Record<string, number> = {};
  for (const [k, xs] of byKey) out[k] = median(xs);
  return out;
}

const UNIT_SUFFIXES = ['_mb', '_kb', '_gb', '_bytes', '_ms', '_s', '_pct', '_percent'];

/**
 * Look a guardrail metric up in a flat metrics map: the exact key first, then the key with a unit
 * suffix (`peak_rss` finds `peak_rss_mb`). The demo config says `peak_rss: "< 1.2x"` while the bench
 * prints `peak_rss_mb`; relative bounds are unit-free, so the alias is safe.
 */
export function resolveMetric(metrics: Readonly<Record<string, number>>, name: string): { key: string; value: number } | undefined {
  if (Object.prototype.hasOwnProperty.call(metrics, name)) return { key: name, value: metrics[name]! };
  for (const s of UNIT_SUFFIXES) {
    const k = name + s;
    if (Object.prototype.hasOwnProperty.call(metrics, k)) return { key: k, value: metrics[k]! };
  }
  return undefined;
}
