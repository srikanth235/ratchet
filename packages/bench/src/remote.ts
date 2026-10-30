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
} from '@ratchet/core';
import { dependencyChangeGuardrail, lockfileChanges, offlineSetupHint, type DepsPlan } from './deps.js';
import { BenchError } from './errors.js';
import type { BenchLog } from './log.js';
import {
  DEFAULT_TIMEOUTS,
  runBenchJob,
  type BenchProgress,
  type CheckoutVeto,
  type PipelineExecutor,
  type ProgressBenchRunner,
  type StepOutcome,
  type StepSpec,
  type StepTimeouts,
} from './pipeline.js';
import { DepsSnapshots } from './remote-deps.js';
import {
  checkoutScript,
  depsPaths,
  HOLDOUT_FETCH_EXIT,
  parseSentinels,
  stepScript,
  type DepsRestore,
  type Isolation,
  type Layout,
} from './remote-scripts.js';
import { InstanceSession, tail } from './remote-session.js';
import { stageSource, type StagedSource } from './stage.js';

export interface RemoteBenchRunnerOptions {
  compute: CloudCompute;
  /**
   * Used on the controller to materialize the candidate and `bench.ref` before the job ("pre-staging"):
   * bench instances have no route to GitHub and never fetch from it.
   */
  git: Pick<GitHost, 'checkout' | 'resolveRef'>;
  clock: Clock;
  /** AMI / image reference of the bench instance (tar, GNU coreutils + findutils + grep -P, bash, setpriv, iptables; `aws` CLI for s3:// downloads; see docs/real-env/bench.md). */
  imageRef: string;
  /** Name of the artifacts bucket (`artifacts_bucket` output): staged inputs are uploaded under `bench-inputs/` and downloaded from there by the instance. Required to run a job. */
  stagingBucket?: string;
  /** Where staged inputs are written. Default: `artifacts`, which must then write to `stagingBucket` (e.g. `S3ArtifactStore`). */
  stagingStore?: ArtifactStore;
  /**
   * `user` (default): setup and guardrails run as the unprivileged candidate user (`ratchet-bench`); each bench
   * run executes the evaluator harness as a second unprivileged user (`ratchet-holdout`) from a read-only snapshot
   * of the built tree, and the harness starts candidate processes through the root-installed launcher. Instance
   * metadata is blocked for both users, and the job is refused if the image lacks the users, the block, the
   * launcher or the holdout directory permissions. `none` runs everything as root and is only for tests that
   * execute the scripts on a developer machine.
   */
  isolation?: Isolation;
  /** Path of the candidate launcher on the image. Default `/usr/local/bin/ratchet-candidate` (installed by the bench user-data). Test seam. */
  candidateLauncher?: string;
  /**
   * Candidate setup, e.g. `pnpm install --frozen-lockfile && pnpm build`. The bench has no network, so when it is set the
   * dependencies come from a SNAPSHOT of the lockfile at `bench.ref` (see deps.ts and `evaluator.deps`): built once per
   * lockfile by a `deps` instance with egress before the first job, restored read-only before setup, and setup runs with the
   * package manager offline. A candidate whose lockfile differs is blocked. Without `setup` nothing is snapshotted.
   */
  setup?: string;
  artifacts?: ArtifactStore;
  /** The bench instance type. Default: `config.bench.hardware`. */
  instanceType?: string;
  /** Instance type of the dependency-snapshot job (must have the bench's architecture). Default: the bench's. */
  depsInstanceType?: string;
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
 * Steps: [dependency snapshot, first job per lockfile only: a separate `deps` instance with egress builds the package store
 * from bench.ref's lockfile; see remote-deps.ts] -> stage on the controller (candidate + bench.ref tarballs into the artifacts
 * bucket; a changed lockfile blocks the candidate here) -> ping (wait for SSM) -> checkout (download from S3, verify digest,
 * overlay, check the isolation, restore the snapshot read-only) -> setup (offline) -> guardrails (both as the candidate
 * user) -> bench x N (the harness as the holdout user). For a confirm job the sealed holdout is fetched by root inside each
 * bench run, after the build is over, and wiped when the run ends: nothing candidate-controlled ever runs while it exists as
 * a file the candidate user could open. Aborting stops waiting immediately and terminates the instance, which kills whatever
 * is running there.
 */
export class RemoteBenchRunner implements ProgressBenchRunner {
  private readonly snapshots: DepsSnapshots;

