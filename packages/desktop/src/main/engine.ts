import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ImageLookupError, regionForOptimization, resolveImageRefs } from '@ratchet/aws';
import type {
  Calibration,
  EffectiveProtected,
  EngineApi,
  EngineCreateOptimizationInput,
  EngineEvent,
  FileDiff,
  Ledger,
  OptimizationId,
  ProposeTemplateInput,
  ProposeTemplateResult,
  SandboxActivity,
  SandboxTeardownReceipt,
} from '@ratchet/core';
import { GitHubGitHost } from '@ratchet/github';
import { createHybridRuntime, createSimulationRuntime, type HybridOptions, type LiveRuntime, type SimulationProject, type SimulationRuntime } from '@ratchet/runtime';
import type { DemoId } from '../shared/types.js';
import { createDevEngine, type AppEngine } from './dev-app.js';
import type { HarnessCache } from './harnesses.js';
import { friendlyMissing, slugOf } from './repo-slug.js';
import { effectivePaths, secretSource, type LivePaths } from './live-config.js';
import type { SettingsStore } from './settings.js';

/** Everything the main process needs from "the engine side". */
export type { AppEngine } from './dev-app.js';
export { friendlyMissing, slugOf } from './repo-slug.js';

export const DEMO_PROJECTS: Record<DemoId, SimulationProject> = { 'demo-search': 'search', 'demo-cli': 'cli', 'demo-forecast': 'forecast' };

/** What the production engine is built from: Settings (non-secret values and, in the main process only, the secrets). */
export interface LiveAppConfig {
  settings: SettingsStore;
  /** Where per-sandbox OpenTofu state, the git cache, local agent worktrees and scratch dirs live (userData/sandboxes). */
  stateRoot: string;
  /** What empty tofu settings resolve to (bundled infra/aws in a packaged app). */
  defaults: LivePaths;
  /** The detected agent CLIs, cached by the main process. */
  harnesses: HarnessCache;
  /** The app version the published AMIs are matched against (`ratchet:image_version`). */
  appVersion?: string;
  /** The parsed ratchet.yaml of a repo the user looked at in step 1 (for `environment.region`). */
  configFor?: (slug: string) => unknown;
  /** Test seams. */
  createRuntime?: typeof createHybridRuntime;
  resolveImages?: typeof resolveImageRefs;
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
  /** Idle window of the automatic environment in demos, ms (RATCHET_SIM_IDLE_MS; default 30 min). */
  simIdleMs?: number;
  /** Needed for real runs; without it only demos work. */
  live?: LiveAppConfig;
  /** Test seam: replaces `createSimulationRuntime`. */
  createSimulation?: typeof createSimulationRuntime;
}

/**
 * THE composition point. The desktop app builds its engine here and nowhere else.
 *
 * - Real runs (a GitHub repo): `createHybridRuntime` per repository. Agents run on this machine by default (the user's own
 *   Claude Code / Codex login), or on cloud runners; the evaluator always runs in the cloud sandbox, which the engine creates
 *   at the first run and removes after the idle window.
 * - Demos: the same real Engine on mock AWS + local git + local bench + scripted agents (`createSimulationRuntime`), one runtime per
 *   demo project, created on first use. No setup, no cloud spend.
 * - `RATCHET_ENGINE=dev`: the seeded stub (design screenshots and renderer tests).
 */
export function createEngineForApp(ledger: Ledger, opts: CreateEngineOptions = {}): AppEngine {
  const which = opts.engine ?? (process.env.RATCHET_ENGINE === 'dev' ? 'dev' : 'real');
  if (which === 'dev') {
    // RATCHET_DEV_TICK_MS: slow the simulated agents down (design screenshots want a stable ledger).
    const tick = Number(process.env.RATCHET_DEV_TICK_MS);
    return createDevEngine(ledger, 'simulation', { ...opts, ...(opts.tickMs === undefined && tick > 0 ? { tickMs: tick } : {}) });
  }
  return new ProductEngine(ledger, opts).asApp();
}

