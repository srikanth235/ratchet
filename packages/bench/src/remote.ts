import { randomUUID } from 'node:crypto';
import type {
  ArtifactStore,
  BenchJob,
  BenchOutcome,
  CloudCompute,
  Clock,
  GitHost,
  InstanceInfo,
  RatchetConfig,
  RemoteCommandResult,
} from '@ratchet/core';
import { BenchAbortedError, BenchError } from './errors.js';
import type { BenchLog } from './log.js';
import {
  DEFAULT_TIMEOUTS,
  runBenchJob,
  type BenchProgress,
  type PipelineExecutor,
  type ProgressBenchRunner,
  type StepOutcome,
  type StepSpec,
  type StepTimeouts,
} from './pipeline.js';
import {
  checkoutScript,
  localTracePath,
  parseSentinels,
  pingScript,
  stepScript,
  stripSentinels,
  traceScript,
  wrapStep,
  type Layout,
} from './remote-scripts.js';

export interface RemoteBenchRunnerOptions {
  compute: CloudCompute;
  /** Only `remoteUrl()` is used: the instance fetches the candidate itself. */
  git: Pick<GitHost, 'remoteUrl'>;
  clock: Clock;
  /** AMI / image reference of the bench instance (git, tar, GNU coreutils + grep -P, bash; `aws` CLI for s3:// traces). */
  imageRef: string;
  /** Override `git.remoteUrl()` (e.g. a URL with a short-lived token). Beware: scripts are visible in SSM history. */
  repoUrl?: string;
  setup?: string;
  artifacts?: ArtifactStore;
  /** Default: `config.bench.hardware`. */
  instanceType?: string;
  /** Per-job working directories are created under here on the instance. Default `/var/tmp/ratchet-bench`. */
  workRoot?: string;
  timeouts?: Partial<StepTimeouts>;
  commandGuardrailsInConfirm?: boolean;
  pruneExtraProtected?: boolean;
  /** The first command is retried while the instance's SSM agent registers. Defaults: 30 attempts, 10 s apart. */
  readyAttempts?: number;
  readyDelayMs?: number;
  /** Called with non-fatal problems, e.g. a failed `terminate` (the reaper is the backstop). */
  onWarning?: (message: string) => void;
}

/**
 * Live-mode bench runner. Per job it launches a fresh bench instance (so a confirm run is on a
 * fresh instance by construction), runs the same pipeline as LocalBenchRunner by sending one
 * generated script per step through `CloudCompute.runCommand`, and ALWAYS terminates the instance
 * in a `finally`. See remote-scripts.ts for the scripts and the sentinel protocol.
 *
 * Steps: ping (wait for SSM) -> checkout (fetch sha + bench.ref, git archive both, overlay) ->
 * [trace download for confirm] -> setup -> guardrails -> bench x N. Aborting stops waiting
 * immediately and terminates the instance, which kills whatever is running there.
 */
export class RemoteBenchRunner implements ProgressBenchRunner {
  constructor(private readonly o: RemoteBenchRunnerOptions) {}

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
        const instance = await this.o.compute.launchBenchInstance({
          optimizationId: job.optimizationId,
          instanceType: this.o.instanceType ?? config.bench.hardware,
          imageRef: this.o.imageRef,
          purpose: job.phase,
        });
        log.line(`bench instance ${instance.id} (${instance.instanceType}) launched for ${job.phase}`);
        return new RemoteExecutor(this.o, job, config, log, instance, signal);
      },
    );
  }
}

class RemoteExecutor implements PipelineExecutor {
  private readonly layout: Layout;
  private readonly timeouts: StepTimeouts;
  /** Set when the trace was downloaded to the instance. */
  private tracePath: string | undefined;

  constructor(
    private readonly o: RemoteBenchRunnerOptions,
    private readonly job: BenchJob,
    private readonly config: RatchetConfig,
    private readonly log: BenchLog,
    private readonly instance: InstanceInfo,
    private readonly signal: AbortSignal | undefined,
  ) {
    const id = `${job.optimizationId.replace(/[^A-Za-z0-9._-]/g, '_')}-${randomUUID().slice(0, 8)}`;
    this.layout = { work: `${o.workRoot ?? '/var/tmp/ratchet-bench'}/${id}` };
    this.timeouts = { ...DEFAULT_TIMEOUTS, ...o.timeouts };
  }

  async checkout(opts: { trace?: string }): Promise<void> {
    await this.waitReady();
    const res = await this.call(
      'checkout',
      checkoutScript({
        layout: this.layout,
        repoUrl: this.o.repoUrl ?? this.o.git.remoteUrl(),
        sha: this.job.sha,
        benchRef: this.config.bench.ref,
        globs: this.config.protected,
        prune: this.o.pruneExtraProtected ?? true,
      }),
      this.timeouts.checkoutMs,
    );
    const s = parseSentinels(res.stdout).find((x) => x.kind === 'checkout');
    if (res.exitCode !== 0 || !s) {
      throw new BenchError(`checkout on the bench instance failed (exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}`);
    }
    const copied = Number(s.data.copied);
    if (!(copied > 0)) {
      throw new BenchError(
        `bench.ref ${this.config.bench.ref} contains no files matching the protected globs [${this.config.protected.join(', ')}]; ` +
          'refusing to benchmark with an unprotected bench',
      );
    }
    this.log.line(`overlay: ${copied} protected file(s) from ${this.config.bench.ref}, ${String(s.data.removed)} candidate-only protected file(s) removed`);

    if (opts.trace !== undefined && opts.trace.startsWith('s3://')) {
      const localPath = localTracePath(this.layout, opts.trace);
      const t = await this.call('trace', traceScript({ layout: this.layout, uri: opts.trace, localPath }), this.timeouts.checkoutMs);
      if (t.exitCode !== 0) throw new BenchError(`downloading the confirm trace failed (exit ${t.exitCode}): ${tail(t.stderr || t.stdout)}`);
      this.tracePath = localPath;
    }
  }

