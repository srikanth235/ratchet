import { isMap, isSeq, parse, parseDocument, stringify, type Document } from 'yaml';

export const EVALUATOR_KINDS = ['benchmark', 'backtest', 'eval', 'simulation'] as const;
export type EvaluatorKind = (typeof EVALUATOR_KINDS)[number];

/**
 * Form state of the "New optimization" screen. It is a view of ratchet.yaml, not a second source of truth: the
 * screen keeps the YAML text, reads the form with `formFromYaml` and writes form edits back with `patchConfigYaml`
 * (so comments and keys the form does not know survive). `parseConfig` in the main process judges validity.
 */
export interface ConfigForm {
  metric: string;
  direction: 'minimize' | 'maximize';
  /** Percent, e.g. 3 = 3%. */
  minGainPct: number;
  evaluatorKind: EvaluatorKind;
  evaluatorRef: string;
  evaluatorRun: string;
  repeats: number;
  hardware: string;
  /** Sealed holdout (hidden trace, unseen period); empty = none. */
  holdout: string;
  /** `objective.value.usd_per_unit`; null = no value model (the whole `objective.value` block is left out). */
  valueUsdPerUnit: number | null;
  valuePeriod: 'month' | 'year';
  valueBasis: 'estimated' | 'measured';
  valueUnit: string;
  /** One guardrail per entry: `run: pnpm test` or `error_rate: < 0.001`. */
  guardrails: string[];
  /** One glob per line. */
  protectedGlobs: string;
  instructions: string;
  /** Comma separated. */
  instanceTypes: string;
}

export const DEFAULT_FORM: ConfigForm = {
  metric: 'http_req_duration.p95',
  direction: 'minimize',
  minGainPct: 3,
  evaluatorKind: 'benchmark',
  evaluatorRef: 'ratchet/bench',
  evaluatorRun: 'k6 run bench/k6/search.js',
  repeats: 5,
  hardware: 'c7i.2xlarge',
  holdout: '',
  valueUsdPerUnit: null,
  valuePeriod: 'year',
  valueBasis: 'estimated',
  valueUnit: '',
  guardrails: ['run: pnpm test:unit', 'error_rate: < 0.001', 'peak_rss: < 1.2x'],
  protectedGlobs: 'bench/**\nratchet.yaml',
  instructions: 'RATCHET.md',
  instanceTypes: 'c7i.2xlarge, c7i.xlarge, m7i.large',
};

const lines = (s: string): string[] =>
  s
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

const list = (s: string): string[] =>
  s
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);

/** Turn one `key: value` guardrail line into a single-key mapping (invalid lines pass through as a bare string so the validator reports them). */
export function parseGuardrailLine(line: string): Record<string, string> | string {
  const i = line.indexOf(':');
  if (i <= 0) return line;
  return { [line.slice(0, i).trim()]: line.slice(i + 1).trim() };
}

/** Inverse of `parseGuardrailLine` for one YAML guardrail entry. */
function guardrailText(g: unknown): string | null {
  if (typeof g === 'string') return g;
  if (g && typeof g === 'object' && !Array.isArray(g)) {
    const [k, v] = Object.entries(g as Record<string, unknown>)[0] ?? [];
    if (k !== undefined) return `${k}: ${String(v ?? '')}`;
  }
  return null;
}

function valueBlock(f: ConfigForm): Record<string, unknown> | undefined {
  if (f.valueUsdPerUnit === null) return undefined;
  return { usd_per_unit: f.valueUsdPerUnit, period: f.valuePeriod, basis: f.valueBasis, ...(f.valueUnit.trim() ? { unit: f.valueUnit.trim() } : {}) };
}

const minGain = (pct: number) => Number((pct / 100).toFixed(6));

/** Generate ratchet.yaml text from the form (the engine/`parseConfig` is the judge of validity). */
export function buildConfigYaml(f: ConfigForm): string {
  const value = valueBlock(f);
  const doc = {
    version: 0,
    objective: { metric: f.metric.trim(), direction: f.direction, min_gain: minGain(f.minGainPct), ...(value ? { value } : {}) },
    evaluator: {
      kind: f.evaluatorKind,
      ref: f.evaluatorRef.trim(),
      run: f.evaluatorRun.trim(),
      repeats: f.repeats,
      hardware: f.hardware.trim(),
      ...(f.holdout.trim() ? { holdout: f.holdout.trim() } : {}),
    },
    guardrails: f.guardrails.map((l) => l.trim()).filter(Boolean).map(parseGuardrailLine),
    protected: lines(f.protectedGlobs),
    agent: { instructions: f.instructions.trim(), allowed_instance_types: list(f.instanceTypes) },
  };
  return stringify(doc, { lineWidth: 0 });
}

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const str = (v: unknown, d = ''): string => (typeof v === 'string' ? v : typeof v === 'number' ? String(v) : d);
const num = (v: unknown, d: number): number => (typeof v === 'number' && Number.isFinite(v) ? v : d);

/**
 * Read the form back from ratchet.yaml text, leniently (missing or odd values fall back to empty / defaults so the
 * form stays editable while the validator explains what is wrong). Null when the text is not a YAML mapping.
 */
