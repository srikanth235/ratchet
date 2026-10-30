import { z } from 'zod';
import { evaluatorTemplateIds, findEvaluatorTemplate } from '../templates/registry.js';

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

/** What kind of trusted evaluation produces the objective metric. Informational for now; packs set it. */
export const EVALUATOR_KINDS = ['benchmark', 'backtest', 'eval', 'simulation'] as const;
export type EvaluatorKind = (typeof EVALUATOR_KINDS)[number];

/**
 * How the candidate's dependencies reach the offline bench (`evaluator.deps`):
 *  - `auto` (default): detect from the lockfile at `evaluator.ref` (`pnpm-lock.yaml` -> pnpm, `package-lock.json` /
 *    `npm-shrinkwrap.json` -> npm, otherwise none);
 *  - `pnpm` / `npm`: that package manager; the lockfile must exist at `evaluator.ref`;
 *  - `none`: the bench installs nothing; dependencies are part of the machine image or not needed.
 * Whatever is chosen, dependency changes are not allowed in v0: a candidate whose lockfile differs from the one at
 * `evaluator.ref` is blocked, because the bench has no network.
 */
export const DEPS_MODES = ['auto', 'pnpm', 'npm', 'none'] as const;
export type DepsMode = (typeof DEPS_MODES)[number];

/** Bench machine used when neither `evaluator.hardware`, `environment.machine` nor a template names one. */
export const DEFAULT_BENCH_MACHINE = 'c7i.large';
/** Protected ref holding the trusted evaluator when `evaluator.ref` is omitted. */
export const DEFAULT_EVALUATOR_REF = 'ratchet/bench';

const evaluatorSchema = z
  .strictObject({
    /** Defaults to the template's kind, else `benchmark`. */
    kind: z.enum(EVALUATOR_KINDS, { error: `evaluator.kind must be one of ${EVALUATOR_KINDS.join(', ')}` }).optional(),
    /** Protected ref holding the trusted evaluator. Default `ratchet/bench`. */
    ref: nonEmpty('evaluator.ref').optional(),
    /** The evaluation command. Required unless `template` supplies it; when both are present `run` wins. */
    run: nonEmpty('evaluator.run').optional(),
    /** Id of an evaluator template (see `listTemplates()`): fills `run`, `kind`, the machine and default guardrails. */
    template: z
      .string({ error: 'evaluator.template must be a template id (e.g. http-latency)' })
      .regex(/^[a-z][a-z0-9-]*$/, 'evaluator.template must be a template id such as http-latency')
      .optional(),
    /**
     * Runs per candidate. Optional: when absent Ratchet calibrates it from the baseline's coefficient of
     * variation (`calibrate`). A set value is used as-is, and is also the baseline's run count.
     */
    repeats: z
      .number({ error: 'repeats must be an integer' })
      .int('repeats must be an integer')
      .min(1, 'repeats must be >= 1')
      .optional(),
    /** Multiplier k of the noise floor `k x stddev(baseline)`. Optional (default 2). */
    noise_factor: z
      .number({ error: 'noise_factor must be a number (the k of noise floor = k x stddev, default 2)' })
      .min(0, 'noise_factor must be >= 0')
      .max(10, 'noise_factor must be <= 10')
      .optional(),
    /** Pinned bench hardware. Default `environment.machine`, else the template's machine. */
    hardware: nonEmpty('evaluator.hardware').optional(),
    /** Package-manager strategy for the offline bench's dependencies. See `DEPS_MODES`. */
    deps: z.enum(DEPS_MODES, { error: `evaluator.deps must be one of ${DEPS_MODES.join(', ')}` }).default('auto'),
    /** Sealed holdout the agents never see: hidden traffic trace, most recent unseen period, held-out partition. */
    holdout: z.string({ error: 'holdout must be a string' }).min(1).optional(),
    /** Deprecated alias of `holdout`. */
    confirm_trace: z.string({ error: 'confirm_trace must be a string' }).min(1).optional(),
  })
  .transform(({ holdout, confirm_trace, ...rest }) => {
    const trace = holdout ?? confirm_trace;
    return { ...rest, ...(trace ? { confirm_trace: trace, holdout: trace } : {}) };
  });

/**
 * How a metric change converts to money (the business case): `usd_per_unit` dollars per
 * one unit of improvement in `objective.metric`, per `period`. `basis: estimated` until it
 * has been measured in production.
 */
const valueSchema = z.strictObject({
  usd_per_unit: z.number({ error: 'value.usd_per_unit must be a number (USD per one unit of metric improvement)' }).positive('value.usd_per_unit must be > 0'),
  period: z.enum(['month', 'year'], { error: 'value.period must be "month" or "year"' }).default('year'),
  basis: z.enum(['estimated', 'measured'], { error: 'value.basis must be "estimated" or "measured"' }).default('estimated'),
  unit: z.string().min(1).optional(),
  note: z.string().min(1).optional(),
});

/** A repo-relative POSIX path: no leading `/`, no `..` segments, no backslashes. */
const repoPath = (what: string) =>
  z
    .string({ error: `${what} must be a path inside the repository` })
    .min(1, `${what} must not be empty`)
    .refine((p) => !p.startsWith('/') && !/^[A-Za-z]:/.test(p), `${what} must be relative to the repository root`)
    .refine((p) => !p.includes('\\'), `${what} must use forward slashes`)
    .refine((p) => !p.split('/').includes('..'), `${what} must stay inside the repository (no "..")`);

