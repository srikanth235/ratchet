import { randomBytes } from 'node:crypto';
import { mkdir, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve, sep } from 'node:path';
import { GetObjectCommand, ListObjectsV2Command, PutObjectCommand, S3Client } from '@aws-sdk/client-s3';
import type { ArtifactStore } from '@ratchet/core';
import { errName } from './util.js';

function assertKey(key: string): void {
  if (!key || key.startsWith('/') || key.includes('\0') || key.split('/').includes('..')) {
    throw new Error(`invalid artifact key ${JSON.stringify(key)}`);
  }
}

export interface S3ArtifactStoreOptions {
  bucket: string;
  region?: string;
  client?: S3Client;
  /** Named profile in ~/.aws/config (static, SSO or assume-role); default credential chain when unset. */
  profile?: string;
  /** Prepended to every key (hidden from callers). */
  prefix?: string;
}

export class S3ArtifactStore implements ArtifactStore {
  private readonly client: S3Client;
  private readonly prefix: string;

  constructor(private readonly opts: S3ArtifactStoreOptions) {
    this.client = opts.client ?? new S3Client({ region: opts.region, ...(opts.profile ? { profile: opts.profile } : {}) });
    this.prefix = opts.prefix ?? '';
  }

  async put(key: string, body: string | Uint8Array): Promise<void> {
    assertKey(key);
    await this.client.send(
      new PutObjectCommand({ Bucket: this.opts.bucket, Key: this.prefix + key, Body: body, ServerSideEncryption: 'AES256' }),
    );
  }

  async get(key: string): Promise<string | null> {
    assertKey(key);
    try {
      const r = await this.client.send(new GetObjectCommand({ Bucket: this.opts.bucket, Key: this.prefix + key }));
      return (await r.Body?.transformToString('utf-8')) ?? '';
    } catch (e) {
      if (errName(e) === 'NoSuchKey') return null;
      throw e;
    }
  }

  async list(prefix: string): Promise<string[]> {
    const keys: string[] = [];
    let token: string | undefined;
    do {
      const r = await this.client.send(
        new ListObjectsV2Command({ Bucket: this.opts.bucket, Prefix: this.prefix + prefix, ContinuationToken: token }),
      );
      for (const o of r.Contents ?? []) if (o.Key) keys.push(o.Key.slice(this.prefix.length));
      token = r.IsTruncated ? r.NextContinuationToken : undefined;
    } while (token);
    return keys.sort();
  }
}

/** Staging directory for atomic writes, inside the root (same filesystem, so rename is atomic); never listed. */
const FS_TMP = '.put-tmp';

/**
 * Local-directory ArtifactStore for simulation. Keys map to files under `root`. Writes are atomic (temp file + rename),
 * like S3 PUTs: readers polling event and inbox batches must never see a half-written object, or they would skip it.
 */
export class FsArtifactStore implements ArtifactStore {
  private readonly root: string;
  constructor(root: string) {
    this.root = resolve(root);
  }

  private path(key: string): string {
    assertKey(key);
    const p = resolve(this.root, key);
    if (!p.startsWith(this.root + sep)) throw new Error(`artifact key escapes store root: ${key}`);
    return p;
  }

  async put(key: string, body: string | Uint8Array): Promise<void> {
    const p = this.path(key);
    const tmpDir = join(this.root, FS_TMP);
    await Promise.all([mkdir(dirname(p), { recursive: true }), mkdir(tmpDir, { recursive: true })]);
    const tmp = join(tmpDir, randomBytes(8).toString('hex'));
    try {
      await writeFile(tmp, body);
      await rename(tmp, p);
    } catch (e) {
      await rm(tmp, { force: true });
      throw e;
    }
  }

  async get(key: string): Promise<string | null> {
    try {
      return await readFile(this.path(key), 'utf8');
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === 'ENOENT' || (e as NodeJS.ErrnoException).code === 'EISDIR') return null;
      throw e;
    }
  }

  async list(prefix: string): Promise<string[]> {
    const out: string[] = [];
    const walk = async (dir: string, rel: string): Promise<void> => {
      let entries;
      try {
        entries = await readdir(dir, { withFileTypes: true });
      } catch {
        return;
      }
      for (const e of entries) {
        if (!rel && e.name === FS_TMP) continue;
        const k = rel ? `${rel}/${e.name}` : e.name;
        if (e.isDirectory()) await walk(join(dir, e.name), k);
        else if (k.startsWith(prefix)) out.push(k);
      }
    };
    await walk(this.root, '');
    return out.sort();
  }
}
