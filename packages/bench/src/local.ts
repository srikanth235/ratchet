import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import type {
  ArtifactStore,
  BenchJob,
  BenchOutcome,
  BenchRunner,
  Clock,
  GitHost,
  RatchetConfig,
} from '@ratchet/core';
import { BenchAbortedError, BenchError } from './errors.js';
import { runProcess } from './exec.js';
import type { BenchLog } from './log.js';
import { applyOverlay, restoreProtected } from './overlay.js';
import {
  DEFAULT_TIMEOUTS,
  runBenchJob,
  withLimits,
  type BenchProgress,
  type PipelineExecutor,
  type StepOutcome,
  type StepSpec,
  type StepTimeouts,
} from './pipeline.js';

export interface LocalBenchRunnerOptions {
  /** Used to materialize `job.sha` and `config.bench.ref` (any ref `GitHost.checkout` can resolve). */
  git: GitHost;
  /** Fresh job directories are created under here and deleted afterwards. */
  scratchRoot: string;
  clock: Clock;
  /** Run in the candidate checkout before the guardrails, e.g. `pnpm install --frozen-lockfile && pnpm build`. */
  setup?: string;
  /** Combined logs go to `bench/<optimization>/<candidate|baseline>/<phase>.log`. */
  artifacts?: ArtifactStore;
  timeouts?: Partial<StepTimeouts>;
  /** Extra environment for every step (merged over `process.env`). */
  env?: Record<string, string>;
  /** Run command guardrails in the confirm phase too. Default false (they passed in the bench phase). */
  commandGuardrailsInConfirm?: boolean;
  /** Delete candidate files that match `protected` but are absent from bench.ref. Default true. */
  pruneExtraProtected?: boolean;
  /** Keep the job directory (debugging). Default false. */
  keepWorkdir?: boolean;
}

const URL_LIKE = /^[a-z][a-z0-9+.-]*:\/\//i;

/**
 * Trusted-bench pipeline on this machine (simulation mode and `ratchet bench`). NOT a security
 * boundary: candidate code runs as the current user with the current environment.
 *
 * Per job: fresh directory under `scratchRoot` with `code/` (job.sha) and `bench/` (config.bench.ref);
 * `.git` is removed from both; every file of bench.ref matching a `protected` glob overwrites the
 * candidate's (see overlay.ts); then setup, command guardrails, `bench.run` x `job.repeats`,
 * threshold guardrails (see pipeline.ts). Trusted files are re-verified before every step.
 *
 * Step environment: `RATCHET_PHASE`, `RATCHET_SHA`, `RATCHET_OPTIMIZATION`, `RATCHET_REPEATS`,
 * `RATCHET_RUN` (bench runs, 1-based) and, for phase `confirm` bench runs only, `RATCHET_TRACE`:
 * the hidden trace (`job.trace`, else `bench.confirm_trace`). A relative local path is made absolute;
 * URLs (`s3://...`) are passed through unchanged, so the bench must be able to read them.
 */
export class LocalBenchRunner implements BenchRunner {
  constructor(private readonly o: LocalBenchRunnerOptions) {}

  async run(
    job: BenchJob,
    config: RatchetConfig,
    baseline: { metrics: Record<string, number> } | null,
    signal?: AbortSignal,
    onProgress?: (p: BenchProgress) => void,
  ): Promise<BenchOutcome> {
    return runBenchJob(
      {
        clock: this.o.clock,
        artifacts: this.o.artifacts,
        setup: this.o.setup,
        timeouts: { ...DEFAULT_TIMEOUTS, ...this.o.timeouts },
        commandGuardrailsInConfirm: this.o.commandGuardrailsInConfirm ?? false,
      },
      job,
      config,
      baseline,
      signal,
      onProgress,
      async (log) => {
        await mkdir(this.o.scratchRoot, { recursive: true });
        const root = await mkdtemp(join(this.o.scratchRoot, `${job.optimizationId.replace(/[^A-Za-z0-9._-]/g, '_')}-`));
        return new LocalExecutor(this.o, job, config, log, root, signal);
      },
    );
  }
}

