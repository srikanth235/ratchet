import { stringify } from 'yaml';

/** Form state of the "New optimization" screen that becomes ratchet.yaml. */
export interface ConfigForm {
  metric: string;
  direction: 'minimize' | 'maximize';
  /** Percent, e.g. 3 = 3%. */
  minGainPct: number;
  benchRef: string;
  benchRun: string;
  repeats: number;
  hardware: string;
  confirmTrace: string;
  /** One per line: `run: pnpm test` or `error_rate: < 0.001`. */
  guardrails: string;
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
  benchRef: 'ratchet/bench',
  benchRun: 'k6 run bench/k6/search.js',
  repeats: 5,
  hardware: 'c7i.2xlarge',
  confirmTrace: '',
  guardrails: 'run: pnpm test:unit\nerror_rate: < 0.001\npeak_rss: < 1.2x',
  protectedGlobs: 'bench/**\nratchet.yaml',
  instructions: 'RATCHET.md',
  instanceTypes: 'c7i.2xlarge, c7i.xlarge, m7i.large',
};

const lines = (s: string): string[] =>
  s
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

/** Turn one `key: value` guardrail line into a single-key mapping (invalid lines pass through as a bare string so the validator reports them). */
export function parseGuardrailLine(line: string): Record<string, string> | string {
  const i = line.indexOf(':');
  if (i <= 0) return line;
  return { [line.slice(0, i).trim()]: line.slice(i + 1).trim() };
}

/** Generate ratchet.yaml text from the form (the engine/`parseConfig` is the judge of validity). */
export function buildConfigYaml(f: ConfigForm): string {
  const doc = {
    version: 0,
    objective: { metric: f.metric.trim(), direction: f.direction, min_gain: Number((f.minGainPct / 100).toFixed(6)) },
    bench: {
      ref: f.benchRef.trim(),
      run: f.benchRun.trim(),
      repeats: f.repeats,
      hardware: f.hardware.trim(),
      ...(f.confirmTrace.trim() ? { confirm_trace: f.confirmTrace.trim() } : {}),
    },
    guardrails: lines(f.guardrails).map(parseGuardrailLine),
    protected: lines(f.protectedGlobs),
    agent: {
      instructions: f.instructions.trim(),
      allowed_instance_types: f.instanceTypes
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
    },
  };
  return stringify(doc, { lineWidth: 0 });
}
