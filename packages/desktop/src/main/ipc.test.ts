import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { openLedger } from '@ratchet/core';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC } from '../shared/channels.js';
import { ARGS } from '../shared/ipc.js';
import { createEngineForApp } from './engine.js';
import { IpcValidationError, isTrustedUrl, registerIpc, type IpcMainLike } from './ipc.js';
import { createService, type RatchetService } from './service.js';
import { SettingsStore } from './settings.js';

type Handler = (event: unknown, ...args: unknown[]) => unknown;

class FakeIpc implements IpcMainLike {
  handlers = new Map<string, Handler>();
  handle(channel: string, fn: Handler): void {
    this.handlers.set(channel, fn);
  }
  invoke(channel: string, ...args: unknown[]): Promise<unknown> {
    const h = this.handlers.get(channel);
    if (!h) throw new Error(`no handler for ${channel}`);
    return Promise.resolve(h({}, ...args));
  }
}

const cipher = { isAvailable: () => true, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };

function setup(trusted?: (e: unknown) => boolean) {
  const ledger = openLedger(':memory:');
  const settings = new SettingsStore(null, cipher, {});
  const service = createService({ ledger, settings, createApp: () => createEngineForApp(ledger, { tickMs: 600_000, engine: 'dev' }) });
  const ipc = new FakeIpc();
  registerIpc(ipc, service, trusted ? { isTrustedSender: trusted } : {});
  return { ledger, settings, service, ipc };
}

