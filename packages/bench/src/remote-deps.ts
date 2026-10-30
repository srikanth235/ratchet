import { randomUUID } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ArtifactStore, BenchJob, Clock, CloudCompute, GitHost, RatchetConfig } from '@ratchet/core';
import {
  archFromInstanceType,
  depsKey,
  hashNamedFiles,
  manifestKey,
  parseDepsManifest,
  resolveDepsStrategy,
  rootEntries,
  type DepsManifest,
  type DepsPlan,
  type DepsStrategy,
} from './deps.js';
import { depsBuildScript } from './deps-scripts.js';
import { BenchAbortedError, BenchError } from './errors.js';
import type { BenchLog } from './log.js';
import type { BenchProgress } from './pipeline.js';
import { InstanceSession, tail, type SessionOptions } from './remote-session.js';
import { parseSentinels, type Isolation } from './remote-scripts.js';
import { stageDepsBase } from './stage.js';

export interface DepsSnapshotOptions extends SessionOptions {
  compute: CloudCompute;
  git: Pick<GitHost, 'checkout' | 'resolveRef'>;
  clock: Clock;
  /** The evaluator image. It is part of the snapshot key and is also what the dependency job boots. */
  imageRef: string;
  /** Artifacts bucket and a store that writes to it (the controller's credentials): staging, and reading manifests. */
  bucket: string | undefined;
  store: ArtifactStore | undefined;
  isolation: Isolation;
  /** Per-job working directories are created under here on the instance. */
  workRoot: string;
  /** Limit for the package-manager fetch. */
  depsMs: number;
  /** Instance type the bench runs on when it is not `evaluator.hardware` (RemoteBenchRunnerOptions.instanceType): its architecture is part of the key. */
  benchInstanceType?: string | undefined;
  /** Instance type of the snapshot job; must have the architecture of the bench. Default: the bench's. */
  instanceType?: string | undefined;
}

export interface EnsureInput {
  job: BenchJob;
  config: RatchetConfig;
  log: BenchLog;
  signal?: AbortSignal | undefined;
  onProgress?: ((p: BenchProgress) => void) | undefined;
}

interface BaseInfo {
  commit: string;
  strategy: DepsStrategy;
  lockfiles: Record<string, string>;
  note?: string | undefined;
}

/**
 * The orchestration half of dependency snapshots (RemoteBenchRunner runs it before the first job of a lockfile, i.e. before
 * the baseline; later jobs only look the manifest up).
 *
 *   resolve bench.ref -> strategy + lockfile hashes of the BASE tree (never the candidate) -> key
 *     -> `bench-deps/<key>.json` valid and its archive present?  yes: reuse
 *     -> no: stage the base tree, launch a `deps` instance (agent subnet, egress), run the build script, ALWAYS terminate
 *            the instance in a `finally`, then re-read and re-validate the manifest the job wrote.
 *
 * The deps instance carries the same `ratchet:opt` tag as every bench machine, so the reaper and the teardown orphan check
 * (`listTagged`) cover it exactly like a bench instance that a crash left behind. Concurrent jobs that need the same key share
 * one build.
 */
export class DepsSnapshots {
  private readonly bases = new Map<string, Promise<BaseInfo>>();
  private readonly building = new Map<string, Promise<void>>();

  constructor(private readonly o: DepsSnapshotOptions) {}

  /** The plan for this job, or undefined when there is nothing to install from a snapshot (no lockfile, `deps: none`). */
  async ensure(a: EnsureInput): Promise<DepsPlan | undefined> {
    const { log, config } = a;
    const base = await this.inspectBase(config);
    if (base.note) log.line(`dependencies: ${base.note}`);
    if (base.strategy.id === 'none') {
      log.line('dependencies: no lockfile to snapshot (or evaluator.deps is none); setup runs with whatever the image provides');
      return undefined;
    }
    const arch = archFromInstanceType(this.o.benchInstanceType ?? config.bench.hardware);
    const input = { strategy: base.strategy, lockfiles: base.lockfiles, image: this.o.imageRef, arch };
    const key = depsKey(input);
    const expect = { ...input, key };
    const plan = (manifest: DepsManifest, reused: boolean): DepsPlan => ({ strategy: base.strategy, key, baseCommit: base.commit, lockfiles: base.lockfiles, manifest, reused });

    const have = await this.lookup(key, expect, log);
    if (have) {
      log.line(`dependencies: reusing snapshot ${key} (${base.strategy.lockfiles.filter((f) => f in base.lockfiles).join(', ')} at ${base.commit.slice(0, 12)})`);
      return plan(have, true);
    }
    await this.buildOnce(key, a.signal, () => this.build(a, base, key, arch));
    const made = await this.lookup(key, expect, log);
    if (!made) throw new BenchError(`the dependency snapshot job for ${key} finished but its manifest is missing or invalid in the artifacts bucket`);
    return plan(made, false);
  }

