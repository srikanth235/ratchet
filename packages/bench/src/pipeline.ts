import {
  evaluateThreshold,
  extractLastJsonObject,
  guardrailName,
  lookupPath,
  type ArtifactStore,
  type BenchJob,
  type BenchOutcome,
  type BenchRun,
  type BenchRunner,
  type Clock,
  type CommandGuardrail,
  type GuardrailResult,
  type RatchetConfig,
  type ThresholdGuardrail,
} from '@ratchet/core';
import { BenchAbortedError, BenchError } from './errors.js';
import { flattenNumeric, medianMetrics, resolveMetric } from './flatten.js';
import { BenchLog } from './log.js';

/**
 * The bench pipeline, shared by LocalBenchRunner and RemoteBenchRunner so both run the same steps:
 *
 *   checkout (+ protected overlay) -> setup -> command guardrails (fail fast) -> bench x N
 *   -> threshold guardrails over the per-metric medians of the N runs
 *
 * Only the `PipelineExecutor` differs (local child processes vs. SSM scripts on a bench instance).
 */

export interface StepTimeouts {
  checkoutMs: number;
  setupMs: number;
  guardrailMs: number;
  /** Per single `bench.run` invocation. */
  benchMs: number;
}

export const DEFAULT_TIMEOUTS: StepTimeouts = {
  checkoutMs: 10 * 60_000,
  setupMs: 30 * 60_000,
  guardrailMs: 15 * 60_000,
  benchMs: 30 * 60_000,
};

export type BenchProgress =
  | { step: 'checkout' }
  | { step: 'setup' }
  | { step: 'guardrails'; index: number; total: number; name: string }
  | { step: 'bench'; run: number; of: number; phase: BenchJob['phase'] }
  | { step: 'evaluate' };

/** A BenchRunner that also reports step progress (the queue turns it into UI states). */
export interface ProgressBenchRunner extends BenchRunner {
  run(
    job: BenchJob,
    config: RatchetConfig,
    baseline: { metrics: Record<string, number> } | null,
    signal?: AbortSignal,
    onProgress?: (p: BenchProgress) => void,
  ): Promise<BenchOutcome>;
}

export interface StepSpec {
  kind: 'setup' | 'guardrail' | 'bench';
  name: string;
  command: string;
  timeoutMs: number;
  /** Extra environment (RATCHET_* variables). */
  env: Record<string, string>;
  /** Confirm bench steps only: the hidden trace reference; the executor exports it as `RATCHET_TRACE`. */
  trace?: string;
}

export interface StepOutcome {
  exitCode: number | null;
  timedOut: boolean;
  /** (Tail of the) stdout; the bench JSON is parsed from here. */
  stdout: string;
  durationMs: number;
  /** Set when trusted files were modified by an earlier step (restored; the step was NOT run). */
  tampered?: string[];
  /** Set when the step could not be started. */
  error?: string;
}

export interface PipelineExecutor {
  /** Materialize the candidate, overlay the protected files from bench.ref, fetch the trace if any. */
  checkout(opts: { trace?: string }): Promise<void>;
  /** Run one step. Throws BenchAbortedError when aborted. Must verify/restore protected files first. */
  runStep(spec: StepSpec): Promise<StepOutcome>;
  dispose(): Promise<void>;
}

export interface PipelineContext {
  job: BenchJob;
  config: RatchetConfig;
  baseline: { metrics: Record<string, number> } | null;
  exec: PipelineExecutor;
  log: BenchLog;
  clock: Clock;
  setup?: string | undefined;
  timeouts: StepTimeouts;
  /** Run command guardrails in the confirm phase too (default false: they passed in the bench phase). */
  commandGuardrailsInConfirm: boolean;
  signal?: AbortSignal | undefined;
  onProgress?: ((p: BenchProgress) => void) | undefined;
}

export interface PipelineResult {
  runs: BenchRun[];
  guardrails: GuardrailResult[];
}

