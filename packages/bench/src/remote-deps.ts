import { randomUUID } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ArtifactStore, BenchJob, Clock, CloudCompute, GitHost, RatchetConfig } from '@ratchet/core';
import {
  archFromInstanceType,
  depsKey,
  hashNamedFiles,
  pinnedDepsFiles,
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
import {
  imageKey,
  imageManifestKey,
  imageTag,
  parseImageManifest,
  readEnvironment,
  resolveImageSource,
  type ImageKeyInput,
  type ImageManifest,
  type ImagePlan,
  type ResolvedImage,
} from './image.js';
import { imageBuildScript, type ContainerRuntime, type ImageRestore } from './image-scripts.js';
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
  /** Limit for building the repo's container image. Default 45 minutes. */
  imageMs?: number | undefined;
  /** Container options of the runs (read-only root, pids limit, ...); the tag comes from the image. */
  container?: Omit<ContainerRuntime, 'tag'> | undefined;
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
  /** The repo's container (`environment.image`) as resolved at the base commit. */
  image?: ResolvedImage | undefined;
}

/** What the controller prepares before a job's instance exists: the dependency snapshot and/or the repo's container image. */
export interface PreparedEnvironment {
  deps?: DepsPlan | undefined;
  image?: ImagePlan | undefined;
}