  constructor(private readonly o: RemoteBenchRunnerOptions) {
    this.snapshots = new DepsSnapshots({
      compute: o.compute,
      git: o.git,
      clock: o.clock,
      imageRef: o.imageRef,
      bucket: o.stagingBucket,
      store: o.stagingStore ?? o.artifacts,
      isolation: o.isolation ?? 'user',
      workRoot: o.workRoot ?? '/var/tmp/ratchet-bench',
      depsMs: { ...DEFAULT_TIMEOUTS, ...o.timeouts }.depsMs,
      readyAttempts: o.readyAttempts,
      readyDelayMs: o.readyDelayMs,
      onWarning: o.onWarning,
      benchInstanceType: o.instanceType,
      instanceType: o.depsInstanceType,
    });
  }

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
        // Before the first job of a lockfile (the baseline, in practice) a separate instance with egress fetches the
        // packages; it is gone again before this job's instance exists. Without setup there is nothing to install.
        const deps = this.o.setup ? await this.snapshots.ensure({ job, config, log, signal, onProgress }) : undefined;
        const instance = await this.o.compute.launchBenchInstance({
          optimizationId: job.optimizationId,
          instanceType: this.o.instanceType ?? config.bench.hardware,
          imageRef: this.o.imageRef,
          purpose: job.phase,
        });
        log.line(`bench instance ${instance.id} (${instance.instanceType}) launched for ${job.phase}`);
        return new RemoteExecutor(this.o, job, config, log, instance, signal, deps);
      },
    );
  }
}

class RemoteExecutor implements PipelineExecutor {
  private readonly layout: Layout;
  private readonly timeouts: StepTimeouts;
  private readonly session: InstanceSession;

  constructor(
    private readonly o: RemoteBenchRunnerOptions,
    private readonly job: BenchJob,
    private readonly config: RatchetConfig,
    private readonly log: BenchLog,
    instance: InstanceInfo,
    signal: AbortSignal | undefined,
    private readonly deps: DepsPlan | undefined,
  ) {
    const id = `${job.optimizationId.replace(/[^A-Za-z0-9._-]/g, '_')}-${randomUUID().slice(0, 8)}`;
    this.layout = { work: `${o.workRoot ?? '/var/tmp/ratchet-bench'}/${id}` };
    this.timeouts = { ...DEFAULT_TIMEOUTS, ...o.timeouts };
    this.session = new InstanceSession(o, instance, log, signal);
  }

  private get isolation(): Isolation {
    return this.o.isolation ?? 'user';
  }

  private async stage(): Promise<StagedSource> {
    const store = this.o.stagingStore ?? this.o.artifacts;
    if (!this.o.stagingBucket || !store) {
      throw new BenchError('RemoteBenchRunner needs stagingBucket (and an artifacts/stagingStore that writes to it): bench instances have no internet and cannot fetch the candidate themselves');
    }
    const staged = await stageSource({
      git: this.o.git,
      store,
      bucket: this.o.stagingBucket,
      optimizationId: this.job.optimizationId,
      sha: this.job.sha,
      // With a snapshot, the bench tree is the exact commit the snapshot's lockfile hashes were computed from.
      benchRef: this.deps?.baseCommit ?? this.config.bench.ref,
      ...(this.deps ? { hashFiles: this.deps.strategy.lockfiles } : {}),
    });
    this.log.line(`staged candidate ${staged.code.commit} and bench.ref ${staged.bench.commit} for the instance (no GitHub access from the bench)`);
    return staged;
  }

  /** What the checkout script restores before setup, from the manifest the snapshot job wrote. */
  private restore(plan: DepsPlan): DepsRestore {
    const bucket = this.o.stagingBucket!;
    return {
      strategy: plan.strategy,
      uri: `s3://${bucket}/${plan.manifest.archive}`,
      sha256: plan.manifest.sha256,
      compression: plan.manifest.compression,
      node: plan.manifest.node,
      pm: plan.manifest.pm,
    };
  }