/** An egress host: `registry.npmjs.org` or `*.example.com`. No scheme, path or port; a bare `*` is not allowed. */
const HOST_RE = /^(?:\*\.)?[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+$/;

/**
 * `environment:` (all optional). What the repo needs; how it is isolated stays with Ratchet.
 *  - `image`: Dockerfile or devcontainer.json in the repo; the toolchain for setup, guardrails and the evaluator.
 *    It is built from the protected ref, so it counts as a protected path.
 *  - `machine`: bench instance type (default: the template's, else c7i.large).
 *  - `region`: AWS region (default us-east-1, chosen by the app).
 *  - `network`: extra egress hosts for agents / the dependency job.
 */
const environmentSchema = z.strictObject({
  image: repoPath('environment.image').optional(),
  machine: z
    .string({ error: 'environment.machine must be an instance type such as c7i.2xlarge' })
    .regex(/^[a-z][a-z0-9-]*\.[a-z0-9-]+$/, 'environment.machine must be an instance type such as c7i.2xlarge')
    .optional(),
  region: z
    .string({ error: 'environment.region must be a region such as us-east-1' })
    .regex(/^[a-z]{2}(?:-[a-z]+)+-\d+$/, 'environment.region must be an AWS region such as us-east-1')
    .optional(),
  network: z
    .array(
      z
        .string({ error: 'environment.network entries must be host names' })
        .regex(HOST_RE, 'environment.network entries must be host names such as registry.npmjs.org (no scheme, path or port; "*." wildcards are allowed)'),
      { error: 'environment.network must be a list of host names' },
    )
    .optional(),
});

const rawConfigSchema = z.strictObject({
  version: z.literal(0, { error: 'version must be 0 (the only supported ratchet.yaml version)' }),
  /** Domain pack this config follows (e.g. perf, forecasting). Informational. */
  pack: z.string().min(1).optional(),
  objective: z.strictObject({
    name: z.string().min(1).optional(),
    metric: nonEmpty('objective.metric'),
    direction: z.enum(['minimize', 'maximize'], { error: 'direction must be "minimize" or "maximize"' }),
    /** Fraction of the best that a win must beat. Optional: when absent it is calibrated from the baseline noise. */
    min_gain: z
      .number({ error: 'min_gain must be a number (a fraction, e.g. 0.03 for 3%)' })
      .min(0, 'min_gain must be >= 0')
      .max(1, 'min_gain is a fraction and must be <= 1 (0.03 means 3%)')
      .optional(),
    value: valueSchema.optional(),
  }),
  evaluator: evaluatorSchema,
  environment: environmentSchema.optional(),
  /**
   * Advanced: path (in the repo) of a custom OpenTofu module for the sandbox. Read ONLY from the protected ref and
   * validated against the sandbox contract (infra/contract.md) by the engine / bench, never taken from a candidate's
   * tree. Off by default; the module that ships with the app is used when this is absent.
   */
  infra: repoPath('infra').optional(),
  /** Absent: the template's default guardrails (when `evaluator.template` is set), else none. */
  guardrails: z.array(guardrailSchema, { error: 'guardrails must be a list' }).optional(),
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

/**
 * `ratchet.yaml` schema. The trusted evaluation lives under `evaluator:`; the older
 * `bench:` key is accepted as an alias. The parsed config exposes it as both
 * `evaluator` and `bench` (same object) so existing code keeps working.
 *
 * Resolution after validation, so the rest of the system sees a fully populated evaluator:
 *  - `evaluator.run` <- the template's command when `run` is absent (an unknown template id is an error);
 *  - `evaluator.kind` <- the template's kind, else `benchmark`;
 *  - `evaluator.ref` <- `ratchet/bench`; `evaluator.hardware` <- `environment.machine`, else the template's machine, else c7i.large;
 *  - `guardrails` <- the template's defaults when the list is absent.
 * `evaluator.repeats` and `objective.min_gain` stay optional in the output: absent means "calibrate from the baseline".
 */
export const configSchema = z
  .preprocess((input) => {
    if (input && typeof input === 'object' && !Array.isArray(input)) {
      const o = input as Record<string, unknown>;
      if ('bench' in o && !('evaluator' in o)) {
        const { bench, ...rest } = o;
        return { ...rest, evaluator: bench };
      }
    }
    return input;
  }, rawConfigSchema)
  .transform((c, ctx) => {
    const tpl = c.evaluator.template ? findEvaluatorTemplate(c.evaluator.template) : undefined;
    if (c.evaluator.template && !tpl) {
      ctx.issues.push({
        code: 'custom',
        message: `unknown evaluator template ${JSON.stringify(c.evaluator.template)} (available: ${evaluatorTemplateIds().join(', ')})`,
        input: c.evaluator.template,
        path: ['evaluator', 'template'],
      });
    }
    const run = c.evaluator.run ?? tpl?.run;
    if (!run && !c.evaluator.template) {
      ctx.issues.push({
        code: 'custom',
        message: 'is required: give the evaluation command as `run`, or pick an evaluator `template`',
        input: c.evaluator,
        path: ['evaluator', 'run'],
      });
    }
    const guardrails =
      c.guardrails ?? (tpl ? tpl.guardrails.map((g) => guardrailSchema.parse(g)) : []);
    const evaluator = {
      ...c.evaluator,
      kind: c.evaluator.kind ?? tpl?.evaluatorKind ?? ('benchmark' as EvaluatorKind),
      ref: c.evaluator.ref ?? DEFAULT_EVALUATOR_REF,
      run: run ?? '',
      hardware: c.evaluator.hardware ?? c.environment?.machine ?? tpl?.machine ?? DEFAULT_BENCH_MACHINE,
    };
    return { ...c, guardrails, protected: c.protected, evaluator, bench: evaluator };
  });

export type RatchetConfig = z.output<typeof configSchema>;
export type Direction = RatchetConfig['objective']['direction'];
