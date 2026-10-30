import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import type { ArtifactStore, GitHost } from '@ratchet/core';
import { DEPS_BASE_ARCHIVE, hashNamedFiles } from './deps.js';
import { BenchError } from './errors.js';
import { assertSafeRef, type StagedArchive } from './remote-scripts.js';

const execFileP = promisify(execFile);

/** Prefix in the artifacts bucket for staged inputs. Agents cannot touch it (boundary: `artifacts/<opt>/<agent>/` only); only the bench identities read it. */
export const STAGING_PREFIX = 'bench-inputs/';

const ID_RE = /^[a-z0-9-]{1,64}$/;
const BUCKET_RE = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/;

export interface StagedSource {
  code: StagedArchive;
  bench: StagedArchive;
}

interface StageBase {
  git: Pick<GitHost, 'checkout' | 'resolveRef'>;
  /** Writes into the artifacts bucket (the controller's own credentials; bench instances only read). */
  store: ArtifactStore;
  /** Name of that bucket, for the `s3://` uris the instance downloads. */
  bucket: string;
  optimizationId: string;
}

export interface StageInput extends StageBase {
  sha: string;
  benchRef: string;
  /**
   * Root-level file names (the dependency strategy's lockfiles) to hash in BOTH trees before archiving; the result is in
   * `StagedArchive.files`. The controller needs the candidate's, to block a lockfile change before the instance runs anything.
   */
  hashFiles?: readonly string[];
}

function validate(i: StageBase): void {
  // These end up in S3 keys and in IAM-scoped prefixes: plain ids only.
  if (!ID_RE.test(i.optimizationId)) throw new BenchError(`optimization id ${JSON.stringify(i.optimizationId)} must match [a-z0-9-] (it is used in an S3 prefix)`);
  if (!BUCKET_RE.test(i.bucket)) throw new BenchError(`staging bucket ${JSON.stringify(i.bucket)} is not a valid bucket name`);
}

/** Materialize `ref` with the GitHost, archive it as `<prefix><name>` and upload it. `.git` and history never leave the controller. */
async function stageOne(i: StageBase, work: string, prefix: string, what: string, ref: string, name: string, hashFiles?: readonly string[]): Promise<StagedArchive> {
  let commit: string;
  const tree = join(work, name);
  try {
    commit = await i.git.resolveRef(ref);
    assertSafeRef(commit, `${what} commit`);
    await i.git.checkout(commit, tree);
  } catch (err) {
    if (err instanceof BenchError) throw err;
    throw new BenchError(`cannot resolve ${what} ${ref}: ${(err as Error).message}`, { cause: err });
  }
  const files = hashFiles ? await hashNamedFiles(tree, hashFiles) : undefined;
  const file = join(work, `${name}.tgz`);
  try {
    await execFileP('tar', ['-czf', file, '-C', tree, '.']);
  } catch (err) {
    throw new BenchError(`cannot archive ${what} ${ref}: ${(err as Error).message}`, { cause: err });
  }
  const bytes = await readFile(file);
  const key = `${prefix}${name}.tgz`;
  await i.store.put(key, bytes);
  return { uri: `s3://${i.bucket}/${key}`, sha256: createHash('sha256').update(bytes).digest('hex'), commit, ...(files ? { files } : {}) };
}

/**
 * Pre-stages what a bench instance needs, so it never talks to GitHub: the candidate tree at `sha` and
 * the trusted tree at `bench.ref`, each as a gzip tarball under `bench-inputs/<opt>/<job>/` in the
 * artifacts bucket, with the sha256 the instance must see. Built on the controller from `GitHost.checkout`
 * (trees only; the candidate's history never reaches the instance).
 */
export async function stageSource(i: StageInput): Promise<StagedSource> {
  validate(i);
  assertSafeRef(i.sha, 'candidate sha');
  assertSafeRef(i.benchRef, 'bench.ref');

  const prefix = `${STAGING_PREFIX}${i.optimizationId}/${randomUUID()}/`;
  const work = await mkdtemp(join(tmpdir(), 'ratchet-stage-'));
  try {
    const code = await stageOne(i, work, prefix, 'candidate', i.sha, 'code', i.hashFiles);
    const bench = await stageOne(i, work, prefix, 'bench.ref', i.benchRef, 'bench', i.hashFiles);
    return { code, bench };
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}

/**
 * Stages ONLY the trusted base tree for the dependency-snapshot job, as `bench-inputs/<opt>/<job>/deps-base.tgz` (the one
 * basename the deps identity may read). The candidate is never staged for that job.
 */
export async function stageDepsBase(i: StageBase & { ref: string }): Promise<StagedArchive> {
  validate(i);
  assertSafeRef(i.ref, 'bench.ref');
  const prefix = `${STAGING_PREFIX}${i.optimizationId}/${randomUUID()}/`;
  const work = await mkdtemp(join(tmpdir(), 'ratchet-stage-'));
  try {
    return await stageOne(i, work, prefix, 'bench.ref', i.ref, DEPS_BASE_ARCHIVE.replace(/\.tgz$/, ''));
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}
