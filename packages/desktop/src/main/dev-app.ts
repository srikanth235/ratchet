import type { EngineApi, EngineCreateOptimizationInput, FileDiff, Ledger, Mode } from '@ratchet/core';
import type { DemoId } from '../shared/types.js';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DevEngine } from './dev-engine.js';
import { DevGit } from './dev-git.js';
import { isLedgerEmpty, seedDemo } from './seed.js';

/** Everything the main process needs from "the engine side". */
export interface AppEngine {
  /** `dev` = the in-repo stub with seeded demo data; `real` = the core engine. Shown in the UI. */
  kind: 'dev' | 'real';
  engine: EngineApi;
  /** Start a demo template in simulation (mock agents, no setup). Absent: the engine runs every optimization the same way (the dev stub). */
  createDemo?(demo: DemoId, input: EngineCreateOptimizationInput): Promise<string>;
  /** What still blocks a real run on this GitHub repo ("what's missing"); builds the repo's runtime. Absent: nothing blocks. */
  missingFor?(slug: string): Promise<string[]>;
  /** Settings or the GitHub sign-in changed: runtimes built from them are rebuilt on next use. */
  invalidate?(): void;
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
    diff: (req) => git.diff(req),
    dispose: () => {
      engine.dispose();
      git.dispose();
    },
  };
}

