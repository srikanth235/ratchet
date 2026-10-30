import type { BenchStats } from '@ratchet/core';

const r3 = (n: number) => Math.round(n * 1000) / 1000;

/**
 * Stats row of a simulated evaluator run, in the shape the real engine stores (`value` = headline median, every
 * evaluator metric as `metric:<name>`). For the search demo's objective it mirrors what examples/demo-search's
 * local bench prints: the latency percentiles, rps, error rate, peak RSS and the p95 request's spans (search time
 * shrinks with the optimization, queueing shrinks with it). Dev engine and seed only: simulated numbers.
 */
export function simulatedStats(metric: string, v: number, extra: Record<string, number> = {}): BenchStats {
  const stats: BenchStats = { value: v, [`metric:${metric}`]: v, ...extra };
  if (metric === 'http_req_duration.p95') {
    const search = Math.max(1.5, 0.55 * (v - 118));
    const serialize = 0.03 * v;
    const parse = 0.9;
    Object.assign(stats, {
      'metric:http_req_duration.p50': r3(v * 0.41),
      'metric:http_req_duration.p99': r3(v * 1.58),
      'metric:rps': r3(498 - (v - 120) * 0.08),
      'metric:error_rate': 0.0002,
      'metric:peak_rss_mb': Math.round(150 + (v - 120) * 0.4),
      'metric:span.parse': parse,
      'metric:span.search': r3(search),
      'metric:span.serialize': r3(serialize),
      'metric:span.wait': r3(Math.max(0, v - parse - search - serialize)),
    });
  }
  return stats;
}
