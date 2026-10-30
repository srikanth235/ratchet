import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { mockClient } from 'aws-sdk-client-mock';
import { GetObjectCommand, ListObjectsV2Command, PutObjectCommand, S3Client } from '@aws-sdk/client-s3';
import { FsArtifactStore, S3ArtifactStore } from './artifacts.js';

const s3 = mockClient(S3Client);
beforeEach(() => s3.reset());

describe('S3ArtifactStore', () => {
  const store = () => new S3ArtifactStore({ bucket: 'b', client: new S3Client({}), prefix: 'artifacts/' });

  it('put encrypts and prefixes', async () => {
    s3.on(PutObjectCommand).resolves({});
    await store().put('o1/a1/events.jsonl', 'x');
    expect(s3.commandCalls(PutObjectCommand)[0]!.args[0].input).toEqual({
      Bucket: 'b',
      Key: 'artifacts/o1/a1/events.jsonl',
      Body: 'x',
      ServerSideEncryption: 'AES256',
    });
  });

  it('get returns body or null for NoSuchKey', async () => {
    s3.on(GetObjectCommand).resolvesOnce({ Body: { transformToString: async () => 'hello' } as never });
    expect(await store().get('k')).toBe('hello');
    expect(s3.commandCalls(GetObjectCommand)[0]!.args[0].input).toEqual({ Bucket: 'b', Key: 'artifacts/k' });
    s3.on(GetObjectCommand).rejects(Object.assign(new Error('nf'), { name: 'NoSuchKey' }));
    expect(await store().get('missing')).toBeNull();
    s3.on(GetObjectCommand).rejects(Object.assign(new Error('denied'), { name: 'AccessDenied' }));
    await expect(store().get('x')).rejects.toThrow('denied');
  });

  it('list pages and strips the prefix', async () => {
    s3.on(ListObjectsV2Command)
      .resolvesOnce({ Contents: [{ Key: 'artifacts/p/b' }, { Key: 'artifacts/p/a' }], IsTruncated: true, NextContinuationToken: 't' })
      .resolvesOnce({ Contents: [{ Key: 'artifacts/p/c' }] });
    expect(await store().list('p/')).toEqual(['p/a', 'p/b', 'p/c']);
    const calls = s3.commandCalls(ListObjectsV2Command);
    expect(calls[0]!.args[0].input).toMatchObject({ Bucket: 'b', Prefix: 'artifacts/p/' });
    expect(calls[1]!.args[0].input.ContinuationToken).toBe('t');
  });

  it.each(['/abs', '../x', 'a/../b', ''])('rejects key %j', async (k) => {
    await expect(store().put(k, 'x')).rejects.toThrow(/invalid artifact key/);
    await expect(store().get(k)).rejects.toThrow();
  });
});

describe('FsArtifactStore', () => {
  let dir: string;
  beforeEach(async () => (dir = await mkdtemp(join(tmpdir(), 'ratchet-art-'))));
  afterEach(() => rm(dir, { recursive: true, force: true }));

  it('round-trips, lists by prefix, nested dirs', async () => {
    const s = new FsArtifactStore(dir);
    await s.put('o1/a1/events.jsonl', 'one');
    await s.put('o1/a2/events.jsonl', new TextEncoder().encode('two'));
    await s.put('o2/a1/x', '3');
    expect(await s.get('o1/a1/events.jsonl')).toBe('one');
    expect(await s.get('o1/a2/events.jsonl')).toBe('two');
    expect(await s.get('nope')).toBeNull();
    expect(await s.get('o1')).toBeNull(); // directory
    expect(await s.list('o1/')).toEqual(['o1/a1/events.jsonl', 'o1/a2/events.jsonl']);
    expect(await s.list('zzz')).toEqual([]);
    await s.put('o1/a1/events.jsonl', 'overwritten');
    expect(await s.get('o1/a1/events.jsonl')).toBe('overwritten');
  });

  it('refuses path traversal', async () => {
    const s = new FsArtifactStore(dir);
    await expect(s.put('../escape', 'x')).rejects.toThrow(/invalid artifact key/);
    await expect(s.get('a/../../escape')).rejects.toThrow();
  });
});
