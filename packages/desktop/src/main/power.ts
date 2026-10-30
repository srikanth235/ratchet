import type { EngineEventVM } from '../shared/types.js';

/** The slice of Electron's `powerSaveBlocker` we use. */
export interface PowerSaveBlockerLike {
  start(type: 'prevent-app-suspension' | 'prevent-display-sleep'): number;
  stop(id: number): void;
  isStarted?(id: number): boolean;
}

/**
 * Keep the machine awake while a run is active: `run-active: true` starts a `prevent-app-suspension` blocker,
 * `false` stops it. Idempotent, so a repeated `true` never leaks a second blocker.
 */
export function createPowerSaveHook(blocker: PowerSaveBlockerLike): { onEvent(e: EngineEventVM): void; held(): boolean; release(): void } {
  let id: number | null = null;
  const release = (): void => {
    if (id !== null) {
      try {
        blocker.stop(id);
      } catch {
        /* already stopped */
      }
      id = null;
    }
  };
  return {
    onEvent(e) {
      if (e.type !== 'run-active') return;
      if (e.active) {
        if (id === null) id = blocker.start('prevent-app-suspension');
      } else release();
    },
    held: () => id !== null,
    release,
  };
}
