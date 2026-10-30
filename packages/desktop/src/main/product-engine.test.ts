import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { HarnessInfo } from '@ratchet/agents';
import { openLedger, parseConfig, CALIBRATION, type EngineApi, type EngineEvent, type Ledger } from '@ratchet/core';
import type { HybridOptions, LiveRuntime, SimulationRuntime } from '@ratchet/runtime';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_CONFIG_YAML } from './default-config.js';
import { configColumns } from './dev-engine.js';
import { createEngineForApp, friendlyMissing, slugOf, type AppEngine, type CreateEngineOptions } from './engine.js';
import { HarnessCache } from './harnesses.js';
import { SettingsStore } from './settings.js';

const cipher = { isAvailable: () => true, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };

/** An engine that only records what it was asked and writes the optimization row the router looks at. */
class FakeEngine {
  calls: string[] = [];
  active = false;
  private listeners = new Set<(e: EngineEvent) => void>();
  proposals: unknown[] = [];
  constructor(readonly name: string, private readonly ledger: Ledger) {}
  mode = () => 'live' as const;
  subscribe(l: (e: EngineEvent) => void) {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }
  emit(e: EngineEvent) {
    this.listeners.forEach((l) => l(e));
  }
  runActive = () => this.active;
  runs = 0;
  async run() { this.runs++; }
  async createOptimization(input: { repoUrl: string; baseRef: string; agents: Array<'claude-code' | 'codex'>; agentsWhere?: string }) {
    this.calls.push(`create:${input.repoUrl}:${input.agentsWhere ?? '-'}`);
    const cfg = parseConfig(DEFAULT_CONFIG_YAML);
    if (!cfg.ok) throw new Error('bad default config');
    const opt = this.ledger.createOptimization({
      name: `${this.name} run`, repo: input.repoUrl, baseRef: input.baseRef, metric: cfg.config.objective.metric, direction: 'minimize', minGain: CALIBRATION.fallbackGain,
      budgetUsd: 10, timeLimitS: 600, configYaml: DEFAULT_CONFIG_YAML, status: 'draft', ...configColumns(cfg.config), sandboxId: null,
    });
    input.agents.forEach((kind, i) => this.ledger.addAgent({ optimizationId: opt.id, kind, branch: `b/${kind}-${i}` }));
    return opt.id;
  }
  async start(id: string) { this.calls.push(`start:${id}`); }
  async stop(id: string) { this.calls.push(`stop:${id}`); }
  async pauseAgent(id: string) { this.calls.push(`pause:${id}`); }
  async resumeAgent(id: string) { this.calls.push(`resume:${id}`); }
  async messageAgent(id: string, t: string) { this.calls.push(`message:${id}:${t}`); }
  async openPullRequest(id: string) { this.calls.push(`pr:${id}`); return { url: `https://github.com/x/y/pull/${this.name}` }; }
  async teardown(id: string) { this.calls.push(`teardown:${id}`); }
  async teardownReceipt(id: string) { this.calls.push(`receipt:${id}`); return null; }
  async calibration(id: string) { this.calls.push(`calibration:${id}`); return null; }
  async sandboxState(id: string) { this.calls.push(`state:${id}`); return {} as never; }
  async planSandbox() { return {} as never; }
  async applySandbox() { return {} as never; }
  async destroySandbox() {}
  async sandboxActivity(id: string) { this.calls.push(`activity:${id}`); return { sandboxId: id, status: 'applied', region: 'us-east-1', auto: true, activeRuns: [], instancesRunning: 0, spendUsd: 0, idleSince: null, destroyAt: null, lastReceipt: null } as never; }
  async endSandbox(id: string) { this.calls.push(`end:${id}`); return {} as never; }
  async proposeTemplate(input: unknown) { this.calls.push('propose'); this.proposals.push(input); return { prUrl: 'https://github.com/acme/api/pull/9', branch: 'ratchet/template-x', sha: 'abc', templateId: 'http-latency', files: ['ratchet.yaml'] }; }
  async protectedPaths() { return {} as never; }
  asEngine = () => this as unknown as EngineApi & { close(): void };
  close() {}
}

