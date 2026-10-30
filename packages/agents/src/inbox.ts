import type { RunnerInbox, RunnerMessage } from '@ratchet/core';

/** In-process inbox: `push` from the engine side, `next` from the loop. */
export class MemoryInbox implements RunnerInbox {
  private queue: RunnerMessage[] = [];
  private waiters: ((m: RunnerMessage | null) => void)[] = [];

  push(message: RunnerMessage): void {
    const waiter = this.waiters.shift();
    if (waiter) waiter(message);
    else this.queue.push(message);
  }

  get pending(): number {
    return this.queue.length;
  }

  next(signal?: AbortSignal): Promise<RunnerMessage | null> {
    const queued = this.queue.shift();
    if (queued) return Promise.resolve(queued);
    if (signal?.aborted) return Promise.resolve(null);
    return new Promise((resolve) => {
      const waiter = (m: RunnerMessage | null) => {
        signal?.removeEventListener('abort', onAbort);
        resolve(m);
      };
      const onAbort = () => {
        this.waiters = this.waiters.filter((w) => w !== waiter);
        resolve(null);
      };
      this.waiters.push(waiter);
      signal?.addEventListener('abort', onAbort, { once: true });
    });
  }
}
