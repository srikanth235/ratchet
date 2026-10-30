import { parse } from 'yaml';

/**
 * What the New optimization screen shows about a pasted ratchet.yaml beyond the validator's own summary:
 * the evaluator (the trusted measurement, `bench:` is its legacy alias) and the business value of the objective.
 * Display only: `parseConfig` in the main process stays the judge of validity, so this is deliberately lenient.
 */
export interface YamlSummary {
  /** `objective.name`, used as the default optimization name. */
  name: string | null;
  evaluator: { kind: string; ref: string | null; run: string | null; repeats: number | null; hardware: string | null; holdout: string | null } | null;
  value: { usdPerUnit: number; period: 'month' | 'year'; basis: 'estimated' | 'measured'; unit: string | null } | null;
}

const str = (v: unknown): string | null => (typeof v === 'string' && v.trim() !== '' ? v.trim() : null);
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);

export function summarizeYaml(text: string): YamlSummary | null {
  let doc: unknown;
  try {
    doc = parse(text);
  } catch {
    return null;
  }
  if (typeof doc !== 'object' || doc === null) return null;
  const d = doc as Record<string, unknown>;
  const obj = (typeof d['objective'] === 'object' && d['objective'] !== null ? d['objective'] : {}) as Record<string, unknown>;
  const evRaw = d['evaluator'] ?? d['bench'];
  const ev = typeof evRaw === 'object' && evRaw !== null ? (evRaw as Record<string, unknown>) : null;
  const v = typeof obj['value'] === 'object' && obj['value'] !== null ? (obj['value'] as Record<string, unknown>) : null;
  const usd = v ? num(v['usd_per_unit']) : null;
  return {
    name: str(obj['name']),
    evaluator: ev
      ? {
          kind: str(ev['kind']) ?? 'benchmark',
          ref: str(ev['ref']),
          run: str(ev['run']),
          repeats: num(ev['repeats']),
          hardware: str(ev['hardware']),
          holdout: str(ev['holdout'] ?? ev['confirm_trace']),
        }
      : null,
    value:
      v && usd !== null
        ? { usdPerUnit: usd, period: v['period'] === 'month' ? 'month' : 'year', basis: v['basis'] === 'measured' ? 'measured' : 'estimated', unit: str(v['unit']) }
        : null,
  };
}
