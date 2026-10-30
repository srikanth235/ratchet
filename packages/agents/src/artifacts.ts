import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, relative, sep } from 'node:path';
import type { AgentEvent, ArtifactStore, Clock, OptimizationId, AgentId, RunnerInbox, RunnerMessage } from '@ratchet/core';
import { systemClock } from './clock.js';
import { runProcess, nodeSpawner, type Spawner } from './process.js';

/** Where one agent's runner state lives in the artifacts bucket (IAM allows exactly this prefix). */
export const runnerPrefix = (optimizationId: OptimizationId, agentId: AgentId): string => `artifacts/${optimizationId}/${agentId}/`;

const pad = (n: number): string => String(n).padStart(8, '0');
export const eventKey = (prefix: string, seq: number): string => `${prefix}events/${pad(seq)}.jsonl`;
export const inboxKey = (prefix: string, seq: number): string => `${prefix}inbox/${pad(seq)}.json`;
export const configKey = (prefix: string): string => `${prefix}config.json`;
export const doneKey = (prefix: string): string => `${prefix}done.json`;

/** `artifacts/o/a/events/00000012.jsonl` -> 12 (NaN for anything else). */
export function seqOf(key: string): number {
  const m = /\/(\d{8})\.[a-z]+$/.exec(key);
  return m ? Number(m[1]) : NaN;
}

async function listSeqs(store: ArtifactStore, dirPrefix: string): Promise<{ key: string; seq: number }[]> {
  const keys = await store.list(dirPrefix);
  return keys
    .map((key) => ({ key, seq: seqOf(key) }))
    .filter((k) => Number.isFinite(k.seq))
    .sort((a, b) => a.seq - b.seq);
}

/** Highest sequence number under `dirPrefix` (0 when empty). */
export async function lastSeq(store: ArtifactStore, dirPrefix: string): Promise<number> {
  const all = await listSeqs(store, dirPrefix);
  return all.at(-1)?.seq ?? 0;
}

/** Read engine -> runner messages, numbered objects `inbox/00000001.json`, in order, exactly once. */
export class ArtifactInbox implements RunnerInbox {
  private last: number;
  private buffered: { key: string; seq: number }[] = [];

  constructor(
    private readonly store: ArtifactStore,
    private readonly prefix: string,
    private readonly clock: Clock = systemClock,
    private readonly pollIntervalMs = 2000,
    /** Skip messages up to this sequence (for a runner restarted mid-run). */
    startAfter = 0,
  ) {
    this.last = startAfter;
  }

  async next(signal?: AbortSignal): Promise<RunnerMessage | null> {
    while (!signal?.aborted) {
      if (this.buffered.length === 0) this.buffered = (await listSeqs(this.store, `${this.prefix}inbox/`)).filter((k) => k.seq > this.last);
      const item = this.buffered.shift();
      if (item) {
        this.last = item.seq;
        const body = await this.store.get(item.key);
        if (!body) continue;
        try {
          const message = JSON.parse(body) as RunnerMessage;
          if (message && typeof message.type === 'string') return message;
        } catch {
          /* malformed message: skip it rather than wedge the runner */
        }
        continue;
      }
      await this.clock.sleep(this.pollIntervalMs, signal);
    }
    return null;
  }
}

export interface EventSinkOptions {
  flushIntervalMs?: number;
  /** Flush at once when this many events are buffered. */
  maxBuffered?: number;
}

/** Runner -> engine events: buffered, written as numbered JSONL objects (`events/00000001.jsonl`). */
export class ArtifactEventSink {
  private buffer: AgentEvent[] = [];
  private seq = 0;
  private chain: Promise<void> = Promise.resolve();
  private timer: NodeJS.Timeout | undefined;
  private ready: Promise<void>;
  errors: unknown[] = [];

  constructor(
    private readonly store: ArtifactStore,
    private readonly prefix: string,
    private readonly options: EventSinkOptions = {},
  ) {
    // continue numbering if the runner restarted
    this.ready = lastSeq(store, `${prefix}events/`).then((n) => void (this.seq = n));
    const interval = options.flushIntervalMs ?? 2000;
    this.timer = setInterval(() => void this.flush(), interval);
    this.timer.unref?.();
  }

  emit = (event: AgentEvent): void => {
    this.buffer.push(event);
    if (this.buffer.length >= (this.options.maxBuffered ?? 50)) void this.flush();
  };