export function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) throw new BenchAbortedError();
}

/** Parse one `bench.run` stdout: the last JSON object; objective value by dotted path; all numeric leaves as metrics. */
export function parseBenchOutput(
  stdout: string,
  metricPath: string,
): { ok: true; value: number; metrics: Record<string, number> } | { ok: false; error: string } {
  const obj = extractLastJsonObject(stdout);
  if (!obj) return { ok: false, error: 'no JSON object found on stdout' };
  const v = lookupPath(obj, metricPath);
  if (v === undefined) return { ok: false, error: `objective metric "${metricPath}" not found in JSON output` };
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    return { ok: false, error: `objective metric "${metricPath}" is not a finite number (got ${JSON.stringify(v)})` };
  }
  const metrics = flattenNumeric(obj);
  metrics[metricPath] = v;
  return { ok: true, value: v, metrics };
}

/** Threshold guardrails over the per-metric medians of `runs`, relative bounds against `baseline.metrics`. */
export function evaluateThresholds(
  guardrails: readonly ThresholdGuardrail[],
  runs: readonly BenchRun[],
  baseline: { metrics: Record<string, number> } | null,
  phase: BenchJob['phase'],
): GuardrailResult[] {
  const medians = medianMetrics(runs);
  return guardrails.map((g): GuardrailResult => {
    if (g.relative && phase === 'baseline') {
      return { name: guardrailName(g), passed: true, detail: 'skipped: relative bounds are measured against this baseline run' };
    }
    const observed = resolveMetric(medians, g.metric);
    if (!observed) {
      return {
        name: guardrailName(g),
        passed: false,
        detail: `metric "${g.metric}" was not reported by the benchmark (reported: ${Object.keys(medians).join(', ') || 'nothing'})`,
      };
    }
    const base = baseline ? resolveMetric(baseline.metrics, g.metric) : undefined;
    const res = evaluateThreshold(g, observed.value, base?.value);
    return observed.key === g.metric ? res : { ...res, detail: res.detail?.replace(g.metric, observed.key) };
  });
}

export function commandGuardrailResult(g: CommandGuardrail, out: StepOutcome, timeoutMs: number): GuardrailResult {
  if (out.timedOut) return { name: g.run, passed: false, detail: `timed out after ${timeoutMs}ms` };
  if (out.error) return { name: g.run, passed: false, detail: out.error };
  if (out.exitCode === null) return { name: g.run, passed: false, detail: 'terminated by a signal' };
  return { name: g.run, passed: out.exitCode === 0, ...(out.exitCode === 0 ? {} : { detail: `exited with status ${out.exitCode}` }) };
}

function tamperedGuardrail(paths: string[]): GuardrailResult {
  const shown = paths.slice(0, 5).join(', ') + (paths.length > 5 ? `, +${paths.length - 5} more` : '');
  return {
    name: 'protected files intact',
    passed: false,
    detail: `trusted files were modified by the candidate's setup/tests/bench and restored from bench.ref: ${shown}`,
  };
}

