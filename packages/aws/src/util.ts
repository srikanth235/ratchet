import { createHash } from 'node:crypto';
import type { Clock } from '@ratchet/core';

/** Reserved tag keys (infra/contract.md identity model). */
export const TAG_OPT = 'ratchet:opt';
export const TAG_AGENT = 'ratchet:agent';
export const TAG_ROLE = 'ratchet:role';
export const TAG_SANDBOX = 'ratchet:sandbox';
export const TAG_PURPOSE = 'ratchet:purpose';

export const DEFAULT_AGENT_ROLE_PATH = '/ratchet/agent/';

const ID_RE = /^[a-z0-9-]+$/;
const MAX_ID_LEN = 64;

/**
 * Ids are substituted into IAM names, tag values, SSM paths and policy
 * patterns, so they are restricted to `[a-z0-9-]` (infra/contract.md).
 */
export function assertValidId(kind: string, id: string): void {
  if (typeof id !== 'string' || id.length === 0 || id.length > MAX_ID_LEN || !ID_RE.test(id)) {
    throw new Error(`invalid ${kind} id ${JSON.stringify(id)}: must match [a-z0-9-], 1-${MAX_ID_LEN} chars`);
  }
}

/** EC2 instance ids end up in API calls and filters (`*` is a wildcard in EC2 filters), so keep them plain. */
export function assertValidInstanceId(id: string): void {
  if (typeof id !== 'string' || !/^i-[0-9a-z]{1,40}$/.test(id)) {
    throw new Error(`invalid instance id ${JSON.stringify(id)}: must match i-[0-9a-z]+`);
  }
}

export function assertValidSandboxId(id: string): void {
  if (!/^[a-z0-9-]{2,32}$/.test(id)) {
    throw new Error(`invalid sandbox id ${JSON.stringify(id)}: must match [a-z0-9-], 2-32 chars`);
  }
}

/** IAM names are limited to 64 chars; keep readable names and hash-shorten long ones. */
export function boundedName(name: string, max = 64): string {
  if (name.length <= max) return name;
  const h = createHash('sha1').update(name).digest('hex').slice(0, 8);
  return `${name.slice(0, max - 9)}-${h}`;
}

/** Last path segment of an ARN (role/profile name). */
export function arnName(arn: string): string {
  const i = arn.lastIndexOf('/');
  return i === -1 ? arn : arn.slice(i + 1);
}

export function shQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

export const systemClock: Clock = {
  now: () => Date.now(),
  sleep: (ms, signal) =>
    new Promise<void>((resolve, reject) => {
      if (signal?.aborted) return reject(new Error('aborted'));
      const t = setTimeout(() => {
        signal?.removeEventListener('abort', onAbort);
        resolve();
      }, ms);
      const onAbort = () => {
        clearTimeout(t);
        reject(new Error('aborted'));
      };
      signal?.addEventListener('abort', onAbort, { once: true });
    }),
};

/** Deterministic clock for tests and simulation: `sleep` advances time instantly. */
export class ManualClock implements Clock {
  constructor(private t = 0) {}
  now(): number {
    return this.t;
  }
  advance(ms: number): void {
    this.t += ms;
  }
  set(t: number): void {
    this.t = t;
  }
  async sleep(ms: number, signal?: AbortSignal): Promise<void> {
    if (signal?.aborted) throw new Error('aborted');
    this.t += ms;
  }
}

export function errName(e: unknown): string {
  return (e as { name?: string; Code?: string })?.name ?? (e as { Code?: string })?.Code ?? '';
}
