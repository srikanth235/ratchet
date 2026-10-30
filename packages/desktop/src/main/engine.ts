import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { EngineApi, EngineEvent, Ledger, Mode, SandboxProvider } from '@ratchet/core';
import { createSimulationRuntime, type SimulationRuntime } from '@ratchet/runtime';
import { createDevEngine } from './dev-app.js';

/** Everything the main process needs from "the engine side". */
export type { AppEngine } from './dev-app.js';
import type { AppEngine } from './dev-app.js';

export interface CreateEngineOptions {
  tickMs?: number;
  seed?: number;
  /** Seed demo data into an empty ledger (dev engine only). Default true. */
  seedDemo?: boolean;
  /** Which engine to build. Default: `real`, unless RATCHET_ENGINE=dev. */
  engine?: 'dev' | 'real';
  /** Monorepo root (simulation needs examples/ and node_modules). Default: RATCHET_REPO_ROOT or derived from this file. */
  repoRoot?: string;
  /** Scratch root for simulation runs. Default: <repoRoot>/.ratchet-app-sim */
  workRoot?: string;
}

/**
 * THE composition point. The desktop app builds its engine here and nowhere else.
 *
 * - `simulation` mode: the real Engine on mock AWS + local git + local bench + scripted agents
 *   (@ratchet/runtime createSimulationRuntime) against examples/demo-search.
 * - `live` mode: needs sandbox outputs, GitHub and API keys from Settings; until those flows land
 *   it reports a clear error instead of pretending (see docs/REAL_ENV_TESTING.md).
 */
export function createEngineForApp(ledger: Ledger, mode: Mode, opts: CreateEngineOptions = {}): AppEngine {
  const which = opts.engine ?? (process.env.RATCHET_ENGINE === 'dev' ? 'dev' : 'real');
  if (which === 'dev') {
    // RATCHET_DEV_TICK_MS: slow the simulated agents down (design screenshots want a stable ledger).
    const tick = Number(process.env.RATCHET_DEV_TICK_MS);
    return createDevEngine(ledger, mode, { ...opts, ...(opts.tickMs === undefined && tick > 0 ? { tickMs: tick } : {}) });
  }
  if (mode === 'live') return createUnconfiguredLive();

  const repoRoot = resolve(opts.repoRoot ?? process.env.RATCHET_REPO_ROOT ?? join(dirname(fileURLToPath(import.meta.url)), '../../../..'));
  const workRoot = resolve(opts.workRoot ?? join(repoRoot, '.ratchet-app-sim', String(Date.now())));
  mkdirSync(workRoot, { recursive: true });
  const ready: Promise<SimulationRuntime> = createSimulationRuntime({ ledger, repoRoot, workRoot });
  const listeners = new Set<(e: EngineEvent) => void>();
  const ac = new AbortController();
  void ready.then((rt) => {
    rt.engine.subscribe((e) => listeners.forEach((l) => l(e)));
    void rt.engine.run(ac.signal);
  });
  const call = async <T>(f: (rt: SimulationRuntime) => Promise<T>): Promise<T> => f(await ready);

  const engine: EngineApi = {
    mode: () => 'simulation',
    // In simulation the repo is the prepared copy of examples/demo-search, whatever the form said.
    createOptimization: (input) => call((rt) => rt.engine.createOptimization({ ...input, repoUrl: rt.repoUrl, baseRef: rt.baseRef })),
    start: (id) => call((rt) => rt.engine.start(id)),
    stop: (id, reason) => call((rt) => rt.engine.stop(id, reason)),
    pauseAgent: (id) => call((rt) => rt.engine.pauseAgent(id)),
    resumeAgent: (id) => call((rt) => rt.engine.resumeAgent(id)),
    messageAgent: (id, text) => call((rt) => rt.engine.messageAgent(id, text)),
    openPullRequest: (id) => call((rt) => rt.engine.openPullRequest(id)),
    teardown: (id) => call((rt) => rt.engine.teardown(id)),
    subscribe: (l) => (listeners.add(l), () => listeners.delete(l)),
  };
  const sandbox: SandboxProvider = {
    plan: (i) => call((rt) => rt.sandbox.plan(i)),
    apply: (i) => call((rt) => rt.sandbox.apply(i)),
    destroy: (i) => call((rt) => rt.sandbox.destroy(i)),
    outputs: (id) => call((rt) => rt.sandbox.outputs(id)),
  };
  return {
    kind: 'real',
    engine,
    sandbox,
    // In simulation every optimization lives in the one prepared repo.
    diff: ({ baseRef, sha }) => call((rt) => rt.git.diff(baseRef, sha)),
    dispose: () => {
      ac.abort();
      void ready.then((rt) => rt.dispose());
    },
  };
}

function createUnconfiguredLive(): AppEngine {
  const fail = async (): Promise<never> => {
    throw new Error('Live mode needs a sandbox, GitHub token and API keys (Settings). Use simulation mode until the real-environment setup in docs/REAL_ENV_TESTING.md is done.');
  };
  const engine: EngineApi = {
    mode: () => 'live',
    createOptimization: fail,
    start: fail,
    stop: fail,
    pauseAgent: fail,
    resumeAgent: fail,
    messageAgent: fail,
    openPullRequest: fail,
    teardown: fail,
    subscribe: () => () => {},
  };
  return { kind: 'real', engine, sandbox: { plan: fail, apply: fail, destroy: fail, outputs: async () => null }, dispose: () => {} };
}