export async function runPipeline(ctx: PipelineContext): Promise<PipelineResult> {
  const { job, config, exec, log } = ctx;
  const guardrails: GuardrailResult[] = [];
  const runs: BenchRun[] = [];
  const baseEnv = {
    RATCHET_PHASE: job.phase,
    RATCHET_SHA: job.sha,
    RATCHET_OPTIMIZATION: job.optimizationId,
    RATCHET_REPEATS: String(job.repeats),
  };
  const emit = (p: BenchProgress) => ctx.onProgress?.(p);

  const trace = job.phase === 'confirm' ? (job.trace ?? config.bench.confirm_trace) : undefined;
  if (job.phase === 'confirm' && trace === undefined) {
    throw new BenchError('confirm job has no trace: set job.trace or bench.confirm_trace in ratchet.yaml');
  }
  if (!Number.isInteger(job.repeats) || job.repeats < 1) throw new BenchError(`job.repeats must be a positive integer (got ${job.repeats})`);

  throwIfAborted(ctx.signal);
  emit({ step: 'checkout' });
  log.section(`checkout ${job.sha} (bench from ${config.bench.ref})`);
  await exec.checkout(trace === undefined ? {} : { trace });

  /** Run a step; returns false (and records the failure) when trusted files were tampered with. */
  const guarded = async (spec: StepSpec): Promise<StepOutcome | undefined> => {
    throwIfAborted(ctx.signal);
    const out = await exec.runStep(spec);
    if (out.tampered && out.tampered.length > 0) {
      guardrails.push(tamperedGuardrail(out.tampered));
      return undefined;
    }
    return out;
  };

  if (ctx.setup) {
    emit({ step: 'setup' });
    const out = await guarded({ kind: 'setup', name: 'setup', command: ctx.setup, timeoutMs: ctx.timeouts.setupMs, env: baseEnv });
    if (!out) return { runs, guardrails };
    const failed = out.timedOut || out.error !== undefined || out.exitCode !== 0;
    if (failed) {
      guardrails.push({
        name: 'setup',
        passed: false,
        detail: out.timedOut ? `timed out after ${ctx.timeouts.setupMs}ms` : (out.error ?? `exited with status ${out.exitCode}`),
      });
      return { runs, guardrails };
    }
  }

  if (job.phase !== 'confirm' || ctx.commandGuardrailsInConfirm) {
    const commands = config.guardrails.filter((g): g is CommandGuardrail => g.kind === 'command');
    for (const [i, g] of commands.entries()) {
      emit({ step: 'guardrails', index: i + 1, total: commands.length, name: g.run });
      const out = await guarded({ kind: 'guardrail', name: `guardrail ${i + 1}/${commands.length}: ${g.run}`, command: g.run, timeoutMs: ctx.timeouts.guardrailMs, env: baseEnv });
      if (!out) return { runs, guardrails };
      const res = commandGuardrailResult(g, out, ctx.timeouts.guardrailMs);
      guardrails.push(res);
      if (!res.passed) {
        log.line(`guardrail failed (${res.detail ?? 'failed'}); the benchmark will not run`);
        return { runs, guardrails }; // fail fast: blocked before spending bench time
      }
    }
  }

  let benchFailed = false;
  for (let i = 1; i <= job.repeats; i++) {
    emit({ step: 'bench', run: i, of: job.repeats, phase: job.phase });
    const out = await guarded({
      kind: 'bench',
      name: `bench run ${i}/${job.repeats}`,
      command: config.bench.run,
      timeoutMs: ctx.timeouts.benchMs,
      env: { ...baseEnv, RATCHET_RUN: String(i) },
      ...(trace === undefined ? {} : { trace }),
    });
    if (!out) return { runs, guardrails };
    const fail = (detail: string) => {
      guardrails.push({ name: 'bench.run', passed: false, detail: `run ${i}/${job.repeats}: ${detail}` });
      benchFailed = true;
    };
    if (out.timedOut) fail(`timed out after ${ctx.timeouts.benchMs}ms`);
    else if (out.error) fail(out.error);
    else if (out.exitCode !== 0) fail(out.exitCode === null ? 'terminated by a signal' : `exited with status ${out.exitCode}`);
    else {
      const parsed = parseBenchOutput(out.stdout, config.objective.metric);
      if (!parsed.ok) fail(parsed.error);
      else runs.push({ value: parsed.value, metrics: parsed.metrics, durationMs: out.durationMs });
    }
    if (benchFailed) break;
  }

  if (!benchFailed) {
    emit({ step: 'evaluate' });
    guardrails.push(
      ...evaluateThresholds(
        config.guardrails.filter((g): g is ThresholdGuardrail => g.kind === 'threshold'),
        runs,
        ctx.baseline,
        job.phase,
      ),
    );
  }
  return { runs, guardrails };
}