  async checkout(opts: { trace?: string }): Promise<CheckoutVeto | void> {
    // Staging overlaps with the instance booting (it was launched before checkout started).
    const staged = await this.stage();

    // Dependencies cannot change in v0: the bench has no network. Decided here, on the controller, from the trees it just
    // archived, so a candidate that touches the lockfile never gets as far as running anything.
    if (this.deps) {
      const base = lockfileChanges(this.deps.lockfiles, staged.bench.files ?? {});
      if (base.length > 0) {
        throw new BenchError(`bench.ref ${this.config.bench.ref} moved while the job was starting: ${base.join(', ')} no longer matches the dependency snapshot ${this.deps.key}`);
      }
      const changed = lockfileChanges(this.deps.lockfiles, staged.code.files ?? {});
      if (changed.length > 0) {
        this.log.line(`!!! dependency change: ${changed.join(', ')} differs from bench.ref ${this.config.bench.ref}`);
        return { guardrail: dependencyChangeGuardrail(changed, this.config.bench.ref) };
      }
    }

    await this.session.waitReady();
    const res = await this.session.call(
      'checkout',
      checkoutScript({
        layout: this.layout,
        code: staged.code,
        bench: staged.bench,
        globs: this.config.protected,
        ...(this.deps ? { pinned: this.deps.strategy.lockfiles, deps: this.restore(this.deps) } : {}),
        prune: this.o.pruneExtraProtected ?? true,
        isolation: this.isolation,
        ...(this.o.candidateLauncher ? { candidateLauncher: this.o.candidateLauncher } : {}),
      }),
      this.timeouts.checkoutMs,
    );
    const s = parseSentinels(res.stdout).find((x) => x.kind === 'checkout');
    if (res.exitCode !== 0 || !s) {
      throw new BenchError(`checkout on the bench instance failed (exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}`);
    }
    const matched = Number(s.data.matched ?? s.data.copied);
    if (!(matched > 0)) {
      throw new BenchError(
        `bench.ref ${this.config.bench.ref} contains no files matching the protected globs [${this.config.protected.join(', ')}]; ` +
          'refusing to benchmark with an unprotected bench',
      );
    }
    this.log.line(`overlay: ${String(s.data.copied)} protected file(s) from ${this.config.bench.ref}, ${String(s.data.removed)} candidate-only protected file(s) removed`);
    if (this.deps) this.log.line(`dependencies restored read-only from snapshot ${this.deps.key}; setup runs offline`);

    // The holdout is NOT fetched here: setup and guardrails (candidate code) run before the first bench
    // step, and nothing sealed may exist on the instance while they do. Each bench step fetches it.
    if (opts.trace !== undefined) this.log.line('holdout is fetched by each bench run, after setup and guardrails, and wiped when the run ends');
  }

  setupFailureHint(): string | undefined {
    return this.deps ? offlineSetupHint(this.deps.strategy, this.config.bench.ref) : undefined;
  }

  async runStep(spec: StepSpec): Promise<StepOutcome> {
    this.log.section(spec.name);
    // Setup and the command guardrails (candidate code, as the candidate user) install from the restored store with the
    // package manager offline. The evaluator harness does not.
    const depsEnv =
      this.deps && spec.kind !== 'bench' ? this.deps.strategy.setupEnv(depsPaths(this.layout).store, depsPaths(this.layout).scratch) : {};
    const res = await this.session.call(
      spec.name,
      stepScript({
        layout: this.layout,
        kind: spec.kind,
        name: spec.name,
        command: spec.command,
        timeoutMs: spec.timeoutMs,
        env: { ...spec.env, ...depsEnv },
        isolation: this.isolation,
        ...(spec.trace !== undefined ? { holdout: { uri: spec.trace } } : {}),
        ...(this.o.candidateLauncher ? { candidateLauncher: this.o.candidateLauncher } : {}),
      }),
      spec.timeoutMs,
    );
    const sentinels = parseSentinels(res.stdout);
    if (spec.trace !== undefined && (Object.values(HOLDOUT_FETCH_EXIT) as number[]).includes(res.exitCode)) {
      throw new BenchError(`fetching the holdout on the bench instance failed (exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}`);
    }
    const held = sentinels.find((x) => x.kind === 'holdout');
    if (held) this.log.line(`holdout: ${String(held.data.bytes)} bytes fetched for this run, readable by the evaluator user only, wiped afterwards`);
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
    // A holdout run's output tail is not written to the stored log: it is the only free-text channel from
    // the confirm run to whoever reads logs. The parsed numbers still travel in the outcome.
    if (stdout && spec.trace === undefined) this.log.line(`[bench stdout tail]\n${stdout}`);
    this.log.line(`[exit ${exitCode}${r.data.timedOut === true ? ', timed out' : ''} after ${String(r.data.ms)}ms]`);
    return { exitCode, timedOut: r.data.timedOut === true, stdout, durationMs: Number(r.data.ms) || 0 };
  }

  async dispose(): Promise<void> {
    await this.session.terminate();
  }
}

