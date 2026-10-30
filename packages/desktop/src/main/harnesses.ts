import type { HarnessInfo } from '@ratchet/agents';
import type { HarnessVM } from '../shared/types.js';

export interface HarnessProbe {
  loginPath(): Promise<string | undefined>;
  detect(loginPath: string | undefined): Promise<HarnessInfo[]>;
}

// Loaded on first use: the renderer tests (jsdom) import this module without ever probing the machine.
const realProbe: HarnessProbe = {
  loginPath: async () => (await (await import('@ratchet/agents')).resolveLoginPath()).path,
  detect: async (loginPath) => (await import('@ratchet/agents')).detectHarnesses(loginPath ? { loginPath } : {}),
};

/**
 * The detected agent CLIs, probed in the main process on launch and on demand ("Re-detect"), cached, and handed to
 * `createHybridRuntime` so the engine does not probe again. A failed probe is an empty result, never a crash.
 */
export class HarnessCache {
  private current: Promise<{ harnesses: HarnessInfo[]; loginPath: string | undefined }> | null = null;

  constructor(private readonly probe: HarnessProbe = realProbe) {}

  /** The cached result; probes once if nothing was probed yet. */
  get(): Promise<{ harnesses: HarnessInfo[]; loginPath: string | undefined }> {
    this.current ??= this.run();
    return this.current;
  }

  /** Probe again and replace the cache. */
  refresh(): Promise<{ harnesses: HarnessInfo[]; loginPath: string | undefined }> {
    this.current = this.run();
    return this.current;
  }

  private async run(): Promise<{ harnesses: HarnessInfo[]; loginPath: string | undefined }> {
    try {
      const loginPath = await this.probe.loginPath();
      return { harnesses: await this.probe.detect(loginPath), loginPath };
    } catch {
      return { harnesses: [], loginPath: undefined };
    }
  }
}

export const toHarnessVM = (h: HarnessInfo): HarnessVM => ({ kind: h.kind, installed: h.installed, version: h.version, path: h.path, auth: h.auth, hint: h.hint });
