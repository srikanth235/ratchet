import type { Clock } from '@ratchet/core';

/** `sleep` resolves (never rejects) when `signal` aborts; callers re-check the signal. */
function sleepFor(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal?.aborted || ms <= 0) return resolve();
    const done = () => {
      clearTimeout(timer);
      signal?.removeEventListener('abort', done);
      resolve();
    };
    const timer = setTimeout(done, ms);
    signal?.addEventListener('abort', done, { once: true });
  });
}

export const systemClock: Clock = {
  now: () => Date.now(),
  sleep: sleepFor,
};

/**
 * Wall clock running `speed` times faster: `sleep(ms)` takes `ms / speed` real ms and `now()` advances
 * `speed` virtual ms per real ms. Lets simulation compress hours into seconds while keeping deadlines
 * and real waits (verdicts arriving from the engine) consistent with each other.
 */
export class AcceleratedClock implements Clock {
  private readonly realStart = Date.now();
  constructor(private readonly speed: number, private readonly virtualStart = Date.now()) {
    if (!(speed > 0)) throw new RangeError('speed must be > 0');
  }
  now(): number {
    return this.virtualStart + (Date.now() - this.realStart) * this.speed;
  }
  sleep(ms: number, signal?: AbortSignal): Promise<void> {
    return sleepFor(ms / this.speed, signal);
  }
}

/** Deterministic clock for tests: time only moves via `advance`. */
export class ManualClock implements Clock {
  private sleepers: { at: number; wake: () => void }[] = [];
  constructor(private time = 0) {}
  now(): number {
    return this.time;
  }
  sleep(ms: number, signal?: AbortSignal): Promise<void> {
    return new Promise((resolve) => {
      if (signal?.aborted || ms <= 0) return resolve();
      const sleeper = {
        at: this.time + ms,
        wake: () => {
          signal?.removeEventListener('abort', sleeper.wake);
          resolve();
        },
      };
      this.sleepers.push(sleeper);
      signal?.addEventListener('abort', sleeper.wake, { once: true });
    });
  }
  /** Move time forward, waking every sleeper that comes due (in time order). */
  async advance(ms: number): Promise<void> {
    const target = this.time + ms;
    for (;;) {
      const due = this.sleepers.filter((s) => s.at <= target).sort((a, b) => a.at - b.at)[0];
      if (!due) break;
      this.sleepers = this.sleepers.filter((s) => s !== due);
      this.time = Math.max(this.time, due.at);
      due.wake();
      await Promise.resolve();
    }
    this.time = target;
  }
  get pendingSleepers(): number {
    return this.sleepers.length;
  }
}
