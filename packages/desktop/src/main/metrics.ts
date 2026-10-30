import { parseConfig } from '@ratchet/core';
import type { CompareRow, MetricOption, SpanRow } from '../shared/types.js';

/**
 * Evaluator metrics as stored in the ledger's `BenchStats`: `value` is the headline (the objective), every flattened
 * metric of the evaluator's JSON rides along as `metric:<name>` (engine) or `metrics.<name>` (bench CLI) medians.
 */
const PREFIXES = ['metric:', 'metrics.'];
export const SPAN_PREFIX = 'span.';

/** Flattened metric name → median value, from a stats record. */
export function metricsOf(stats: Readonly<Record<string, number>>): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(stats)) {
    const p = PREFIXES.find((x) => k.startsWith(x));
    if (p && Number.isFinite(v)) out[k.slice(p.length)] = v;
  }
  return out;
}

/** Value of metric `key` in a stats record; the objective reads the headline `value`. */
export function statOf(stats: Readonly<Record<string, number>> | undefined, key: string, objective: string): number | null {
  if (!stats) return null;
  if (key === objective && Number.isFinite(stats.value)) return stats.value!;
  const v = metricsOf(stats)[key];
  return v === undefined ? null : v;
}

/** Best-effort display unit from a metric key such as `http_req_duration.p95`. */
export function unitForMetric(metric: string): string {
  const m = metric.toLowerCase();
  if (/(^|[._])(rss|mem|memory|heap)/.test(m) || /_mb$/.test(m)) return 'MB';
  if (/(throughput|events_per_s|per_s|rps|qps)/.test(m)) return 'k/s';
  if (/(cold_start|\.s$|_s$|seconds)/.test(m)) return 's';
  if (/(duration|latency|render|\.p\d+|_ms$|ms)/.test(m)) return 'ms';
  return '';
}

const UNIT_SUFFIXES = ['_mb', '_kb', '_gb', '_bytes', '_ms', '_s', '_pct', '_percent'];

/** Threshold guardrails of a config: metric → which way is better (`< x` = lower). */
export function guardrailMetrics(configYaml: string): { metric: string; better: 'lower' | 'higher' }[] {
  const r = parseConfig(configYaml);
  if (!r.ok) return [];
  return r.config.guardrails.flatMap((g) => (g.kind === 'threshold' ? [{ metric: g.metric, better: g.op.startsWith('>') ? 'higher' as const : 'lower' as const }] : []));
}

/** A guardrail names `peak_rss`, the evaluator prints `peak_rss_mb`: same lookup as the bench runner's. */
function resolveName(metrics: Record<string, number>, name: string): string | null {
  if (name in metrics) return name;
  for (const s of UNIT_SUFFIXES) if (name + s in metrics) return name + s;
  return null;
}

/**
 * Metrics that belong with the objective, in the evaluator's own order:
 * - nested objective (`http_req_duration.p95`): its siblings (`.p50`, `.p95`, `.p99`);
 * - top-level objective (`wape`): the other top-level metrics that are not guardrails (`mape`, `bias`).
 * The objective is always included. Spans are never options.
 */
export function metricOptions(objective: string, configYaml: string, stats: Readonly<Record<string, number>>): MetricOption[] {
  const metrics = metricsOf(stats);
  const dot = objective.lastIndexOf('.');
  let names: string[];
  if (dot > 0) {
    const parent = objective.slice(0, dot + 1);
    names = Object.keys(metrics).filter((k) => k.startsWith(parent) && !k.slice(parent.length).includes('.'));
  } else {
    const guards = new Set(guardrailMetrics(configYaml).map((g) => resolveName(metrics, g.metric)).filter(Boolean));
    names = Object.keys(metrics).filter((k) => !k.includes('.') && !guards.has(k));
  }
  if (!names.includes(objective)) names.unshift(objective);
  // Percentile siblings read best in order: p50, p95, p99.
  const pct = (k: string) => /\.p(\d+(?:\.\d+)?)$/.exec(k)?.[1];
  if (names.every((k) => pct(k) !== undefined)) names.sort((a, b) => Number(pct(a)) - Number(pct(b)));
  return names.map((key) => ({
    key,
    label: dot > 0 && key.startsWith(objective.slice(0, dot + 1)) ? key.slice(dot + 1) : key,
    unit: unitForMetric(key),
    objective: key === objective,
  }));
}

/** Baseline vs best for the objective family, then every threshold-guardrail metric the evaluator reported. */
export function compareRows(
  objective: string,
  direction: 'minimize' | 'maximize',
  configYaml: string,
  baseline: Readonly<Record<string, number>>,
  best: Readonly<Record<string, number>> | undefined,
): CompareRow[] {
  if (!best) return [];
  const better = direction === 'minimize' ? 'lower' : 'higher';
  const row = (key: string, label: string, b: 'lower' | 'higher'): CompareRow => {
    const a = statOf(baseline, key, objective);
    const z = statOf(best, key, objective);
    return { key, label, unit: unitForMetric(key), baseline: a, best: z, deltaPct: a !== null && z !== null && a !== 0 ? ((z - a) / Math.abs(a)) * 100 : null, better: b };
  };
  const rows = metricOptions(objective, configYaml, baseline).map((o) => row(o.key, o.label, better));
  const seen = new Set(rows.map((r) => r.key));
  const metrics = metricsOf(baseline);
  for (const g of guardrailMetrics(configYaml)) {
    const key = resolveName(metrics, g.metric);
    if (key && !seen.has(key)) {
      seen.add(key);
      rows.push(row(key, key, g.better));
    }
  }
  return rows;
}

/** "Where the time went": `span.<name>` metrics of the baseline vs the best verified run; null when either lacks them. */
export function spanRows(baseline: Readonly<Record<string, number>>, best: Readonly<Record<string, number>> | undefined): SpanRow[] | null {
  if (!best) return null;
  const spans = (s: Readonly<Record<string, number>>) =>
    Object.entries(metricsOf(s)).filter(([k]) => k.startsWith(SPAN_PREFIX)).map(([k, v]) => [k.slice(SPAN_PREFIX.length), v] as const);
  const a = spans(baseline);
  const b = new Map(spans(best));
  const rows = a.filter(([k]) => b.has(k)).map(([name, v]) => ({ name, baseline: v, best: b.get(name)! }));
  return rows.length ? rows : null;
}
