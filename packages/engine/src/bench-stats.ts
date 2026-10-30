import { median, summarize, type BenchOutcome, type BenchStats } from '@ratchet/core';

/** Metric medians are stored in `BenchStats` under this prefix so they cannot clash with summary keys. */
export const METRIC_PREFIX = 'metric:';

export function samplesOf(outcome: BenchOutcome): number[] {
  const s = outcome.samples.length > 0 ? outcome.samples : outcome.runs.map((r) => r.value);
  if (s.length === 0) throw new Error('bench outcome has no samples');
  return [...s];
}

/** Median of every flattened metric across runs. */
export function medianMetrics(outcome: BenchOutcome): Record<string, number> {
  const byKey = new Map<string, number[]>();
  for (const run of outcome.runs) {
    for (const [k, v] of Object.entries(run.metrics)) {
      if (!Number.isFinite(v)) continue;
      const list = byKey.get(k) ?? [];
      list.push(v);
      byKey.set(k, list);
    }
  }
  const out: Record<string, number> = {};
  for (const [k, list] of byKey) out[k] = median(list);
  return out;
}

/** Ledger stats row: `value` is the headline (median); summary numbers and per-metric medians ride along. */
export function statsOf(outcome: BenchOutcome): BenchStats {
  const samples = samplesOf(outcome);
  const sum = summarize(samples);
  const stats: BenchStats = {
    value: sum.median,
    n: sum.n,
    mean: sum.mean,
    stddev: sum.stddev,
    cv: Number.isFinite(sum.cv) ? sum.cv : 0,
    min: sum.min,
    max: sum.max,
  };
  for (const [k, v] of Object.entries(medianMetrics(outcome))) stats[METRIC_PREFIX + k] = v;
  return stats;
}

/** Inverse of the prefixing in `statsOf`: the baseline metrics the bench runner needs for relative guardrails. */
export function metricsFromStats(stats: BenchStats): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(stats)) {
    if (k.startsWith(METRIC_PREFIX)) out[k.slice(METRIC_PREFIX.length)] = v;
  }
  return out;
}