const harness = (kind: HarnessInfo['kind']): HarnessInfo => ({ kind, installed: true, version: '2.0.0', path: `/bin/${kind}`, auth: 'signed-in', hint: 'ok' });

interface Built {
  app: AppEngine;
  ledger: Ledger;
  settings: SettingsStore;
  live: Array<{ options: HybridOptions; engine: FakeEngine; rt: LiveRuntime; disposed: boolean }>;
  sims: Array<{ project: string; settings: unknown; engine: FakeEngine; disposed: boolean }>;
  imageCalls: unknown[];
  stateRoot: string;
  configs: Map<string, unknown>;
  setMissing(m: string[]): void;
  setResolver(f: (a: unknown) => Promise<unknown>): void;
}

const roots: string[] = [];
beforeEach(() => vi.stubEnv('AWS_ACCESS_KEY_ID', ''));
afterEach(() => {
  vi.unstubAllEnvs();
  roots.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true }));
});

function build(extra: Partial<CreateEngineOptions> = {}, opts: { token?: boolean; seedAwsSandbox?: boolean; aws?: boolean } = {}): Built {
  const stateRoot = mkdtempSync(join(tmpdir(), 'ratchet-pe-'));
  roots.push(stateRoot);
  const ledger = openLedger(':memory:');
  const settings = new SettingsStore(null, cipher, {});
  if (opts.token ?? true) settings.setSecret('githubToken', 'gho_TOKENTOKENTOKENTOKEN1234');
  if (opts.aws ?? true) settings.setAwsProfile('sandbox');
  if (opts.seedAwsSandbox) {
    ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'us-east-1', status: 'applied', outputs: { artifactsBucket: 'x' } } as never);
  }
  const live: Built['live'] = [];
  const sims: Built['sims'] = [];
  const imageCalls: unknown[] = [];
  const configs = new Map<string, unknown>();
  let missing: string[] = [];
  let resolver: (a: unknown) => Promise<unknown> = async (a) => {
    imageCalls.push(a);
    return { bench: 'ami-0bench00000000000a', source: { bench: 'tag' } };
  };
  const harnesses = new HarnessCache({ loginPath: async () => '/login/bin', detect: async () => [harness('claude-code'), { ...harness('codex'), auth: 'signed-out' }] });
  const app = createEngineForApp(ledger, {
    workRoot: join(stateRoot, 'sim'),
    repoRoot: stateRoot,
    ...extra,
    live: {
      settings, stateRoot, defaults: { tofuBinary: 'tofu', moduleDir: '/m' }, harnesses, appVersion: '1.2.3', configFor: (s) => configs.get(s),
      createRuntime: async (options) => {
        const engine = new FakeEngine(`live${live.length}`, ledger);
        const entry = { options, engine, disposed: false, rt: null as unknown as LiveRuntime };
        entry.rt = { engine: engine.asEngine(), repoUrl: options.githubRepo ? `https://github.com/${options.githubRepo}.git` : null, missing: [...missing], agentsWhere: 'local', async dispose() { entry.disposed = true; } } as unknown as LiveRuntime;
        live.push(entry);
        return entry.rt;
      },
      resolveImages: ((a: unknown) => resolver(a)) as never,
    },
    createSimulation: (async (o: { project?: string; settings?: unknown }) => {
      const engine = new FakeEngine(`sim-${o.project}`, ledger);
      const entry = { project: o.project!, settings: o.settings, engine, disposed: false };
      sims.push(entry);
      return {
        engine: engine.asEngine(), repoUrl: `/work/${o.project}/remote.git`, baseRef: 'main', git: { diff: async () => [{ path: 'a.ts', additions: 1, deletions: 0, binary: false, patch: '@@' }] }, run: async () => undefined,
        async dispose() { entry.disposed = true; },
      } as unknown as SimulationRuntime;
    }) as never,
  });
  return { app, ledger, settings, live, sims, imageCalls, stateRoot, configs, setMissing: (m) => { missing = m; }, setResolver: (f) => { resolver = f; } };
}

