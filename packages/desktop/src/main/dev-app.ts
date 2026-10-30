import type { EngineApi, Ledger, Mode, SandboxProvider } from '@ratchet/core';
import { DevEngine } from './dev-engine.js';
import { DevSandboxProvider } from './dev-sandbox.js';
import { isLedgerEmpty, seedDemo } from './seed.js';

/** Everything the main process needs from "the engine side". */
export interface AppEngine {
  /** `dev` = the in-repo stub with seeded demo data; `real` = the core engine. Shown in the UI. */
  kind: 'dev' | 'real';
  engine: EngineApi;
  sandbox: SandboxProvider;
  /** Release timers / connections (called on mode switch and app quit). */
  dispose(): void;
}

export interface DevEngineOptions {
  tickMs?: number;
  seed?: number;
  seedDemo?: boolean;
}

/** The dev stub (seeded demo data, simulated progress). Used by renderer tests and RATCHET_ENGINE=dev. */
export function createDevEngine(ledger: Ledger, mode: Mode, opts: DevEngineOptions = {}): AppEngine {
  const engine = new DevEngine(ledger, { mode, ...(opts.tickMs !== undefined && { tickMs: opts.tickMs }), ...(opts.seed !== undefined && { seed: opts.seed }) });
  if ((opts.seedDemo ?? true) && isLedgerEmpty(ledger)) seedDemo(ledger);
  for (const o of ledger.listOptimizations({ status: ['baselining', 'running'] })) void engine.start(o.id);
  return { kind: 'dev', engine, sandbox: new DevSandboxProvider(), dispose: () => engine.dispose() };
}