class ProductEngine {
  private readonly listeners = new Set<(e: EngineEvent) => void>();
  private readonly attached = new Set<string>();
  private readonly unsubs: Array<() => void> = [];
  private readonly activeBy = new Map<string, OptimizationId[]>();
  private lastActive = false;
  private lastIds = '';
  private disposed = false;

  private readonly sims = new Map<SimulationProject, Promise<SimulationRuntime>>();
  private readonly simRoots = new Map<string, SimulationProject>();
  private readonly lives = new Map<string, { promise: Promise<LiveRuntime>; stale: boolean; key: string }>();
  private readonly gitHosts = new Map<string, GitHubGitHost>();
  /** Runtimes that have no cloud runner image: fine for local agents, a blocker for "Cloud (unattended)". */
  private readonly noRunnerImage = new WeakMap<LiveRuntime, string>();
  private readonly repoRoot: string;
  private readonly workRoot: string;

  constructor(
    private readonly ledger: Ledger,
    private readonly opts: CreateEngineOptions,
  ) {
    this.repoRoot = resolve(opts.repoRoot ?? process.env.RATCHET_REPO_ROOT ?? join(dirname(fileURLToPath(import.meta.url)), '../../../..'));
    this.workRoot = resolve(opts.workRoot ?? join(this.repoRoot, '.ratchet-app-sim', String(Date.now())));
    // An environment that outlived the previous run of the app must still be watched and removed after its idle window.
    if (opts.live && ledger.listSandboxes().some((r) => r.provider === 'aws' && r.status === 'applied')) void this.live(null).catch(() => undefined);
  }

  // ------------------------------------------------------------------ routing

  private isSimRepo(repo: string): boolean {
    return slugOf(repo) === null;
  }

  private simProjectFor(repo: string): SimulationProject {
    return this.simRoots.get(repo) ?? [...this.sims.keys()][0] ?? 'search';
  }

  private async sim(project: SimulationProject): Promise<SimulationRuntime> {
    let p = this.sims.get(project);
    if (!p) {
      const root = join(this.workRoot, project);
      mkdirSync(root, { recursive: true });
      const idleMs = this.opts.simIdleMs ?? (Number(process.env.RATCHET_SIM_IDLE_MS) || undefined);
      p = (this.opts.createSimulation ?? createSimulationRuntime)({
        ledger: this.ledger,
        repoRoot: this.repoRoot,
        workRoot: root,
        project,
        // The demo environment is automatic too: created at the first run, removed after the idle window, with a receipt.
        settings: { sandbox: { auto: true, sandboxId: () => `ratchet-sim-${project}`, ...(idleMs ? { idleMs } : {}) } },
      });
      this.sims.set(project, p);
      p.then(
        (rt) => {
          this.simRoots.set(rt.repoUrl, project);
          this.attach(`sim:${project}`, rt.engine);
          void rt.engine.run(this.ac.signal);
        },
        () => this.sims.delete(project),
      );
    }
    return p;
  }

  private readonly ac = new AbortController();

  /** The runtime for one GitHub repo (`null`: no repo, only to watch/remove an environment that is already there). */
  private async live(slug: string | null): Promise<LiveRuntime> {
    const cfg = this.opts.live;
    if (!cfg) throw new Error('Real runs are not set up in this build. Use a demo, or start the app normally.');
    const { settings } = cfg;
    const s = settings.live;
    const token = settings.getSecret('githubToken');
    if (slug && !token) throw new Error('Sign in with GitHub first (Settings).');
    const region = regionForOptimization(slug ? cfg.configFor?.(slug) : undefined, s.awsRegion || 'us-east-1');
    const key = [slug ?? '*', region, s.awsProfile, s.runnerImageRef, s.benchImageRef, s.tofuBinary, s.moduleDir, token ? 'tok' : ''].join('|');
    const existing = this.lives.get(slug ?? '*');
    if (existing && !existing.stale && existing.key === key) return existing.promise;
    if (existing) {
      const old = await existing.promise.catch(() => null);
      if (old && !old.engine.runActive()) void old.dispose().catch(() => undefined);
      else if (old) return old; // a run is active on it: keep it until it finishes, then the next call rebuilds
    }
    const promise = this.buildLive(cfg, slug, region, token);
    this.lives.set(slug ?? '*', { promise, stale: false, key });
    promise.then(
      (rt) => {
        this.attach(`live:${slug ?? '*'}`, rt.engine);
        void rt.engine.run(this.ac.signal);
      },
      () => this.lives.delete(slug ?? '*'),
    );
    return promise;
  }