  flush(): Promise<void> {
    const batch = this.buffer;
    if (batch.length === 0) return this.chain;
    this.buffer = [];
    this.chain = this.chain.then(async () => {
      await this.ready;
      try {
        await this.store.put(eventKey(this.prefix, ++this.seq), `${batch.map((e) => JSON.stringify(e)).join('\n')}\n`);
      } catch (err) {
        this.seq--;
        this.buffer.unshift(...batch); // retry on the next flush
        this.errors.push(err);
      }
    });
    return this.chain;
  }

  async close(): Promise<void> {
    clearInterval(this.timer);
    await this.flush();
    await this.chain;
  }
}

/** Parse every event line from the batches after `cursor`. Returns the new cursor (highest seq read). */
export async function readEvents(store: ArtifactStore, prefix: string, cursor?: string): Promise<{ events: AgentEvent[]; cursor: string }> {
  const after = cursor ? Number(cursor) : 0;
  const batches = (await listSeqs(store, `${prefix}events/`)).filter((k) => k.seq > after);
  const events: AgentEvent[] = [];
  let last = after;
  for (const { key, seq } of batches) {
    const body = await store.get(key);
    if (body === null) break;
    for (const line of body.split('\n')) {
      if (!line.trim()) continue;
      try {
        events.push(JSON.parse(line) as AgentEvent);
      } catch {
        /* skip a torn line */
      }
    }
    last = seq;
  }
  return { events, cursor: String(last) };
}

/** Filesystem-backed store (keys are relative paths under `root`): the local stand-in for S3, also used by tests and the runner bin. */
export class FsArtifactStore implements ArtifactStore {
  constructor(private readonly root: string) {}
  async put(key: string, body: string | Uint8Array): Promise<void> {
    const file = join(this.root, key);
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, body);
  }
  async get(key: string): Promise<string | null> {
    return readFile(join(this.root, key), 'utf8').catch(() => null);
  }
  async list(prefix: string): Promise<string[]> {
    const out: string[] = [];
    const walk = async (dir: string): Promise<void> => {
      for (const entry of await readdir(dir, { withFileTypes: true }).catch(() => [])) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) await walk(full);
        else out.push(relative(this.root, full).split(sep).join('/'));
      }
    };
    await walk(this.root);
    return out.filter((k) => k.startsWith(prefix)).sort();
  }
}

/**
 * S3 through the `aws` CLI (present on the runner image; credentials come from the instance profile).
 * The real `@ratchet/aws` S3 store is used by the engine; this one lets the runner stay dependency-free.
 */
export class AwsCliArtifactStore implements ArtifactStore {
  constructor(
    private readonly bucket: string,
    private readonly spawner: Spawner = nodeSpawner,
    private readonly env: Record<string, string> = process.env as Record<string, string>,
    private readonly region?: string,
  ) {}

  private async aws(args: string[], stdin?: string): Promise<{ code: number | null; out: string; err: string }> {
    let out = '';
    const res = await runProcess(this.spawner, 'aws', [...args, ...(this.region ? ['--region', this.region] : [])], {
      cwd: process.cwd(),
      env: this.env,
      ...(stdin !== undefined ? { stdin } : {}),
      onStdoutLine: (l) => (out += `${l}\n`),
    });
    return { code: res.code, out, err: res.stderrTail };
  }

  async put(key: string, body: string | Uint8Array): Promise<void> {
    const text = typeof body === 'string' ? body : Buffer.from(body).toString('utf8');
    const r = await this.aws(['s3', 'cp', '-', `s3://${this.bucket}/${key}`, '--only-show-errors'], text);
    if (r.code !== 0) throw new Error(`aws s3 cp failed for ${key}: ${r.err}`);
  }

  async get(key: string): Promise<string | null> {
    const r = await this.aws(['s3', 'cp', `s3://${this.bucket}/${key}`, '-', '--only-show-errors']);
    if (r.code === 0) return r.out.replace(/\n$/, '') + (r.out.endsWith('\n') ? '\n' : '');
    if (/NoSuchKey|Not Found|404/.test(r.err)) return null;
    throw new Error(`aws s3 cp failed for ${key}: ${r.err}`);
  }

  async list(prefix: string): Promise<string[]> {
    const r = await this.aws(['s3api', 'list-objects-v2', '--bucket', this.bucket, '--prefix', prefix, '--query', 'Contents[].Key', '--output', 'json']);
    if (r.code !== 0) throw new Error(`aws s3api list-objects-v2 failed: ${r.err}`);
    const parsed = JSON.parse(r.out.trim() || 'null') as string[] | null;
    return parsed ?? [];
  }
}
