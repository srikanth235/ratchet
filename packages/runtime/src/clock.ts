import type { Clock } from '@ratchet/core';

export const systemClock: Clock = {
  now: () => Date.now(),
  sleep: (ms, signal) =>
    new Promise<void>((resolve) => {
      if (signal?.aborted) return resolve();
      const t = setTimeout(resolve, ms);
      signal?.addEventListener('abort', () => (clearTimeout(t), resolve()), { once: true });
    }),
};