describe('registerIpc', () => {
  let ctx: ReturnType<typeof setup>;
  beforeEach(() => {
    ctx = setup();
  });

  it('registers a handler for every method in the schema table', () => {
    for (const method of Object.keys(ARGS)) expect(ctx.ipc.handlers.has(IPC[method as keyof typeof ARGS])).toBe(true);
    expect(ctx.ipc.handlers.size).toBe(Object.keys(ARGS).length);
  });

  it('the preload bridge exposes exactly the IPC methods, and none that reads a secret', () => {
    const src = readFileSync(join(__dirname, '../preload/index.ts'), 'utf8');
    const list = /const methods: Invoke\[\] = \[([\s\S]*?)\];/.exec(src)![1]!;
    const exposed = [...list.matchAll(/'(\w+)'/g)].map((m) => m[1]!).sort();
    expect(exposed).toEqual(Object.keys(ARGS).sort());
    expect(exposed.filter((m) => /secret/i.test(m) && !/^(setSecret|clearSecret)$/.test(m))).toEqual([]);
    // the bridge forwards through ipcRenderer.invoke only; no raw ipcRenderer or Node handle is exposed
    expect(src).not.toMatch(/exposeInMainWorld\('ratchet',\s*\{?\s*ipcRenderer/);
  });

  it('serves reads from the ledger', async () => {
    const snap = (await ctx.ipc.invoke(IPC.listOptimizations)) as { optimizations: unknown[]; featuredId: string };
    expect(snap.optimizations).toHaveLength(4);
    expect(snap.featuredId).toBe('opt_demo_search');
    const detail = (await ctx.ipc.invoke(IPC.getOptimization, 'opt_demo_search')) as { agents: unknown[] };
    expect(detail.agents).toHaveLength(2);
    // everything crossing IPC must survive structured clone
    expect(structuredClone(snap)).toEqual(snap);
    ctx.service.dispose();
  });

  it('serves the Overview for a range (default 90d) and it survives structured clone', async () => {
    const o = (await ctx.ipc.invoke(IPC.overview, '30d')) as { range: string; rows: { id: string }[]; value: { wins: number } | null };
    expect(o.range).toBe('30d');
    expect(o.rows.map((r) => r.id)).toContain('opt_demo_search');
    expect(o.value!.wins).toBeGreaterThan(0);
    expect(((await ctx.ipc.invoke(IPC.overview)) as { range: string }).range).toBe('90d');
    expect(structuredClone(o)).toEqual(o);
    ctx.service.dispose();
  });

  const bad: [keyof typeof IPC, unknown[]][] = [
    ['overview', ['7d']],
    ['overview', [30]],
    ['overview', ['all', 'extra']],
    ['getOptimization', []],
    ['getOptimization', [42]],
    ['getOptimization', ['']],
    ['getOptimization', ['a', 'extra']],
    ['agentEvents', ['agt', -1]],
    ['agentEvents', ['agt', 1.5]],
    ['agentEvents', ['agt', 0, 100000]],
    ['validateConfig', [{ not: 'a string' }]],
    ['messageAgent', ['agt', '   ']],
    ['messageAgent', ['agt', 'x'.repeat(5000)]],
    ['inspectRepo', [{ kind: 'github', repo: 'not a repo' }]],
    ['inspectRepo', [{ kind: 'github', repo: 'https://evil.example/x/y' }]],
    ['inspectRepo', [{ kind: 'local', path: '' }]],
    ['inspectRepo', [{ kind: 'ftp', path: '/x' }]],
    ['listRepos', [42]],
    ['proposeTemplate', [{ repoUrl: 'a/b', baseRef: 'main', templateId: 'Bad Id' }]],
    ['proposeTemplate', [{ repoUrl: 'a/b', baseRef: 'main', templateId: 'http-latency', files: ['/etc/passwd'] }]],
    ['preflight', [{ agents: ['codex'], agentsWhere: 'moon', source: 'github' }]],
    ['endSandbox', []],
    ['githubSignIn', ['extra']],
    ['setAwsProfile', ['bad profile; rm -rf /']],
    ['setSecret', ['anthropicApiKey', 'short']],
    ['setSecret', ['rootPassword', 'long-enough-secret']],
    ['setSecret', ['anthropicApiKey', 'sk-proj-this-is-an-openai-key']],
    ['setSecret', ['githubToken', 'not-a-github-token']],
    ['saveLiveSettings', [{ awsRegion: 'mars-north-9' }]],
    ['saveLiveSettings', [{ runnerImageRef: 'i-0123456789abcdef0' }]],
    ['saveLiveSettings', [{ githubRepo: 'acme/api' }]],
    ['saveLiveSettings', [{ tofuBinary: 'tofu --version; id' }]],
    ['saveLiveSettings', [{ moduleDir: '../../etc' }]],
    ['saveLiveSettings', [{ awsAuth: 'password' }]],
    ['saveLiveSettings', [{ anthropicApiKey: 'sk-ant-smuggled-through-settings' }]],
    ['saveLiveSettings', [{}, {}]],
    ['testConnection', ['stripe']],
    ['testConnection', ['tofu']],
    ['testConnection', []],
    ['recentActivity', [{ limit: 0 }]],
    ['recentActivity', [{ unknown: 1 }]],
    ['createOptimization', [{ repoUrl: 'x', baseRef: 'main', agents: [], budgetUsd: 10, timeLimitSeconds: 60, plateauEvals: 3 }]],
    ['createOptimization', [{ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: -5, timeLimitSeconds: 60, plateauEvals: 3 }]],
    ['createOptimization', [{ repoUrl: 'x', baseRef: 'main', agents: ['gpt-9'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 }]],
    ['createOptimization', [{ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3, isAdmin: true }]],
    ['createOptimization', [{ repoUrl: '', baseRef: 'main', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 }]],
    ['createOptimization', [{ repoUrl: 'a/b', baseRef: 'main', demo: 'demo-evil', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 }]],
    ['createOptimization', [{ repoUrl: 'a/b', baseRef: 'main', configYaml: 'version: 0', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 }]],
    ['createOptimization', ['not an object']],
  ];
  it.each(bad)('rejects invalid arguments to %s %j before executing anything', async (method, args) => {
    const spy = vi.spyOn(ctx.service, method as 'getOptimization');
    await expect(ctx.ipc.invoke(IPC[method], ...args)).rejects.toBeInstanceOf(IpcValidationError);
    expect(spy).not.toHaveBeenCalled();
    ctx.service.dispose();
  });

  it('accepts a well-formed createOptimization and returns the new id', async () => {
    const id = (await ctx.ipc.invoke(IPC.createOptimization, {
      repoUrl: 'acme/api', baseRef: 'main', agents: ['claude-code'], agentsWhere: 'local', budgetUsd: 25, timeLimitSeconds: 3600, plateauEvals: 10,
    })) as string;
    expect(ctx.ledger.getOptimization(id)?.status).toBe('draft');
    ctx.service.dispose();
  });

  it('a demo needs no repository', async () => {
    const id = (await ctx.ipc.invoke(IPC.createOptimization, {
      demo: 'demo-search', repoUrl: '', baseRef: 'main', agents: ['claude-code', 'codex'], budgetUsd: 50, timeLimitSeconds: 7200, plateauEvals: 25,
    })) as string;
    expect(ctx.ledger.getOptimization(id)?.name).toBe('Search API latency (demo)');
    ctx.service.dispose();
  });

  it('says so when the repository is not a GitHub owner/name', async () => {
    await expect(
      ctx.ipc.invoke(IPC.createOptimization, { repoUrl: '/some/local/path', baseRef: 'main', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 }),
    ).rejects.toThrow(/not a GitHub repository/);
    ctx.service.dispose();
  });

  it('rejects untrusted senders without touching the service', async () => {
    const t = setup(() => false);
    const spy = vi.spyOn(t.service, 'listOptimizations');
    await expect(t.ipc.invoke(IPC.listOptimizations)).rejects.toThrow(/Untrusted sender/);
    expect(spy).not.toHaveBeenCalled();
    t.service.dispose();
  });

  it('isTrustedUrl allows file:// and the dev server origin only', () => {
    expect(isTrustedUrl('file:///app/out/renderer/index.html')).toBe(true);
    expect(isTrustedUrl('http://localhost:5173/#/x', 'http://localhost:5173')).toBe(true);
    expect(isTrustedUrl('http://localhost:5173/', undefined)).toBe(false);
    expect(isTrustedUrl('https://evil.example/', 'http://localhost:5173')).toBe(false);
    expect(isTrustedUrl('not a url', 'http://localhost:5173')).toBe(false);
  });
});

describe('RatchetService commands & events', () => {
  it('forwards engine events and wires commands to the engine', async () => {
    const { service, ledger } = setup();
    const events: string[] = [];
    service.onEvent((e) => events.push(e.type));
    const agent = ledger.listAgents('opt_demo_search')[0]!;
    await service.pauseAgent(agent.id);
    await service.resumeAgent(agent.id);
    await service.messageAgent(agent.id, 'hello');
    expect(events.filter((e) => e === 'agent-event').length).toBeGreaterThanOrEqual(3);
    await service.stopOptimization('opt_demo_search', 'test');
    expect(ledger.getOptimization('opt_demo_search')!.status).toBe('stopped');
    expect(events).toContain('optimization');
    service.dispose();
  });

  it('opens a PR (stored for the UI) and exports a report', async () => {
    const saved: string[] = [];
    const ledger = openLedger(':memory:');
    const settings = new SettingsStore(null, cipher, {});
    const service = createService({
      ledger, settings,
      createApp: () => createEngineForApp(ledger, { tickMs: 600_000, engine: 'dev' }),
      saveReport: async (name, md) => (saved.push(name, md), '/tmp/r.md'),
    });
    const pr = await service.openPullRequest('opt_demo_ingest');
    expect(pr.url).toContain('acme/ingest/pull/');
    expect((await service.getOptimization('opt_demo_ingest')).prUrl).toBe(pr.url);
    expect((await service.getOptimization('opt_demo_ingest')).summary.status).toBe('pr-open');
    expect(await service.exportReport('opt_demo_ingest')).toEqual({ saved: true, path: '/tmp/r.md' });
    expect(saved[1]).toContain('Ratchet report: Ingest throughput');
    // The dev engine has no sandbox: no receipt, and no calibration before a baseline.
    expect((await service.getOptimization('opt_demo_ingest')).receipt).toBeNull();
    service.dispose();
  });

  it('exportReport reports cancellation', async () => {
    const { service } = setup();
    expect(await service.exportReport('opt_demo_search')).toEqual({ saved: false });
    service.dispose();
  });
});