  private async buildLive(cfg: LiveAppConfig, slug: string | null, region: string, token: string | null): Promise<LiveRuntime> {
    const { settings } = cfg;
    const s = settings.live;
    const paths = effectivePaths(s, cfg.defaults);
    mkdirSync(cfg.stateRoot, { recursive: true });
    const { harnesses, loginPath } = await cfg.harnesses.get();
    let runnerImageRef = s.runnerImageRef || undefined;
    let benchImageRef = s.benchImageRef || undefined;
    // The AMIs are found by tag in the repo's region, never typed. Needs AWS credentials. The evaluator's (bench) image is required;
    // the cloud runner image only for "Cloud (unattended)" runs, so a miss there is noted but does not block local agents.
    const canLookUp = !!cfg.appVersion && (s.awsProfile !== '' || !!process.env['AWS_ACCESS_KEY_ID']);
    let imageProblem: string | undefined;
    let runnerProblem: string | undefined;
    const lookup = cfg.resolveImages ?? resolveImageRefs;
    const find = (need: Array<'bench' | 'runner'>) =>
      lookup({ appVersion: cfg.appVersion!, region, need, override: { runner: runnerImageRef, bench: benchImageRef }, profile: s.awsProfile || undefined });
    if (canLookUp && !benchImageRef) {
      try {
        benchImageRef = (await find(['bench'])).bench;
      } catch (err) {
        imageProblem = err instanceof ImageLookupError ? err.message : `Could not look up the evaluator image: ${(err as Error).message.split('\n')[0]}`;
      }
    }
    if (canLookUp && !runnerImageRef) {
      try {
        runnerImageRef = (await find(['runner'])).runner;
      } catch (err) {
        runnerProblem = err instanceof ImageLookupError ? err.message : `Could not look up the cloud runner image: ${(err as Error).message.split('\n')[0]}`;
      }
    }
    const options: HybridOptions = {
      ledger: this.ledger,
      region,
      sandbox: null,
      sandboxId: 'ratchet-sandbox',
      githubRepo: slug ?? undefined,
      githubToken: token ?? undefined,
      runnerImageRef,
      benchImageRef,
      appVersion: cfg.appVersion,
      tofuBinary: paths.tofuBinary,
      moduleDir: paths.moduleDir,
      stateRoot: cfg.stateRoot,
      secrets: secretSource(settings),
      awsProfile: s.awsProfile || undefined,
      harnesses,
      loginPath,
    };
    const rt = await (cfg.createRuntime ?? createHybridRuntime)(options);
    if (imageProblem) rt.missing.push(imageProblem);
    if (runnerProblem || !runnerImageRef) this.noRunnerImage.set(rt, runnerProblem ?? 'The cloud runner image was not found in your AWS account (use "This Mac", or build the image)');
    return rt;
  }

  /** Mark every real runtime stale (settings or sign-in changed): idle ones are rebuilt on next use, busy ones when they finish. */
  invalidate(): void {
    for (const v of this.lives.values()) v.stale = true;
  }

  /** What still blocks a real run on this repo ("what's missing"), after building its runtime. */
  async missingFor(slug: string): Promise<string[]> {
    const rt = await this.live(slug);
    return friendlyMissing(rt.missing);
  }

