import type { EngineApi, Ledger, Mode, SandboxProvider } from '@ratchet/core';
import { DevEngine } from './dev-engine.js';
import { DevSandboxProvider } from './dev-sandbox.js';
import { isLedgerEmpty, seedDemo } from './seed.js';

/** Everything the main process needs from "the engine side". */
export interface AppEngine {
  /** `dev` = the in-repo simulated stub; `real` = the core engine. Shown in the UI. */
  kind: 'dev' | 'real';
  engine: EngineApi;
  sandbox: SandboxProvider;
  /** Release timers / connections (called on mode switch and app quit). */
  dispose(): void;
}

export interface CreateEngineOptions {
  tickMs?: number;
  seed?: number;
  /** Seed demo data into an empty ledger. Default true for the dev engine. */
  seedDemo?: boolean;
}

/**
 * THE composition point. The desktop app builds its engine here and nowhere else.
 * The orchestrator replaces this body with the real engine + sandbox provider; keep the signature.
 */
export function createEngineForApp(ledger: Ledger, mode: Mode, opts: CreateEngineOptions = {}): AppEngine {
  const engine = new DevEngine(ledger, { mode, ...(opts.tickMs !== undefined && { tickMs: opts.tickMs }), ...(opts.seed !== undefined && { seed: opts.seed }) });
  if ((opts.seedDemo ?? true) && isLedgerEmpty(ledger)) seedDemo(ledger);
  // Resume anything that was running when the app last closed (the real engine does this from the ledger).
  for (const o of ledger.listOptimizations({ status: ['baselining', 'running'] })) void engine.start(o.id);
  return { kind: 'dev', engine, sandbox: new DevSandboxProvider(), dispose: () => engine.dispose() };
}
