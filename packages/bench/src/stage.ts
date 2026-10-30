import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import type { ArtifactStore, GitHost } from '@ratchet/core';
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

export interface StageInput {
  git: Pick<GitHost, 'checkout' | 'resolveRef'>;
  /** Writes into the artifacts bucket (the controller's own credentials; bench instances only read). */
  store: ArtifactStore;
  /** Name of that bucket, for the `s3://` uris the instance downloads. */
  bucket: string;
  optimizationId: string;
  sha: string;
  benchRef: string;
}

/**
 * Pre-stages what a bench instance needs, so it never talks to GitHub: the candidate tree at `sha` and
 * the trusted tree at `bench.ref`, each as a gzip tarball under `bench-inputs/<opt>/<job>/` in the
 * artifacts bucket, with the sha256 the instance must see. Built on the controller from `GitHost.checkout`
 * (trees only; the candidate's history never reaches the instance).
 */
export async function stageSource(i: StageInput): Promise<StagedSource> {
  // These end up in S3 keys and in IAM-scoped prefixes: plain ids only.
  if (!ID_RE.test(i.optimizationId)) throw new BenchError(`optimization id ${JSON.stringify(i.optimizationId)} must match [a-z0-9-] (it is used in an S3 prefix)`);
  if (!BUCKET_RE.test(i.bucket)) throw new BenchError(`staging bucket ${JSON.stringify(i.bucket)} is not a valid bucket name`);
  assertSafeRef(i.sha, 'candidate sha');
  assertSafeRef(i.benchRef, 'bench.ref');

  const job = randomUUID();
  const prefix = `${STAGING_PREFIX}${i.optimizationId}/${job}/`;
  const work = await mkdtemp(join(tmpdir(), 'ratchet-stage-'));
  try {
    const one = async (what: string, ref: string, name: string): Promise<StagedArchive> => {
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
      const file = join(work, `${name}.tgz`);
      try {
        await execFileP('tar', ['-czf', file, '-C', tree, '.']);
      } catch (err) {
        throw new BenchError(`cannot archive ${what} ${ref}: ${(err as Error).message}`, { cause: err });
      }
      const bytes = await readFile(file);
      const key = `${prefix}${name}.tgz`;
      await i.store.put(key, bytes);
      return { uri: `s3://${i.bucket}/${key}`, sha256: createHash('sha256').update(bytes).digest('hex'), commit };
    };
    const code = await one('candidate', i.sha, 'code');
    const bench = await one('bench.ref', i.benchRef, 'bench');
    return { code, bench };
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}
