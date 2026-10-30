import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@ratchet/core';
import { makeHarness } from './testing/harness.js';

const ev = (type: AgentEvent['type'], extra: Partial<AgentEvent> = {}): AgentEvent => ({ at: 1, type, ...extra });
const cloudCalls = (log: string[]): string[] => log.filter((l) => /^compute\.(createAgentIdentity|launchRunner|revoke|deleteIdentity|deleteSecrets)/.test(l) || l.startsWith('compute.put'));

describe('agents on this machine', () => {
  it('defaults to local when the engine has a local runner control, and starts agents without any cloud runner resources', async () => {
    // no model keys at all: local agents use the user's own CLI login
    const h = makeHarness({ localRunners: true, settings: { secrets: () => undefined } });
    const { id, agents } = await h.begin();
    expect(h.engine.agentsWhere(id)).toBe('local');
    expect(h.runners.started).toEqual([]);
    expect(h.localRunners.started.map((s) => s.handle)).toEqual([
      { optimizationId: id, agentId: agents['claude-code'], instanceId: `local:${agents['claude-code']}` },
      { optimizationId: id, agentId: agents.codex, instanceId: `local:${agents.codex}` },
    ]);
    expect(h.localRunners.started[0]!.config).toMatchObject({ repoUrl: h.git.remoteUrl(), baseRef: 'main', branch: 'ratchet/opt-test/claude-code', kind: 'claude-code' });
    expect(cloudCalls(h.log)).toEqual([]);
    expect(h.compute.instances.size).toBe(0);
    expect(h.ledger.listAgents(id).map((a) => [a.status, a.runnerInstanceId])).toEqual([
      ['running', `local:${agents['claude-code']}`],
      ['running', `local:${agents.codex}`],
    ]);
    // the agents' branches are still created on the git host
    expect([...h.git.branches.keys()].sort()).toEqual(['ratchet/opt-test/claude-code', 'ratchet/opt-test/codex']);
  });

  it('routes events, messages, pause / resume and stop to the local control', async () => {
    const h = makeHarness({ localRunners: true });
    const { id, agents } = await h.begin();
    const cc = agents['claude-code']!;
    h.localRunners.emit(cc, ev('message', { text: 'hello from the laptop' }));
    await h.cycle(1);
    expect(h.localRunners.pollCalls.length).toBeGreaterThan(0);
    expect(h.runners.pollCalls).toEqual([]);
    expect(h.ledger.agentEvents(cc).some((e) => (e.payload as AgentEvent).text === 'hello from the laptop')).toBe(true);

    await h.engine.messageAgent(cc, 'try the cache');
    await h.engine.pauseAgent(cc);
    await h.engine.resumeAgent(cc);
    expect(h.localRunners.sentTo(cc).map((m) => m.type)).toEqual(['user', 'pause', 'resume']);
    expect(h.runners.sent).toEqual([]);

    await h.engine.stop(id);
    expect(h.localRunners.stopped.sort()).toEqual(Object.values(agents).sort());
    expect(h.runners.stopped).toEqual([]);
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
  });

  it('teardown releases the local workspaces and leaves cloud-side secrets alone', async () => {
    const h = makeHarness({ localRunners: true });
    const { id } = await h.begin();
    await h.engine.stop(id);
    await h.engine.teardown(id);
    expect(h.localRunners.released).toEqual([id]);
    expect(h.runners.released).toEqual([]);
    expect(h.log.some((l) => l.startsWith('compute.deleteSecrets'))).toBe(false);
    expect(h.engine.teardownReport(id)).toMatchObject({ deletedSecretsFor: [], deletedIdentitiesFor: [], errors: [] });
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
  });

  it('a lost local agent (the app restarted) is reported failed and the run can end', async () => {
    const h = makeHarness({ localRunners: true });
    const { id, agents } = await h.begin({ agents: ['codex'] });
    h.localRunners.emit(agents.codex!, ev('status', { text: 'local agent lost: the app was restarted while it was running', data: { state: 'failed', reason: 'lost' } }));
    await h.cycle(1);
    expect(h.ledger.getAgent(agents.codex!)!.status).toBe('failed');
    await h.engine.stop(id);
  });

  it('an optimization can ask for the cloud, which keeps today\'s path (identity, secrets, runner instance)', async () => {
    const h = makeHarness({ localRunners: true });
    const { id, agents } = await h.begin({ agents: ['codex'], agentsWhere: 'cloud' });
    expect(h.engine.agentsWhere(id)).toBe('cloud');
    expect(h.localRunners.started).toEqual([]);
    expect(h.runners.started).toHaveLength(1);
    expect(h.log).toContain(`compute.createAgentIdentity:${agents.codex}`);
    expect(h.log).toContain(`compute.launchRunner:${agents.codex}`);
    expect(h.compute.live(id)).toHaveLength(1);
    await h.engine.stop(id);
    await h.engine.teardown(id);
    expect(h.runners.released).toEqual([id]); // the control may have nothing to release; the engine still asks
    expect(h.localRunners.released).toEqual([]);
  });

  it('the engine setting picks the default, and an engine without a local control stays in the cloud', async () => {
    const cloudDefault = makeHarness({ localRunners: true, settings: { agentsWhere: 'cloud' } });
    const a = await cloudDefault.begin({ agents: ['codex'] });
    expect(cloudDefault.engine.agentsWhere(a.id)).toBe('cloud');

    const plain = makeHarness();
    const b = await plain.begin({ agents: ['codex'] });
    expect(plain.engine.agentsWhere(b.id)).toBe('cloud');
    expect(plain.runners.started).toHaveLength(1);
  });

  it('local agents are refused where there is no local control', async () => {
    const h = makeHarness();
    await expect(h.begin({ agentsWhere: 'local' })).rejects.toMatchObject({ code: 'invalid_input', message: expect.stringContaining('no local runner control') });
    expect(h.ledger.listOptimizations()).toHaveLength(0);
    await expect(h.begin({ agentsWhere: 'moon' as never })).rejects.toMatchObject({ code: 'invalid_input' });
  });

  it('survives an engine restart: the choice is remembered per optimization', async () => {
    const h = makeHarness({ localRunners: true });
    const { id, agents } = await h.begin({ agents: ['codex'], agentsWhere: 'cloud' });
    h.restart();
    await h.engine.ready();
    expect(h.engine.agentsWhere(id)).toBe('cloud');
    await h.engine.messageAgent(agents.codex!, 'still cloud');
    expect(h.runners.sentTo(agents.codex!).map((m) => m.type)).toEqual(['user']);
  });

  it('works with the automatic sandbox: the evaluator is cloud even when the agents are not', async () => {
    const h = makeHarness({ mode: 'live', localRunners: true, settings: { sandbox: { auto: true } } });
    const { id } = await h.begin({ agents: ['codex'] });
    expect(h.engine.agentsWhere(id)).toBe('local');
    expect(h.log).toContain('sandbox.apply:ratchet-sandbox');
    expect(h.log.some((l) => l.startsWith('bench.baseline'))).toBe(true);
    expect(cloudCalls(h.log)).toEqual([]);
    await h.engine.stop(id);
    h.clock.advance(31 * 60_000);
    await h.engine.tick();
    const receipt = (await h.engine.teardownReceipt(id))!;
    expect(receipt).toMatchObject({ nothingRunning: true, removed: { instances: [], agentIdentities: [], secrets: [] } });
    expect(h.localRunners.released).toEqual([id]);
  });
});