interface BuildWhat {
  arch: string;
  image?: { key: string; expect: ImageKeyInput & { key: string } } | undefined;
  imageHave?: ImageManifest | undefined;
  imageKey?: string | undefined;
  deps?: { key: string } | undefined;
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
    return (await this.prepare(a)).deps;
  }

  /**
   * Everything the job needs from an instance WITH egress, prepared once and reused: the dependency snapshot (unless `wantDeps` is
   * false, i.e. the runner has no setup command) and the repo's container image (`environment.image`). Both are built from bench.ref
   * on ONE throwaway instance when either is missing; with the image present the snapshot's fetch runs inside it.
   */
  async prepare(a: EnsureInput & { wantDeps?: boolean }): Promise<PreparedEnvironment> {
    const { log, config } = a;
    const imagePath = readEnvironment(config).image;
    const base = await this.inspectBase(config, imagePath);
    if (base.note) log.line(`dependencies: ${base.note}`);
    const wantDeps = (a.wantDeps ?? true) && base.strategy.id !== 'none';
    if (!wantDeps && (a.wantDeps ?? true)) {
      log.line('dependencies: no lockfile to snapshot (or evaluator.deps is none); setup runs with whatever the image provides');
    }
    if (!wantDeps && !base.image) return {};
    const arch = archFromInstanceType(this.o.benchInstanceType ?? config.bench.hardware);

    // ---- the container image
    let imgKey: string | undefined;
    let imgExpect: (ImageKeyInput & { key: string }) | undefined;
    let haveImage: ImageManifest | undefined;
    if (base.image) {
      for (const n of base.image.spec.notes) log.line(`environment: ${n}`);
      const ki: ImageKeyInput = { spec: base.image.spec, files: base.image.files, arch };
      imgKey = imageKey(ki);
      imgExpect = { ...ki, key: imgKey };
      haveImage = await this.lookupImage(imgExpect, log);
      if (haveImage) log.line(`environment: reusing container image ${imgKey} (${base.image.spec.path} at ${base.commit.slice(0, 12)})`);
    }

    // ---- the dependency snapshot (its "image" is the container's key when there is one: the toolchain is part of what it was fetched with)
    let dKey: string | undefined;
    let dExpect: Parameters<typeof parseDepsManifest>[1] | undefined;
    let haveDeps: DepsManifest | undefined;
    if (wantDeps) {
      const input = { strategy: base.strategy, lockfiles: base.lockfiles, image: imgKey ?? this.o.imageRef, arch };
      dKey = depsKey(input);
      dExpect = { ...input, key: dKey };
      haveDeps = await this.lookup(dKey, dExpect, log);
      if (haveDeps) log.line(`dependencies: reusing snapshot ${dKey} (${base.strategy.lockfiles.filter((f) => f in base.lockfiles).join(', ')} at ${base.commit.slice(0, 12)})`);
    }

    const needImage = !!imgKey && !haveImage;
    const needDeps = !!dKey && !haveDeps;
    if (needImage || needDeps) {
      const buildKey = `${needImage ? imgKey : ''}|${needDeps ? dKey : ''}`;
      const what: BuildWhat = {
        arch,
        image: needImage ? { key: imgKey!, expect: imgExpect! } : undefined,
        imageHave: haveImage,
        imageKey: imgKey,
        deps: needDeps ? { key: dKey! } : undefined,
      };
      await this.buildOnce(buildKey, a.signal, () => this.build(a, base, what));
    }

    let image: ImagePlan | undefined;
    if (imgKey && base.image) {
      const made = haveImage ?? (await this.lookupImage(imgExpect!, log));
      if (!made) throw new BenchError(`the container image job for ${imgKey} finished but its manifest is missing or invalid in the artifacts bucket`);
      image = { key: imgKey, tag: imageTag(imgKey), spec: base.image.spec, files: base.image.files, pinned: Object.keys(base.image.files), baseCommit: base.commit, manifest: made, reused: !!haveImage };
    }
    let deps: DepsPlan | undefined;
    if (dKey) {
      const made = haveDeps ?? (await this.lookup(dKey, dExpect!, log));
      if (!made) throw new BenchError(`the dependency snapshot job for ${dKey} finished but its manifest is missing or invalid in the artifacts bucket`);
      deps = { strategy: base.strategy, key: dKey, baseCommit: base.commit, lockfiles: base.lockfiles, manifest: made, reused: !!haveDeps };
    }
    return { deps, image };
  }

  /** The manifest of the saved image for `key`, if it exists, matches what the controller computed, and its archive is still in the bucket. */
  private async lookupImage(expect: ImageKeyInput & { key: string }, log: BenchLog): Promise<ImageManifest | undefined> {
    const store = this.o.store;
    if (!store) throw new BenchError('container images need an artifacts store (stagingStore or artifacts): the bench has no network');
    let text: string | null;
    try {
      text = await store.get(imageManifestKey(expect.key));
    } catch (err) {
      throw new BenchError(`cannot read ${imageManifestKey(expect.key)}: ${(err as Error).message}`, { cause: err });
    }
    if (text === null) return undefined;
    const check = parseImageManifest(text, expect);
    if (!check.ok) {
      log.line(`environment: ignoring ${imageManifestKey(expect.key)}: ${check.error}`);
      return undefined;
    }
    const present = await store.list(check.manifest.archive).catch(() => [] as string[]);
    if (!present.includes(check.manifest.archive)) {
      log.line(`environment: ${check.manifest.archive} is gone from the bucket; rebuilding`);
      return undefined;
    }
    return check.manifest;
  }

  /** Strategy, lockfile hashes and (with `environment.image`) the resolved container of bench.ref's tree. Cached per commit (a commit never changes). */
  private inspectBase(config: RatchetConfig, imagePath?: string): Promise<BaseInfo> {
    const { git } = this.o;
    const mode = config.evaluator.deps;
    return (async () => {
      let commit: string;
      try {
        commit = await git.resolveRef(config.bench.ref);
      } catch (err) {
        throw new BenchError(`cannot resolve bench.ref ${config.bench.ref}: ${(err as Error).message}`, { cause: err });
      }
      const cacheKey = `${commit}\0${mode}\0${imagePath ?? ''}`;
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
            const lockfiles = await hashNamedFiles(tree, pinnedDepsFiles(resolved.strategy));
            const image = imagePath ? await resolveImageSource(tree, imagePath) : undefined;
            return { commit, strategy: resolved.strategy, lockfiles, note: resolved.note, image };
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

  private async build(a: EnsureInput, base: BaseInfo, w: BuildWhat): Promise<void> {
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
      throw new BenchError(`the dependency snapshot job runs on ${depsType} but the bench on ${benchType}: different architectures would fetch different native packages and images`);
    }
    if (signal?.aborted) throw new BenchAbortedError();
    a.onProgress?.({ step: 'deps' });
    const what = [w.image ? `container image ${w.image.key}` : '', w.deps ? `dependency snapshot ${w.deps.key}` : ''].filter(Boolean).join(' + ');
    log.section(what);
    log.line(`building from bench.ref ${config.bench.ref} (${base.commit}); the candidate is not involved`);

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
      const workDir = (kind: string): string => `${this.o.workRoot}/${kind}-${job.optimizationId.replace(/[^A-Za-z0-9._-]/g, '_')}-${randomUUID().slice(0, 8)}`;

      let imageManifest = w.imageHave;
      if (w.image && base.image) {
        const imageMs = this.o.imageMs ?? 45 * 60_000;
        const res = await session.call(
          'image',
          imageBuildScript({
            work: workDir('image'),
            base: staged,
            key: w.image.key,
            tag: imageTag(w.image.key),
            spec: base.image.spec,
            files: base.image.files,
            arch: w.arch,
            bucket,
            timeoutMs: imageMs,
          }),
          imageMs + 10 * 60_000,
        );
        const done = parseSentinels(res.stdout).find((x) => x.kind === 'image');
        if (res.exitCode !== 0 || !done) {
          throw new BenchError(
            `building the container image from ${base.image.spec.path} failed (exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}. ` +
              'The image job runs in the agent subnet: the registries the Dockerfile pulls from must be reachable through the egress proxy / DNS allow-list (docs/real-env/aws.md)',
          );
        }
        log.line(`container image built: ${String(done.data.bytes)} bytes (${String(done.data.compression)}) in ${String(done.data.ms)}ms`);
        imageManifest = await this.lookupImage(w.image.expect, log);
        if (!imageManifest) throw new BenchError(`the container image job for ${w.image.key} finished but its manifest is missing or invalid in the artifacts bucket`);
      }

      if (w.deps) {
        const container: ImageRestore | undefined = imageManifest
          ? { uri: `s3://${bucket}/${imageManifest.archive}`, sha256: imageManifest.sha256, compression: imageManifest.compression, imageId: imageManifest.imageId, rt: { ...this.o.container, tag: imageManifest.tag } }
          : undefined;
        const res = await session.call(
          'deps',
          depsBuildScript({
            layout: { work: workDir('deps') },
            strategy: base.strategy,
            base: staged,
            key: w.deps.key,
            lockfiles: base.lockfiles,
            image: w.imageKey ?? this.o.imageRef,
            arch: w.arch,
            bucket,
            timeoutMs: this.o.depsMs,
            isolation: this.o.isolation,
            ...(container ? { container } : {}),
          }),
          this.o.depsMs + 5 * 60_000,
        );
        const done = parseSentinels(res.stdout).find((x) => x.kind === 'deps');
        if (res.exitCode !== 0 || !done) {
          throw new BenchError(
            `building the ${base.strategy.id} dependency snapshot failed (exit ${res.exitCode}): ${tail(res.stderr || res.stdout)}. ` +
              'The snapshot job runs in the agent subnet and needs the package registry through the egress proxy / DNS allow-list (docs/real-env/aws.md)',
          );
        }
        log.line(`dependency snapshot built: ${String(done.data.bytes)} bytes (${String(done.data.compression)}) in ${String(done.data.ms)}ms`);
      }
    } finally {
      await session.terminate();
    }
  }
}