export function formFromYaml(text: string): ConfigForm | null {
  let raw: unknown;
  try {
    raw = parse(text);
  } catch {
    return null;
  }
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
  const d = raw as Record<string, unknown>;
  const o = obj(d['objective']);
  const e = obj(d['evaluator'] ?? d['bench']);
  const v = d['objective'] && 'value' in o ? obj(o['value']) : null;
  const a = obj(d['agent']);
  const kind = str(e['kind'], 'benchmark');
  const usd = v ? v['usd_per_unit'] : undefined;
  return {
    metric: str(o['metric']),
    direction: o['direction'] === 'maximize' ? 'maximize' : 'minimize',
    minGainPct: Number((num(o['min_gain'], 0) * 100).toFixed(4)),
    evaluatorKind: (EVALUATOR_KINDS as readonly string[]).includes(kind) ? (kind as EvaluatorKind) : 'benchmark',
    evaluatorRef: str(e['ref']),
    evaluatorRun: str(e['run']),
    repeats: num(e['repeats'], 0),
    hardware: str(e['hardware']),
    holdout: str(e['holdout'] ?? e['confirm_trace']),
    valueUsdPerUnit: typeof usd === 'number' && Number.isFinite(usd) ? usd : null,
    valuePeriod: v?.['period'] === 'month' ? 'month' : 'year',
    valueBasis: v?.['basis'] === 'measured' ? 'measured' : 'estimated',
    valueUnit: str(v?.['unit']),
    guardrails: Array.isArray(d['guardrails']) ? d['guardrails'].map(guardrailText).filter((g): g is string => g !== null) : [],
    protectedGlobs: Array.isArray(d['protected']) ? d['protected'].map((p) => str(p)).join('\n') : '',
    instructions: str(a['instructions']),
    instanceTypes: Array.isArray(a['allowed_instance_types']) ? a['allowed_instance_types'].map((t) => str(t)).join(', ') : '',
  };
}

/** Set `path` to `value`, keeping a list's flow style (`[a, b]`) when it had one. */
function setKeepStyle(doc: Document, path: (string | number)[], value: unknown): void {
  const prev = doc.getIn(path, true);
  const node = doc.createNode(value, { flow: isSeq(prev) || isMap(prev) ? Boolean((prev as { flow?: boolean }).flow) : false });
  doc.setIn(path, node);
}

/**
 * Write form fields into existing ratchet.yaml text, touching only the keys that changed (comments and unknown keys
 * survive). The evaluator is written under the key the text already uses (`evaluator:`, or the legacy `bench:`).
 * Text that is not a YAML mapping is returned unchanged.
 */
export function patchConfigYaml(text: string, patch: Partial<ConfigForm>): string {
  const doc = parseDocument(text);
  if (doc.errors.length > 0 || !isMap(doc.contents)) return text;
  const ev = doc.has('evaluator') || !doc.has('bench') ? 'evaluator' : 'bench';
  const set = (path: string[], value: unknown) => setKeepStyle(doc, path, value);
  for (const [k, raw] of Object.entries(patch) as [keyof ConfigForm, unknown][]) {
    switch (k) {
      case 'metric': set(['objective', 'metric'], raw); break;
      case 'direction': set(['objective', 'direction'], raw); break;
      case 'minGainPct': set(['objective', 'min_gain'], minGain(raw as number)); break;
      case 'evaluatorKind': set([ev, 'kind'], raw); break;
      case 'evaluatorRef': set([ev, 'ref'], raw); break;
      case 'evaluatorRun': set([ev, 'run'], raw); break;
      case 'repeats': set([ev, 'repeats'], raw); break;
      case 'hardware': set([ev, 'hardware'], raw); break;
      case 'holdout': {
        doc.deleteIn([ev, 'confirm_trace']);
        if (String(raw).trim()) set([ev, 'holdout'], raw);
        else doc.deleteIn([ev, 'holdout']);
        break;
      }
      case 'valueUsdPerUnit':
        if (raw === null) doc.deleteIn(['objective', 'value']);
        else {
          set(['objective', 'value', 'usd_per_unit'], raw);
          if (!doc.hasIn(['objective', 'value', 'period'])) set(['objective', 'value', 'period'], patch.valuePeriod ?? 'year');
          if (!doc.hasIn(['objective', 'value', 'basis'])) set(['objective', 'value', 'basis'], patch.valueBasis ?? 'estimated');
        }
        break;
      case 'valuePeriod':
      case 'valueBasis':
        if (doc.hasIn(['objective', 'value'])) set(['objective', 'value', k === 'valuePeriod' ? 'period' : 'basis'], raw);
        break;
      case 'valueUnit':
        if (!doc.hasIn(['objective', 'value'])) break;
        if (String(raw).trim()) set(['objective', 'value', 'unit'], raw);
        else doc.deleteIn(['objective', 'value', 'unit']);
        break;
      case 'guardrails': set(['guardrails'], (raw as string[]).map((l) => l.trim()).filter(Boolean).map(parseGuardrailLine)); break;
      case 'protectedGlobs': set(['protected'], lines(raw as string)); break;
      case 'instructions': set(['agent', 'instructions'], raw); break;
      case 'instanceTypes': set(['agent', 'allowed_instance_types'], list(raw as string)); break;
    }
  }
  return doc.toString({ lineWidth: 0, flowCollectionPadding: false });
}
