import { z } from 'zod';

/** Comparison operators allowed in guardrail threshold strings. */
export const COMPARATORS = ['<', '<=', '>', '>=', '=='] as const;
export type Comparator = (typeof COMPARATORS)[number];

/** A guardrail that runs a command; passes iff exit status is 0. */
export interface CommandGuardrail {
  kind: 'command';
  run: string;
}

/**
 * A guardrail that compares a metric reported by the benchmark/run to a bound.
 *
 * `"< 0.001"` -> { op: '<', value: 0.001, relative: false }
 * `"< 1.2x"`  -> { op: '<', value: 1.2,   relative: true }  (observed < 1.2 * baseline)
 */
export interface ThresholdGuardrail {
  kind: 'threshold';
  metric: string;
  op: Comparator;
  value: number;
  /** True when written with an `x` suffix: the bound is a multiple of the baseline value. */
  relative: boolean;
  /** The original string, e.g. "< 1.2x". */
  raw: string;
}

export type Guardrail = CommandGuardrail | ThresholdGuardrail;

const THRESHOLD_RE = /^\s*(<=|>=|==|<|>)\s*([+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)\s*(x?)\s*$/;

/**
 * Parse a guardrail threshold string such as "< 0.001" or "<= 1.2x".
 * Returns an error message (string) when the string is malformed.
 */
export function parseThreshold(
  metric: string,
  raw: string,
): ThresholdGuardrail | string {
  const m = THRESHOLD_RE.exec(raw);
  if (!m) {
    return `invalid threshold ${JSON.stringify(raw)}: expected a comparator (${COMPARATORS.join(
      ', ',
    )}) followed by a number, optionally suffixed with "x" for a multiple of baseline (e.g. "< 0.001" or "< 1.2x")`;
  }
  const value = Number(m[2]);
  if (!Number.isFinite(value)) return `invalid threshold ${JSON.stringify(raw)}: number is not finite`;
  const relative = m[3] === 'x';
  if (relative && value <= 0) {
    return `invalid threshold ${JSON.stringify(raw)}: a baseline multiple must be greater than 0`;
  }
  return { kind: 'threshold', metric, op: m[1] as Comparator, value, relative, raw: raw.trim() };
}

const guardrailSchema = z
  .record(z.string(), z.string({ error: 'guardrail values must be strings (e.g. run: "pytest -q" or error_rate: "< 0.001")' }), {
    error: 'each guardrail must be a mapping with a single key, e.g. `run: pytest -q` or `error_rate: "< 0.001"`',
  })
  .transform((rec, ctx): Guardrail => {
    const keys = Object.keys(rec);
    const bail = (message: string, path: (string | number)[] = []): Guardrail => {
      ctx.issues.push({ code: 'custom', message, input: rec, path });
      return { kind: 'command', run: '' };
    };
    if (keys.length !== 1) {
      return bail(
        `a guardrail must have exactly one key (\`run\` or a metric name), found ${keys.length}${
          keys.length ? `: ${keys.join(', ')}` : ''
        }`,
      );
    }
    const key = keys[0]!;
    const value = rec[key]!;
    if (key === 'run') {
      if (value.trim() === '') return bail('`run` must be a non-empty command', ['run']);
      return { kind: 'command', run: value };
    }
    const parsed = parseThreshold(key, value);
    if (typeof parsed === 'string') return bail(parsed, [key]);
    return parsed;
  });

const nonEmpty = (what: string) => z.string({ error: `${what} must be a string` }).min(1, `${what} must not be empty`);

export const configSchema = z.strictObject({
  version: z.literal(0, { error: 'version must be 0 (the only supported ratchet.yaml version)' }),
  objective: z.strictObject({
    metric: nonEmpty('objective.metric'),
    direction: z.enum(['minimize', 'maximize'], { error: 'direction must be "minimize" or "maximize"' }),
    min_gain: z
      .number({ error: 'min_gain must be a number (a fraction, e.g. 0.03 for 3%)' })
      .min(0, 'min_gain must be >= 0')
      .max(1, 'min_gain is a fraction and must be <= 1 (0.03 means 3%)'),
  }),
  bench: z.strictObject({
    ref: nonEmpty('bench.ref'),
    run: nonEmpty('bench.run'),
    repeats: z
      .number({ error: 'repeats must be an integer' })
      .int('repeats must be an integer')
      .min(1, 'repeats must be >= 1'),
    hardware: nonEmpty('bench.hardware'),
    confirm_trace: z.string({ error: 'confirm_trace must be a string' }).min(1).optional(),
  }),
  guardrails: z.array(guardrailSchema, { error: 'guardrails must be a list' }).default([]),
  protected: z
    .array(nonEmpty('protected glob'), { error: 'protected must be a list of globs' })
    .default([]),
  agent: z.strictObject({
    instructions: nonEmpty('agent.instructions'),
    allowed_instance_types: z
      .array(nonEmpty('instance type'), { error: 'allowed_instance_types must be a list of strings' })
      .min(1, 'allowed_instance_types must list at least one instance type'),
  }),
});

export type RatchetConfig = z.output<typeof configSchema>;
export type Direction = RatchetConfig['objective']['direction'];