/** Log key: `bench/<optimization>/<candidate|baseline>/<phase>.log`. */
export function logKey(job: BenchJob): string {
  const seg = (s: string) => s.replace(/[^A-Za-z0-9._-]/g, '_');
  const who = job.phase === 'baseline' ? 'baseline' : seg(job.candidateId ?? job.sha);
  return `bench/${seg(job.optimizationId)}/${who}/${job.phase}.log`;
}

/** Race `p` against a timeout and an abort signal (for calls that cannot be cancelled, like GitHost.checkout). */
export async function withLimits<T>(p: Promise<T>, what: string, timeoutMs: number, signal?: AbortSignal): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  let onAbort: (() => void) | undefined;
  const guard = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new BenchError(`${what} timed out after ${timeoutMs}ms`)), timeoutMs);
    if (signal) {
      onAbort = () => reject(new BenchAbortedError());
      if (signal.aborted) onAbort();
      else signal.addEventListener('abort', onAbort, { once: true });
    }
  });
  p.catch(() => undefined); // the loser of the race must not become an unhandled rejection
  try {
    return await Promise.race([p, guard]);
  } finally {
    if (timer) clearTimeout(timer);
    if (signal && onAbort) signal.removeEventListener('abort', onAbort);
  }
}

export interface JobDeps {
  clock: Clock;
  artifacts?: ArtifactStore | undefined;
  setup?: string | undefined;
  timeouts: StepTimeouts;
  commandGuardrailsInConfirm: boolean;
}

/**
 * Run one job end to end: build the executor, run the pipeline, always dispose the executor and
 * (best effort) store the combined log, then assemble the BenchOutcome.
 * Throws BenchAbortedError when `signal` aborts and BenchError for infrastructure failures.
 */
export async function runBenchJob(
  deps: JobDeps,
  job: BenchJob,
  config: RatchetConfig,
  baseline: { metrics: Record<string, number> } | null,
  signal: AbortSignal | undefined,
  onProgress: ((p: BenchProgress) => void) | undefined,
  makeExecutor: (log: BenchLog) => Promise<PipelineExecutor> | PipelineExecutor,
): Promise<BenchOutcome> {
  const startedAt = deps.clock.now();
  const log = new BenchLog(deps.clock);
  log.line(`bench job: optimization=${job.optimizationId} phase=${job.phase} sha=${job.sha} repeats=${job.repeats}` + (job.candidateId ? ` candidate=${job.candidateId}` : ''));
  let exec: PipelineExecutor | undefined;
  let result: PipelineResult | undefined;
  let failure: unknown;
  try {
    throwIfAborted(signal);
    exec = await makeExecutor(log);
    result = await runPipeline({
      job,
      config,
      baseline,
      exec,
      log,
      clock: deps.clock,
      setup: deps.setup,
      timeouts: deps.timeouts,
      commandGuardrailsInConfirm: deps.commandGuardrailsInConfirm,
      signal,
      onProgress,
    });
  } catch (err) {
    failure = err;
    log.line(`\n!!! ${err instanceof Error ? `${err.name}: ${err.message}` : String(err)}`);
  } finally {
    try {
      await exec?.dispose();
    } catch (err) {
      log.line(`dispose failed: ${(err as Error).message}`);
    }
  }
  let logsKey: string | undefined;
  if (deps.artifacts) {
    try {
      await deps.artifacts.put(logKey(job), log.toString());
      logsKey = logKey(job);
    } catch {
      // logs are best effort
    }
  }
  if (failure !== undefined || !result) throw failure ?? new BenchError('bench pipeline produced no result');
  return {
    runs: result.runs,
    samples: result.runs.map((r) => r.value),
    guardrails: result.guardrails,
    ...(logsKey ? { logsKey } : {}),
    startedAt,
    finishedAt: deps.clock.now(),
  };
}