  // ------------------------------------------------------------------ events

  private attach(key: string, engine: { subscribe(l: (e: EngineEvent) => void): () => void }): void {
    if (this.attached.has(key)) return;
    this.attached.add(key);
    this.unsubs.push(
      engine.subscribe((e) => {
        if (e.type === 'run-active') {
          this.activeBy.set(key, e.active ? e.optimizationIds : []);
          const ids = [...new Set([...this.activeBy.values()].flat())].sort();
          const active = ids.length > 0;
          const sig = ids.join(',');
          if (active === this.lastActive && sig === this.lastIds) return;
          this.lastActive = active;
          this.lastIds = sig;
          this.emit({ type: 'run-active', active, optimizationIds: ids });
          return;
        }
        this.emit(e);
      }),
    );
  }

  private emit(e: EngineEvent): void {
    for (const l of this.listeners) l(e);
  }

  // ------------------------------------------------------------------ routing by optimization / sandbox

  private async engineForOptimization(id: OptimizationId): Promise<EngineApi> {
    const repo = this.ledger.getOptimization(id)?.repo;
    if (repo === undefined) throw new Error(`optimization ${id} not found`);
    const slug = slugOf(repo);
    if (slug === null) return (await this.sim(this.simProjectFor(repo))).engine;
    return (await this.live(slug)).engine;
  }

  private async engineForSandbox(sandboxId: string): Promise<EngineApi> {
    const row = this.ledger.getSandbox(sandboxId);
    if (row?.provider === 'mock' || (!row && sandboxId.startsWith('ratchet-sim'))) {
      const project = (/^ratchet-sim-(\w+)$/.exec(sandboxId)?.[1] as SimulationProject | undefined) ?? [...this.sims.keys()][0] ?? 'search';
      return (await this.sim(project)).engine;
    }
    const firstLive = [...this.lives.values()][0];
    return (firstLive ? await firstLive.promise : await this.live(null)).engine;
  }

  // ------------------------------------------------------------------ the AppEngine