  /** Strategy and lockfile hashes of bench.ref's tree. Cached per commit (a commit never changes). */
  private inspectBase(config: RatchetConfig): Promise<BaseInfo> {
    const { git } = this.o;
    const mode = config.evaluator.deps;
    return (async () => {
      let commit: string;
      try {
        commit = await git.resolveRef(config.bench.ref);
      } catch (err) {
        throw new BenchError(`cannot resolve bench.ref ${config.bench.ref}: ${(err as Error).message}`, { cause: err });
      }
      const cacheKey = `${commit}\0${mode}`;
      let p = this.bases.get(cacheKey);
      if (!p) {
        p = (async () => {
          const dir = await mkdtemp(join(tmpdir(), 'ratchet-deps-base-'));
          try {
            const tree = join(dir, 'base');
            try {
              await git.checkout(commit, tree);
            } catch (err) {
              throw new BenchError(`cannot check out bench.ref ${config.bench.ref}: ${(err as Error).message}`, { cause: err });
            }
            const resolved = resolveDepsStrategy(mode, await rootEntries(tree));
            const lockfiles = await hashNamedFiles(tree, resolved.strategy.lockfiles);
            return { commit, strategy: resolved.strategy, lockfiles, note: resolved.note };
          } finally {
            await rm(dir, { recursive: true, force: true });
          }
        })();
        p.catch(() => this.bases.delete(cacheKey));
        this.bases.set(cacheKey, p);
      }
      return p;
    })();
  }

  /** The manifest for `key`, if it exists, matches what the controller computed, and its archive is still in the bucket. */
  private async lookup(key: string, expect: Parameters<typeof parseDepsManifest>[1], log: BenchLog): Promise<DepsManifest | undefined> {
    const store = this.o.store;
    if (!store) throw new BenchError('dependency snapshots need an artifacts store (stagingStore or artifacts): the bench has no network');
    let text: string | null;
    try {
      text = await store.get(manifestKey(key));
    } catch (err) {
      throw new BenchError(`cannot read ${manifestKey(key)}: ${(err as Error).message}`, { cause: err });
    }
    if (text === null) return undefined;
    const check = parseDepsManifest(text, expect);
    if (!check.ok) {
      log.line(`dependencies: ignoring ${manifestKey(key)}: ${check.error}`);
      return undefined;
    }
    // Lifecycle rules can expire the archive a little before its manifest: do not hand out a manifest without its bytes.
    const present = await store.list(check.manifest.archive).catch(() => [] as string[]);
    if (!present.includes(check.manifest.archive)) {
      log.line(`dependencies: ${check.manifest.archive} is gone from the bucket; rebuilding`);
      return undefined;
    }
    return check.manifest;
  }

  private async buildOnce(key: string, signal: AbortSignal | undefined, run: () => Promise<void>): Promise<void> {
    for (;;) {
      const inflight = this.building.get(key);
      if (!inflight) break;
      try {
        await inflight;
        return;
      } catch (err) {
        // The job that was building was cancelled: that is not this job's failure, build it ourselves.
        if (err instanceof BenchAbortedError && !signal?.aborted) continue;
        throw err;
      }
    }
    const p: Promise<void> = run().finally(() => {
      if (this.building.get(key) === p) this.building.delete(key);
    });
    this.building.set(key, p);
    await p;
  }

  private async build(a: EnsureInput, base: BaseInfo, key: string, arch: string): Promise<void> {
    const { job, config, log, signal } = a;
    const { compute } = this.o;
    const bucket = this.o.bucket;
    const store = this.o.store;
    if (!bucket || !store) {
      throw new BenchError('RemoteBenchRunner needs stagingBucket (and an artifacts/stagingStore that writes to it): bench instances have no internet and cannot fetch dependencies themselves');
    }
    const benchType = this.o.benchInstanceType ?? config.bench.hardware;
    const depsType = this.o.instanceType ?? benchType;
    if (archFromInstanceType(depsType) !== archFromInstanceType(benchType)) {
      throw new BenchError(`the dependency snapshot job runs on ${depsType} but the bench on ${benchType}: different architectures would fetch different native packages`);
    }
    if (signal?.aborted) throw new BenchAbortedError();
    a.onProgress?.({ step: 'deps' });
    log.section(`dependency snapshot ${key}`);
    log.line(`building from ${base.strategy.lockfiles.filter((f) => f in base.lockfiles).join(', ')} at bench.ref ${config.bench.ref} (${base.commit}); the candidate is not involved`);

    const staged = await stageDepsBase({ git: this.o.git, store, bucket, optimizationId: job.optimizationId, ref: base.commit });
    let instance;
    try {
      instance = await compute.launchBenchInstance({ optimizationId: job.optimizationId, instanceType: depsType, imageRef: this.o.imageRef, purpose: 'deps' });
    } catch (err) {
      throw new BenchError(`cannot launch the dependency snapshot instance: ${(err as Error).message}`, { cause: err });
    }
    log.line(`dependency snapshot instance ${instance.id} (${depsType}) launched`);
    const session = new InstanceSession(this.o, instance, log, signal, 'dependency snapshot');
    try {
      await session.waitReady();
      const layout = { work: `${this.o.workRoot}/deps-${job.optimizationId.replace(/[^A-Za-z0-9._-]/g, '_')}-${randomUUID().slice(0, 8)}` };
      const res = await session.call(
        'deps',
        depsBuildScript({
          layout,
          strategy: base.strategy,
          base: staged,
          key,
          lockfiles: base.lockfiles,
          image: this.o.imageRef,
          arch,
          bucket,
          timeoutMs: this.o.depsMs,
          isolation: this.o.isolation,
        }),
        this.o.depsMs + 5 * 60_000,
      );
      const done = parseSentinels(res.stdout).find((s) => s.kind === 'deps');
      if (res.exitCode !== 0 || !done) {
        throw new BenchError(
          `building the ${base.strategy.id} dependency snapshot failed (exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}. ` +
            'The snapshot job runs in the agent subnet and needs the package registry through the egress proxy / DNS allow-list (docs/real-env/aws.md)',
        );
      }
      log.line(`dependency snapshot built: ${String(done.data.bytes)} bytes (${String(done.data.compression)}) in ${String(done.data.ms)}ms`);
    } finally {
      await session.terminate();
    }
  }
}