const input = (repoUrl: string) => ({ repoUrl, baseRef: 'main', agents: ['claude-code' as const], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 5 });

describe('slugOf', () => {
  it.each([
    ['acme/api', 'acme/api'],
    ['https://github.com/acme/api.git', 'acme/api'],
    ['git@github.com:acme/api', 'acme/api'],
    ['/work/search/remote.git', null],
    ['C:\\x\\y', null],
  ])('%s -> %s', (repo, slug) => expect(slugOf(repo)).toBe(slug));
});

describe('the product engine: real runs', () => {
  it('builds one hybrid runtime per repo from Settings, the cached harnesses and the GitHub token (main process only)', async () => {
    const b = build();
    b.configs.set('acme/api', { environment: { region: 'eu-west-2' } });
    const id = await b.app.engine.createOptimization({ ...input('acme/api'), agentsWhere: 'local' });
    expect(b.live).toHaveLength(1);
    const o = b.live[0]!.options;
    expect(o).toMatchObject({
      githubRepo: 'acme/api', githubToken: 'gho_TOKENTOKENTOKENTOKEN1234', region: 'eu-west-2', awsProfile: 'sandbox', sandbox: null, appVersion: '1.2.3',
      loginPath: '/login/bin', benchImageRef: 'ami-0bench00000000000a', tofuBinary: 'tofu', moduleDir: '/m',
    });
    expect(o.harnesses?.map((h) => [h.kind, h.auth])).toEqual([['claude-code', 'signed-in'], ['codex', 'signed-out']]);
    expect(b.imageCalls[0]).toMatchObject({ appVersion: '1.2.3', region: 'eu-west-2', need: ['bench'], profile: 'sandbox' });
    expect(b.imageCalls[1]).toMatchObject({ need: ['runner'] });
    // the ledger records the runtime's remote; the second call reuses the runtime
    expect(b.ledger.getOptimization(id)?.repo).toBe('https://github.com/acme/api.git');
    await b.app.engine.createOptimization({ ...input('https://github.com/acme/api.git') });
    expect(b.live).toHaveLength(1);
    await b.app.engine.createOptimization({ ...input('acme/other') });
    expect(b.live).toHaveLength(2);
    b.app.dispose();
  });

  it('routes commands to the runtime of the optimization, by the repo in the ledger', async () => {
    const b = build();
    const a = await b.app.engine.createOptimization(input('acme/api'));
    const o = await b.app.engine.createOptimization(input('acme/other'));
    await b.app.engine.start(o);
    await b.app.engine.stop(a, 'x');
    await b.app.engine.openPullRequest(a);
    const agent = b.ledger.listAgents(o)[0]!.id;
    await b.app.engine.pauseAgent(agent);
    await b.app.engine.messageAgent(agent, 'hi');
    expect(b.live[0]!.engine.calls.filter((c) => !c.startsWith('create'))).toEqual([`stop:${a}`, `pr:${a}`]);
    expect(b.live[1]!.engine.calls.filter((c) => !c.startsWith('create'))).toEqual([`start:${o}`, `pause:${agent}`, `message:${agent}:hi`]);
    b.app.dispose();
  });

  it('refuses without a GitHub sign-in, and says what is missing when the runtime is not ready', async () => {
    const none = build({}, { token: false });
    await expect(none.app.engine.createOptimization(input('acme/api'))).rejects.toThrow(/Sign in with GitHub/);
    const b = build();
    b.setMissing(['set the bench AMI (Settings)', 'install and sign in to Claude Code or Codex on this machine (Settings > Agents)']);
    await expect(b.app.engine.createOptimization(input('acme/api'))).rejects.toThrow(/Not ready: The evaluator image for this app version was not found.*Install and sign in to Claude Code or Codex/s);
    expect(await b.app.missingFor!('acme/api')).toHaveLength(2);
    expect(friendlyMissing(['something else'])).toEqual(['something else']);
    b.app.dispose();
  });

  it('a repo that is not on GitHub is rejected by name', async () => {
    const b = build();
    await expect(b.app.engine.createOptimization(input('/home/me/project'))).rejects.toThrow(/not a GitHub repository/);
    b.app.dispose();
  });

  it('skips the image lookup without AWS credentials, and keeps an explicit AMI override', async () => {
    const noAws = build({}, { aws: false });
    await noAws.app.engine.createOptimization(input('acme/api'));
    expect(noAws.imageCalls).toHaveLength(0);
    expect(noAws.live[0]!.options.benchImageRef).toBeUndefined();
    const over = build();
    over.settings.setLive({ benchImageRef: 'ami-0123456789abcdef0' });
    await over.app.engine.createOptimization(input('acme/api'));
    expect(over.imageCalls).toHaveLength(1); // only the runner image is looked up
    expect(over.imageCalls[0]).toMatchObject({ need: ['runner'] });
    expect(over.live[0]!.options.benchImageRef).toBe('ami-0123456789abcdef0');
  });

  it('a failed image lookup becomes a "what is missing" line, not a crash', async () => {
    const { ImageLookupError } = await import('@ratchet/aws');
    const b = build();
    b.setResolver(async () => {
      throw new ImageLookupError('bench', '1.2.3', 'us-east-1', 'x86_64', ['0.0.1']);
    });
    await expect(b.app.engine.createOptimization(input('acme/api'))).rejects.toThrow(/Not ready: .*1\.2\.3/s);
    expect(b.live).toHaveLength(1);
    b.app.dispose();
  });

  it('a missing cloud runner image only blocks Cloud (unattended) runs, not this-machine runs', async () => {
    const { ImageLookupError } = await import('@ratchet/aws');
    const b = build();
    b.setResolver(async (a) => {
      if ((a as { need: string[] }).need[0] === 'runner') throw new ImageLookupError('runner', '1.2.3', 'us-east-1', 'x86_64', []);
      return { bench: 'ami-0bench00000000000a' };
    });
    await expect(b.app.engine.createOptimization({ ...input('acme/api'), agentsWhere: 'local' })).resolves.toMatch(/^opt_/);
    await expect(b.app.engine.createOptimization({ ...input('acme/api'), agentsWhere: 'cloud' })).rejects.toThrow(/Not ready: .*runner/s);
    b.app.dispose();
  });

  it('proposeTemplate goes to the runtime of that repo, with the owner/name', async () => {
    const b = build();
    const r = await b.app.engine.proposeTemplate!({ repoUrl: 'https://github.com/acme/api', baseRef: 'main', templateId: 'http-latency' });
    expect(r.prUrl).toBe('https://github.com/acme/api/pull/9');
    expect(b.live[0]!.options.githubRepo).toBe('acme/api');
    expect(b.live[0]!.engine.proposals[0]).toMatchObject({ repoUrl: 'https://github.com/acme/api.git', templateId: 'http-latency' });
    b.app.dispose();
  });

  it('a changed setting rebuilds an idle runtime on next use, but never one that is running', async () => {
    const b = build();
    await b.app.engine.createOptimization(input('acme/api'));
    b.app.invalidate!();
    await b.app.engine.createOptimization(input('acme/api'));
    expect(b.live).toHaveLength(2);
    expect(b.live[0]!.disposed).toBe(true);
    b.live[1]!.engine.active = true;
    b.app.invalidate!();
    await b.app.engine.createOptimization(input('acme/api'));
    expect(b.live).toHaveLength(2);
    expect(b.live[1]!.disposed).toBe(false);
    b.app.dispose();
    await Promise.resolve();
  });

  it('watches an environment that outlived the last session even with no repo open', async () => {
    const b = build({}, { seedAwsSandbox: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(b.live).toHaveLength(1);
    expect(b.live[0]!.options.githubRepo).toBeUndefined();
    const act = await b.app.engine.sandboxActivity!('ratchet-sandbox');
    expect(act.region).toBe('us-east-1');
    await b.app.engine.endSandbox!('ratchet-sandbox');
    expect(b.live[0]!.engine.calls).toEqual(['activity:ratchet-sandbox', 'end:ratchet-sandbox']);
    b.app.dispose();
  });
});

describe('the product engine: demos', () => {
  it('starts a demo in a simulation runtime with an automatic environment, created once per project', async () => {
    const b = build();
    const id = await b.app.createDemo!('demo-search', input('acme/demo-search'));
    expect(b.sims).toHaveLength(1);
    expect(b.sims[0]).toMatchObject({ project: 'search', settings: { sandbox: { auto: true } } });
    expect(b.live).toHaveLength(0);
    expect(b.ledger.getOptimization(id)?.repo).toBe('/work/search/remote.git');
    expect(b.sims[0]!.engine.calls[0]).toBe('create:/work/search/remote.git:local');
    await b.app.createDemo!('demo-search', input('x'));
    expect(b.sims).toHaveLength(1);
    await b.app.createDemo!('demo-cli', input('x'));
    await b.app.createDemo!('demo-forecast', input('x'));
    expect(b.sims.map((s) => s.project)).toEqual(['search', 'cli', 'forecast']);
    // each demo project has its own environment id
    const idOf = (i: number) => (b.sims[i]!.settings as { sandbox: { sandboxId(): string } }).sandbox.sandboxId();
    expect(new Set([0, 1, 2].map(idOf)).size).toBe(3);
    b.app.dispose();
    await new Promise((r) => setTimeout(r, 5));
    expect(b.sims.every((s) => s.disposed)).toBe(true);
  });

  it('routes a demo optimization to its simulation runtime and diffs with its git', async () => {
    const b = build();
    const id = await b.app.createDemo!('demo-cli', input('x'));
    await b.app.engine.start(id);
    await b.app.engine.stop(id);
    expect(b.sims[0]!.engine.calls).toEqual([`create:/work/cli/remote.git:local`, `start:${id}`, `stop:${id}`]);
    const diff = await b.app.diff!({ optimizationId: id, repo: '/work/cli/remote.git', baseRef: 'main', sha: 'abc' });
    expect(diff?.[0]?.path).toBe('a.ts');
    b.app.dispose();
  });

  it('works without any sign-in or AWS: a demo needs nothing', async () => {
    const b = build({}, { token: false, aws: false });
    await expect(b.app.createDemo!('demo-search', input('x'))).resolves.toMatch(/^opt_/);
    b.app.dispose();
  });
});

describe('the product engine: events', () => {
  it('merges run-active from every runtime: active while any runs, off when all are done; forwards the rest', async () => {
    const b = build();
    const seen: EngineEvent[] = [];
    b.app.engine.subscribe((e) => seen.push(e));
    await b.app.engine.createOptimization(input('acme/api'));
    await b.app.createDemo!('demo-search', input('x'));
    const [live, sim] = [b.live[0]!.engine, b.sims[0]!.engine];
    live.emit({ type: 'run-active', active: true, optimizationIds: ['opt_a'] });
    sim.emit({ type: 'run-active', active: true, optimizationIds: ['opt_b'] });
    live.emit({ type: 'run-active', active: false, optimizationIds: [] });
    sim.emit({ type: 'run-active', active: false, optimizationIds: [] });
    sim.emit({ type: 'sandbox', sandboxId: 'ratchet-sim-search', phase: 'ready', region: 'us-east-1', text: 'environment ready' });
    expect(seen.filter((e) => e.type === 'run-active')).toEqual([
      { type: 'run-active', active: true, optimizationIds: ['opt_a'] },
      { type: 'run-active', active: true, optimizationIds: ['opt_a', 'opt_b'] },
      { type: 'run-active', active: true, optimizationIds: ['opt_b'] },
      { type: 'run-active', active: false, optimizationIds: [] },
    ]);
    expect(seen.at(-1)).toMatchObject({ type: 'sandbox', phase: 'ready' });
    expect(b.app.engine.runActive?.()).toBe(false);
    b.app.dispose();
  });
});
