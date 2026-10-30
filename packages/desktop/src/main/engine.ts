import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SandboxLifecycle, type EngineApi, type EngineEvent, type Ledger, type Mode, type SandboxProvider } from '@ratchet/core';
import { createLiveRuntime, createSimulationRuntime, type AwsSandboxOutputs, type LiveRuntime, type SimulationRuntime } from '@ratchet/runtime';
import { sandboxIdFor } from './constants.js';
import { createDevEngine } from './dev-app.js';
import { effectivePaths, secretSource, type LivePaths } from './live-config.js';
import type { SettingsStore } from './settings.js';

/** Everything the main process needs from "the engine side". */
export type { AppEngine } from './dev-app.js';
import type { AppEngine } from './dev-app.js';

/** What live mode is built from: Settings (non-secret values and, in the main process only, the secrets). */
export interface LiveAppConfig {
  settings: SettingsStore;
  /** Where per-sandbox OpenTofu state, the git cache and scratch dirs live (userData/sandboxes). */
  stateRoot: string;
  /** What empty tofu settings resolve to (bundled infra/aws in a packaged app). */
  defaults: LivePaths;
  /** Test seam: replaces `createLiveRuntime`. */
  createRuntime?: typeof createLiveRuntime;
}

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
  /** Needed for live mode; without it live mode reports that it is not configured. */
  live?: LiveAppConfig;
}

/**
 * THE composition point. The desktop app builds its engine here and nowhere else.
 *
 * - `simulation` mode: the real Engine on mock AWS + local git + local bench + scripted agents
 *   (@ratchet/runtime createSimulationRuntime) against examples/demo-search.
 * - `live` mode: the real Engine on real AWS + GitHub, built from Settings (createLiveRuntime). Before the sandbox
 *   is applied only plan / apply / destroy work; after the apply the service builds the app again with the
 *   outputs from the ledger. See docs/REAL_ENV_TESTING.md.
 */
export function createEngineForApp(ledger: Ledger, mode: Mode, opts: CreateEngineOptions = {}): AppEngine {
  const which = opts.engine ?? (process.env.RATCHET_ENGINE === 'dev' ? 'dev' : 'real');
  if (which === 'dev') {
    // RATCHET_DEV_TICK_MS: slow the simulated agents down (design screenshots want a stable ledger).
    const tick = Number(process.env.RATCHET_DEV_TICK_MS);
    return createDevEngine(ledger, mode, { ...opts, ...(opts.tickMs === undefined && tick > 0 ? { tickMs: tick } : {}) });
  }
  if (mode === 'live') return createLiveApp(ledger, opts.live);

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
    sandboxState: (id) => call((rt) => rt.engine.sandboxState(id)),
    planSandbox: (i) => call((rt) => rt.engine.planSandbox(i)),
    applySandbox: (i) => call((rt) => rt.engine.applySandbox(i)),
    destroySandbox: (id) => call((rt) => rt.engine.destroySandbox(id)),
    subscribe: (l) => (listeners.add(l), () => listeners.delete(l)),
  };
  return {
    kind: 'real',
    engine,
    // In simulation every optimization lives in the one prepared repo.
    diff: ({ baseRef, sha }) => call((rt) => rt.git.diff(baseRef, sha)),
    dispose: () => {
      ac.abort();
      void ready.then((rt) => rt.dispose());
    },
  };
}

/**
 * Live mode from Settings. Never throws: when the settings cannot build a runtime (or there are none) the returned
 * engine fails every command with the reason, and the Settings screen shows what to fix.
 */
function createLiveApp(ledger: Ledger, live: LiveAppConfig | undefined): AppEngine {
  if (!live) return createUnconfiguredLive(ledger, 'Live mode is not configured: open Settings and fill in the AWS, GitHub and image settings.');
  const { settings } = live;
  const settingsNow = settings.live;
  const paths = effectivePaths(settingsNow, live.defaults);
  const sandboxId = sandboxIdFor('live');

  // Only a sandbox that the real provider applied: a mock row (from simulation) must never reach the AWS clients.
  const row = ledger.getSandbox(sandboxId);
  const outputs = row && row.status === 'applied' && row.provider === 'aws' && row.outputs ? (row.outputs as unknown as AwsSandboxOutputs) : null;

  let rt: LiveRuntime;
  try {
    mkdirSync(live.stateRoot, { recursive: true });
    rt = (live.createRuntime ?? createLiveRuntime)({
      ledger,
      region: settingsNow.awsRegion || 'us-east-1',
      sandbox: outputs,
      sandboxId,
      githubRepo: settingsNow.githubRepo || undefined,
      githubToken: settings.getSecret('githubToken') ?? undefined,
      runnerImageRef: settingsNow.runnerImageRef || undefined,
      benchImageRef: settingsNow.benchImageRef || undefined,
      tofuBinary: paths.tofuBinary,
      moduleDir: paths.moduleDir,
      stateRoot: live.stateRoot,
      secrets: secretSource(settings),
      awsProfile: settingsNow.awsProfile || undefined,
    });
  } catch (err) {
    return createUnconfiguredLive(ledger, `Live mode could not start: ${(err as Error).message}`);
  }

  const ac = new AbortController();
  void rt.engine.run(ac.signal);
  return {
    kind: 'real',
    engine: rt.engine,
    dispose: () => {
      ac.abort();
      void rt.dispose();
    },
  };
}

/** Every command fails with `reason`; the sandbox state is still readable from the ledger. */
function createUnconfiguredLive(ledger: Ledger, reason: string): AppEngine {
  const fail = async (): Promise<never> => {
    throw new Error(reason);
  };
  const provider: SandboxProvider = { plan: fail, apply: fail, destroy: fail, outputs: async () => null };
  const lifecycle = new SandboxLifecycle({ ledger, provider, kind: 'aws' });
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
    sandboxState: async (id) => lifecycle.state(id),
    planSandbox: fail,
    applySandbox: fail,
    destroySandbox: fail,
    subscribe: () => () => {},
  };
  return { kind: 'real', engine, dispose: () => {} };
}
