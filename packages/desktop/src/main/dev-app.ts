import type { EngineApi, FileDiff, Ledger, Mode, SandboxProvider } from '@ratchet/core';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DevEngine } from './dev-engine.js';
import { DevGit } from './dev-git.js';
import { DevSandboxProvider } from './dev-sandbox.js';
import { isLedgerEmpty, seedDemo } from './seed.js';

/** Everything the main process needs from "the engine side". */
export interface AppEngine {
  /** `dev` = the in-repo stub with seeded demo data; `real` = the core engine. Shown in the UI. */
  kind: 'dev' | 'real';
  engine: EngineApi;
  sandbox: SandboxProvider;
  /**
   * Diff of a verified commit against the optimization's base ref (GitHost.diff), for the Result screen's
   * "Winning change". Resolves null when the commit is unknown to this engine's git; absent without git access.
   */
  diff?(req: { optimizationId: string; repo: string; baseRef: string; sha: string }): Promise<FileDiff[] | null>;
  /** Release timers / connections (called on mode switch and app quit). */
  dispose(): void;
}

export interface DevEngineOptions {
  tickMs?: number;
  seed?: number;
  seedDemo?: boolean;
  /** Monorepo root, for the real demo patches behind the dev engine's diffs. Default: RATCHET_REPO_ROOT or derived from this file. */
  repoRoot?: string;
}

/** The dev stub (seeded demo data, simulated progress). Used by renderer tests and RATCHET_ENGINE=dev. */
export function createDevEngine(ledger: Ledger, mode: Mode, opts: DevEngineOptions = {}): AppEngine {
  const engine = new DevEngine(ledger, { mode, ...(opts.tickMs !== undefined && { tickMs: opts.tickMs }), ...(opts.seed !== undefined && { seed: opts.seed }) });
  if ((opts.seedDemo ?? true) && isLedgerEmpty(ledger)) seedDemo(ledger);
  for (const o of ledger.listOptimizations({ status: ['baselining', 'running'] })) void engine.start(o.id);
  const git = new DevGit(ledger, opts.repoRoot ?? process.env.RATCHET_REPO_ROOT ?? join(dirname(fileURLToPath(import.meta.url)), '../../../..'));
  return {
    kind: 'dev',
    engine,
    sandbox: new DevSandboxProvider(),
    diff: (req) => git.diff(req),
    dispose: () => {
      engine.dispose();
      git.dispose();
    },
  };
}