  async runStep(spec: StepSpec): Promise<StepOutcome> {
    const env = { ...spec.env };
    if (spec.trace !== undefined) env.RATCHET_TRACE = this.tracePath ?? spec.trace;
    this.log.section(spec.name);
    const res = await this.call(
      spec.name,
      stepScript({ layout: this.layout, kind: spec.kind, name: spec.name, command: spec.command, timeoutMs: spec.timeoutMs, env }),
      spec.timeoutMs,
    );
    const sentinels = parseSentinels(res.stdout);
    const tampered = sentinels.find((s) => s.kind === 'tampered');
    if (tampered) {
      const paths = Buffer.from(String(tampered.data.paths_b64 ?? ''), 'base64').toString('utf8').split('\n').filter(Boolean);
      this.log.line(`!!! protected files were modified and restored: ${paths.join(', ')}`);
      return { exitCode: null, timedOut: false, stdout: '', durationMs: 0, tampered: paths.length > 0 ? paths : ['(unknown)'] };
    }
    const r = sentinels.find((s) => s.kind === (spec.kind === 'bench' ? 'run' : 'step'));
    if (!r) {
      return {
        exitCode: null,
        timedOut: false,
        stdout: '',
        durationMs: 0,
        error: `no result from the bench instance (script exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}`,
      };
    }
    const exitCode = typeof r.data.exit === 'number' ? r.data.exit : null;
    const stdout = typeof r.data.stdout_b64 === 'string' ? Buffer.from(r.data.stdout_b64, 'base64').toString('utf8') : '';
    if (stdout) this.log.line(`[bench stdout tail]\n${stdout}`);
    this.log.line(`[exit ${exitCode}${r.data.timedOut === true ? ', timed out' : ''} after ${String(r.data.ms)}ms]`);
    return { exitCode, timedOut: r.data.timedOut === true, stdout, durationMs: Number(r.data.ms) || 0 };
  }

  async dispose(): Promise<void> {
    try {
      await this.o.compute.terminate([this.instance.id]);
      this.log.line(`bench instance ${this.instance.id} terminated`);
    } catch (err) {
      const msg = `FAILED to terminate bench instance ${this.instance.id}: ${(err as Error).message} (the reaper must clean it up)`;
      this.log.line(msg);
      this.o.onWarning?.(msg);
    }
  }

  private async waitReady(): Promise<void> {
    const attempts = this.o.readyAttempts ?? 30;
    for (let i = 1; ; i++) {
      try {
        const r = await this.call('ping', pingScript(), 60_000);
        if (r.exitCode === 0) return;
        throw new Error(`exit ${r.exitCode}`);
      } catch (err) {
        if (err instanceof BenchAbortedError) throw err;
        if (i >= attempts) throw new BenchError(`bench instance ${this.instance.id} never became ready: ${(err as Error).message}`);
        await this.o.clock.sleep(this.o.readyDelayMs ?? 10_000, this.signal);
      }
    }
  }

  /** One runCommand, raced against the abort signal; the output (minus sentinel lines) goes to the log. */
  private async call(name: string, script: string, timeoutMs: number): Promise<RemoteCommandResult> {
    if (this.signal?.aborted) throw new BenchAbortedError();
    const pending = this.o.compute.runCommand(this.instance.id, {
      script: wrapStep(script),
      timeoutSeconds: Math.ceil(timeoutMs / 1000) + 120,
    });
    pending.catch(() => undefined);
    let onAbort: (() => void) | undefined;
    const aborted = new Promise<never>((_, reject) => {
      if (!this.signal) return;
      onAbort = () => reject(new BenchAbortedError());
      this.signal.addEventListener('abort', onAbort, { once: true });
    });
    let res: RemoteCommandResult;
    try {
      res = await Promise.race([pending, aborted]);
    } catch (err) {
      if (err instanceof BenchAbortedError) throw err;
      throw new BenchError(`runCommand "${name}" on ${this.instance.id} failed: ${(err as Error).message}`, { cause: err });
    } finally {
      if (onAbort) this.signal?.removeEventListener('abort', onAbort);
    }
    if (name !== 'ping') {
      this.log.section(`remote step: ${name} (exit ${res.exitCode})`);
      this.log.line(stripSentinels(res.stdout));
      if (res.stderr.trim()) this.log.line(`[stderr]\n${res.stderr}`);
    }
    return res;
  }
}

function tail(s: string, n = 600): string {
  const t = s.trim();
  return t.length > n ? '...' + t.slice(-n) : t;
}