  asApp(): AppEngine {
    const byOpt = <T>(id: OptimizationId, f: (e: EngineApi) => Promise<T>): Promise<T> => this.engineForOptimization(id).then(f);
    const engine: EngineApi = {
      mode: () => 'live',
      createOptimization: async (input: EngineCreateOptimizationInput) => {
        const slug = slugOf(input.repoUrl);
        if (!slug) throw new Error(`${input.repoUrl} is not a GitHub repository (owner/name).`);
        const rt = await this.live(slug);
        const missing = friendlyMissing(rt.missing);
        const cloud = (input.agentsWhere ?? rt.agentsWhere) === 'cloud';
        if (cloud && this.noRunnerImage.has(rt)) missing.push(this.noRunnerImage.get(rt)!);
        if (missing.length > 0) throw new Error(`Not ready: ${missing.join('; ')}`);
        // The runtime is scoped to this repo; its remote is what the ledger records.
        return rt.engine.createOptimization({ ...input, repoUrl: rt.repoUrl ?? input.repoUrl, agentsWhere: input.agentsWhere ?? rt.agentsWhere });
      },
      start: (id) => byOpt(id, (e) => e.start(id)),
      stop: (id, reason) => byOpt(id, (e) => e.stop(id, reason)),
      pauseAgent: async (agentId) => {
        const opt = this.ledger.getAgent(agentId)?.optimizationId;
        if (!opt) throw new Error(`agent ${agentId} not found`);
        return byOpt(opt, (e) => e.pauseAgent(agentId));
      },
      resumeAgent: async (agentId) => {
        const opt = this.ledger.getAgent(agentId)?.optimizationId;
        if (!opt) throw new Error(`agent ${agentId} not found`);
        return byOpt(opt, (e) => e.resumeAgent(agentId));
      },
      messageAgent: async (agentId, text) => {
        const opt = this.ledger.getAgent(agentId)?.optimizationId;
        if (!opt) throw new Error(`agent ${agentId} not found`);
        return byOpt(opt, (e) => e.messageAgent(agentId, text));
      },
      openPullRequest: (id) => byOpt(id, (e) => e.openPullRequest(id)),
      teardown: (id) => byOpt(id, (e) => e.teardown(id)),
      calibration: (id) => byOpt(id, async (e): Promise<Calibration | null> => (e.calibration ? e.calibration(id) : null)),
      protectedPaths: (id) => byOpt(id, (e) => (e.protectedPaths as (id: string) => Promise<EffectiveProtected>)(id)),
      proposeTemplate: async (input: ProposeTemplateInput): Promise<ProposeTemplateResult> => {
        const slug = slugOf(input.repoUrl);
        if (!slug) throw new Error(`${input.repoUrl} is not a GitHub repository (owner/name).`);
        const rt = await this.live(slug);
        if (!rt.engine.proposeTemplate) throw new Error('This engine cannot open template pull requests.');
        return rt.engine.proposeTemplate({ ...input, repoUrl: rt.repoUrl ?? input.repoUrl });
      },
      sandboxState: async (id) => (await this.engineForSandbox(id)).sandboxState(id),
      planSandbox: async (i) => (await this.engineForSandbox(i.sandboxId)).planSandbox(i),
      applySandbox: async (i) => (await this.engineForSandbox(i.sandboxId)).applySandbox(i),
      destroySandbox: async (id) => (await this.engineForSandbox(id)).destroySandbox(id),
      teardownReceipt: async (id): Promise<SandboxTeardownReceipt | null> => {
        const e = await this.engineForOptimization(id).catch(() => null);
        return e?.teardownReceipt ? e.teardownReceipt(id) : null;
      },
      sandboxActivity: async (id): Promise<SandboxActivity> => {
        const e = await this.engineForSandbox(id);
        if (!e.sandboxActivity) throw new Error('This engine has no sandbox activity.');
        return e.sandboxActivity(id);
      },
      endSandbox: async (id): Promise<SandboxTeardownReceipt> => {
        const e = await this.engineForSandbox(id);
        if (!e.endSandbox) throw new Error('This engine cannot end the sandbox.');
        return e.endSandbox(id);
      },
      runActive: () => this.lastActive,
      subscribe: (l) => (this.listeners.add(l), () => this.listeners.delete(l)),
    };
    return {
      kind: 'real',
      engine,
      createDemo: async (demo, input) => {
        const rt = await this.sim(DEMO_PROJECTS[demo]);
        // The simulation's scripted agents are its runner port; the engine picks that, there is no real local control to choose.
        const { agentsWhere: _ignored, ...rest } = input;
        return rt.engine.createOptimization({ ...rest, repoUrl: rt.repoUrl, baseRef: rt.baseRef });
      },
      missingFor: (slug) => this.missingFor(slug),
      invalidate: () => this.invalidate(),
      diff: async ({ baseRef, sha, repo }): Promise<FileDiff[] | null> => {
        const slug = slugOf(repo);
        if (slug === null) return (await this.sim(this.simProjectFor(repo))).git.diff(baseRef, sha);
        const token = this.opts.live?.settings.getSecret('githubToken');
        if (!token || !this.opts.live) return null;
        let git = this.gitHosts.get(slug);
        if (!git) {
          git = new GitHubGitHost({ repo: slug, token, cacheDir: join(this.opts.live.stateRoot, 'git-cache-diff', slug.replace('/', '__')) });
          this.gitHosts.set(slug, git);
        }
        return git.diff(baseRef, sha);
      },
      dispose: () => {
        if (this.disposed) return;
        this.disposed = true;
        this.ac.abort();
        for (const u of this.unsubs) u();
        for (const p of this.sims.values()) void p.then((rt) => rt.dispose()).catch(() => undefined);
        for (const v of this.lives.values()) void v.promise.then((rt) => rt.dispose()).catch(() => undefined);
      },
    };
  }
}