class LocalExecutor implements PipelineExecutor {
  readonly codeDir: string;
  readonly benchDir: string;
  private protectedFiles: string[] = [];
  private readonly timeouts: StepTimeouts;

  constructor(
    private readonly o: LocalBenchRunnerOptions,
    private readonly job: BenchJob,
    private readonly config: RatchetConfig,
    private readonly log: BenchLog,
    private readonly root: string,
    private readonly signal: AbortSignal | undefined,
  ) {
    this.codeDir = join(root, 'code');
    this.benchDir = join(root, 'bench');
    this.timeouts = { ...DEFAULT_TIMEOUTS, ...o.timeouts };
  }

  async checkout(opts: { trace?: string }): Promise<void> {
    void opts;
    await mkdir(this.codeDir);
    await mkdir(this.benchDir);
    const { git } = this.o;
    const checkout = async (ref: string, dir: string, what: string) => {
      try {
        await withLimits(git.checkout(ref, dir), what, this.timeouts.checkoutMs, this.signal);
      } catch (err) {
        if (err instanceof BenchError || err instanceof BenchAbortedError) throw err;
        throw new BenchError(`${what} failed: ${(err as Error).message.split('\n')[0]}`, { cause: err });
      }
    };
    await checkout(this.job.sha, this.codeDir, `checkout of candidate ${this.job.sha}`);
    await checkout(this.config.bench.ref, this.benchDir, `checkout of bench.ref ${this.config.bench.ref}`);
    await rm(join(this.codeDir, '.git'), { recursive: true, force: true });
    await rm(join(this.benchDir, '.git'), { recursive: true, force: true });
    const res = await applyOverlay({
      codeDir: this.codeDir,
      benchDir: this.benchDir,
      globs: this.config.protected,
      prune: this.o.pruneExtraProtected ?? true,
    });
    if (res.copied.length === 0) {
      throw new BenchError(
        `bench.ref ${this.config.bench.ref} contains no files matching the protected globs [${this.config.protected.join(', ')}]; ` +
          'refusing to benchmark with an unprotected bench',
      );
    }
    this.protectedFiles = res.copied;
    this.log.line(`overlay: ${res.copied.length} protected file(s) taken from ${this.config.bench.ref}, ${res.removed.length} candidate-only protected file(s) removed`);
    for (const p of res.removed) this.log.line(`  removed ${p}`);
  }

  async runStep(spec: StepSpec): Promise<StepOutcome> {
    const restored = await restoreProtected({ codeDir: this.codeDir, benchDir: this.benchDir, files: this.protectedFiles });
    if (restored.length > 0) {
      this.log.line(`!!! protected files were modified and restored: ${restored.join(', ')}`);
      return { exitCode: null, timedOut: false, stdout: '', durationMs: 0, tampered: restored };
    }
    this.log.section(spec.name);
    const env: Record<string, string> = { ...this.o.env, ...spec.env };
    if (spec.trace !== undefined) {
      env.RATCHET_TRACE = URL_LIKE.test(spec.trace) || isAbsolute(spec.trace) ? spec.trace : resolve(spec.trace);
    }
    const r = await runProcess(spec.command, {
      cwd: this.codeDir,
      env,
      timeoutMs: spec.timeoutMs,
      ...(this.signal ? { signal: this.signal } : {}),
      onData: (chunk) => this.log.write(chunk),
    });
    if (r.aborted) throw new BenchAbortedError();
    this.log.line(
      r.spawnError
        ? `[could not start: ${r.spawnError}]`
        : `[exit ${r.exitCode ?? `signal ${r.signal}`}${r.timedOut ? ', timed out' : ''} after ${Math.round(r.durationMs)}ms]`,
    );
    return {
      exitCode: r.exitCode,
      timedOut: r.timedOut,
      stdout: r.stdout,
      durationMs: r.durationMs,
      ...(r.spawnError ? { error: `could not start: ${r.spawnError}` } : {}),
    };
  }

  async dispose(): Promise<void> {
    if (!this.o.keepWorkdir) await rm(this.root, { recursive: true, force: true });
  }
}
