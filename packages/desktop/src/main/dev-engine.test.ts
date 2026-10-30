import { openLedger, type EngineEvent, type Ledger } from '@ratchet/core';
import { beforeEach, describe, expect, it } from 'vitest';
import { DevEngine, repoSlug } from './dev-engine.js';
import { createEngineForApp } from './engine.js';

let ledger: Ledger;
let t: number;
let engine: DevEngine;

beforeEach(() => {
  t = 1_000_000;
  ledger = openLedger(':memory:');
  engine = new DevEngine(ledger, { mode: 'simulation', seed: 7, autoRun: false, now: () => (t += 500) });
});

const input = { repoUrl: 'https://github.com/acme/search-api.git', baseRef: 'main', agents: ['claude-code', 'codex'] as const, budgetUsd: 500, timeLimitSeconds: 86_400, plateauEvals: 1000 };

async function started() {
  const id = await engine.createOptimization({ ...input, agents: [...input.agents] });
  await engine.start(id);
  return id;
}

describe('DevEngine', () => {
  it('creates a draft with one agent per kind and the parsed objective', async () => {
    const id = await engine.createOptimization({ ...input, agents: [...input.agents] });
    const o = ledger.getOptimization(id)!;
    expect(o).toMatchObject({ status: 'draft', repo: 'acme/search-api', direction: 'minimize', metric: 'http_req_duration.p95', budgetUsd: 500 });
    expect(ledger.listAgents(id).map((a) => a.kind)).toEqual(['claude-code', 'codex']);
  });

  it('rejects an invalid config', async () => {
    await expect(engine.createOptimization({ ...input, agents: ['codex'], configYaml: 'version: 1' })).rejects.toThrow(/invalid ratchet.yaml/);
  });

  it('start records a baseline and moves to running', async () => {
    const id = await started();
    expect(ledger.getOptimization(id)!.status).toBe('running');
    const b = ledger.getBaseline(id)!;
    expect(b.samples.length).toBeGreaterThanOrEqual(5);
    expect(b.noiseFloor).toBeGreaterThan(0);
    expect(ledger.listAgents(id).every((a) => a.status === 'running')).toBe(true);
  });

  it('simulates turns through the real ratchet rule: monotonic ratchet, verdicts, spend, events', async () => {
    const events: EngineEvent[] = [];
    engine.subscribe((e) => events.push(e));
    const id = await started();
    for (let i = 0; i < 40; i++) engine.tick(id);

    const cands = ledger.candidatesWithResults(id);
    expect(cands.length).toBe(80);
    expect(cands.every((c) => c.verdict !== 'pending')).toBe(true);
    const moves = ledger.ratchetHistory(id);
    expect(moves.length).toBeGreaterThan(0);
    for (const m of moves) expect(m.toValue).toBeLessThan(m.fromValue); // minimize
    expect(cands.filter((c) => c.verdict === 'verified')).toHaveLength(moves.length);
    for (const c of cands.filter((x) => x.verdict === 'verified')) expect(c.results.map((r) => r.phase)).toEqual(['bench', 'confirm']);
    expect(ledger.totalSpend(id)).toBeGreaterThan(0);

    const kinds = new Set(events.map((e) => e.type));
    for (const k of ['optimization', 'agent-event', 'candidate', 'ratchet', 'spend']) expect(kinds.has(k as EngineEvent['type'])).toBe(true);
    expect(ledger.agentEvents(ledger.listAgents(id)[0]!.id).some((e) => e.type === 'verdict')).toBe(true);
  });

  it('pause skips an agent, resume brings it back, messages show up as the next hypothesis', async () => {
    const id = await started();
    const [a, b] = ledger.listAgents(id);
    await engine.pauseAgent(a!.id);
    engine.tick(id);
    expect(ledger.candidatesWithResults(id).map((c) => c.agentId)).toEqual([b!.id]);
    expect(ledger.getAgent(a!.id)!.status).toBe('paused');
    await engine.resumeAgent(a!.id);
    await engine.messageAgent(a!.id, 'try a bloom filter');
    engine.tick(id);
    const msgs = ledger.agentEvents(a!.id).filter((e) => e.type === 'message').map((e) => (e.payload as { text: string }).text);
    expect(msgs).toContain('You: try a bloom filter');
    expect(msgs.at(-1)).toBe('Following your note: try a bloom filter');
  });

  it('stops on budget exhaustion and on plateau', async () => {
    const id = await engine.createOptimization({ ...input, agents: ['codex'], budgetUsd: 3 });
    await engine.start(id);
    for (let i = 0; i < 50 && ledger.getOptimization(id)!.status === 'running'; i++) engine.tick(id);
    expect(ledger.getOptimization(id)!.status).toBe('completed');
    expect(ledger.totalSpend(id)).toBeGreaterThanOrEqual(3);

    const p = await engine.createOptimization({ ...input, agents: ['codex'], plateauEvals: 1 });
    await engine.start(p);
    for (let i = 0; i < 200 && ledger.getOptimization(p)!.status === 'running'; i++) engine.tick(p);
    expect(ledger.getOptimization(p)!.status).toBe('completed');
  });

  it('stop is the kill switch: agents stopped, optimization stopped, no further ticks act', async () => {
    const id = await started();
    await engine.stop(id, 'user');
    expect(ledger.getOptimization(id)!.status).toBe('stopped');
    expect(ledger.listAgents(id).every((a) => a.status === 'stopped')).toBe(true);
    engine.tick(id);
    expect(ledger.candidatesWithResults(id)).toHaveLength(0);
  });

  it('opens a PR only when there is a verified win', async () => {
    const id = await started();
    await expect(engine.openPullRequest(id)).rejects.toThrow(/no verified wins/);
    for (let i = 0; i < 60 && ledger.ratchetHistory(id).length === 0; i++) engine.tick(id);
    const pr = await engine.openPullRequest(id);
    expect(pr.url).toMatch(/^https:\/\/github\.com\/acme\/search-api\/pull\/\d+$/);
  });

  it('unknown ids fail loudly', async () => {
    await expect(engine.start('nope')).rejects.toThrow(/not found/);
    await expect(engine.pauseAgent('nope')).rejects.toThrow(/not found/);
  });

  it('repoSlug normalises git urls', () => {
    expect(repoSlug('https://github.com/acme/search-api.git')).toBe('acme/search-api');
    expect(repoSlug('git@github.com:acme/search-api')).toBe('acme/search-api');
    expect(repoSlug('acme/search-api')).toBe('acme/search-api');
  });
});

describe('createEngineForApp', () => {
  it('seeds an empty ledger, resumes the running demo optimization, and reports the dev engine', async () => {
    const l = openLedger(':memory:');
    const app = createEngineForApp(l, { tickMs: 60_000, engine: 'dev' });
    try {
      expect(app.kind).toBe('dev');
      expect(app.engine.mode()).toBe('simulation');
      expect(l.listOptimizations()).toHaveLength(4);
      expect((await app.engine.planSandbox({ sandboxId: 'ratchet-sim', region: 'us-east-1', allowedInstanceTypes: ['c7i.large'], maxInstancesPerAgent: 1, maxInstanceLifetimeHours: 1 })).changes.length).toBeGreaterThan(0);
    } finally {
      app.dispose();
    }
  });

  it('does not reseed a ledger that already has data', () => {
    const l = openLedger(':memory:');
    const a = createEngineForApp(l, { tickMs: 60_000, engine: 'dev' });
    a.dispose();
    const b = createEngineForApp(l, { tickMs: 60_000, engine: 'dev' });
    b.dispose();
    expect(l.listOptimizations()).toHaveLength(4);
  });
});
