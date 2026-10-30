import { describe, it, expect } from 'vitest';
import { Reaper } from './reaper.js';
import { MockCloudCompute } from './compute/mock.js';
import { ManualClock } from './util.js';

const HOUR = 3600_000;

function setup(over: Partial<ConstructorParameters<typeof Reaper>[0]> = {}) {
  const clock = new ManualClock(100 * HOUR);
  const cloud = new MockCloudCompute({ clock });
  const ended = new Set<string>();
  const stopped = new Set<string>();
  const reaper = new Reaper({
    compute: cloud,
    clock,
    maxInstanceLifetimeHours: 2,
    isOptimizationEnded: (id) => ended.has(id),
    isAgentStopped: (o, a) => stopped.has(`${o}/${a}`),
    ...over,
  });
  return { clock, cloud, ended, stopped, reaper };
}

const states = async (cloud: MockCloudCompute) => Object.fromEntries((await cloud.listTagged()).map((i) => [i.id, i.state]));

describe('Reaper', () => {
  it('terminates instances past max lifetime and leaves young ones', async () => {
    const { clock, cloud, reaper } = setup();
    const old = cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    clock.advance(2 * HOUR + 1);
    const young = cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    const rep = await reaper.reap();
    expect(rep.terminated).toEqual([{ instanceId: old.id, optimizationId: 'o1', agentId: 'a1', role: 'experiment', reason: 'max-lifetime', ageMs: 2 * HOUR + 1 }]);
    expect(rep.scanned).toBe(2);
    expect(await states(cloud)).toEqual({ [old.id]: 'terminated', [young.id]: 'running' });
  });

  it('exactly at the limit is kept', async () => {
    const { clock, cloud, reaper } = setup();
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    clock.advance(2 * HOUR);
    expect((await reaper.reap()).terminated).toEqual([]);
  });

  it('terminates instances of stopped agents (only that agent)', async () => {
    const { cloud, stopped, reaper } = setup();
    const mine = cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    const other = cloud.simulateExperiment('o1', 'a2', 'c7i.large');
    stopped.add('o1/a1');
    const rep = await reaper.reap();
    expect(rep.terminated.map((t) => [t.instanceId, t.reason])).toEqual([[mine.id, 'agent-stopped']]);
    expect((await states(cloud))[other.id]).toBe('running');
  });

  it('terminates everything of an ended optimization, including bench (no agent)', async () => {
    const { cloud, ended, reaper } = setup();
    const e = cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    const b = await cloud.launchBenchInstance({ optimizationId: 'o1', instanceType: 'c7i.2xlarge', imageRef: 'a', purpose: 'bench' });
    const keep = cloud.simulateExperiment('o2', 'a1', 'c7i.large');
    ended.add('o1');
    const rep = await reaper.reap();
    expect(rep.terminated.map((t) => t.instanceId).sort()).toEqual([e.id, b.id].sort());
    expect(rep.terminated.every((t) => t.reason === 'optimization-ended')).toBe(true);
    expect((await states(cloud))[keep.id]).toBe('running');
  });

  it('optimization-ended takes precedence over agent-stopped and lifetime', async () => {
    const { clock, cloud, ended, stopped, reaper } = setup();
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    clock.advance(5 * HOUR);
    ended.add('o1');
    stopped.add('o1/a1');
    expect((await reaper.reap()).terminated[0]!.reason).toBe('optimization-ended');
  });

  it('terminates merely-stopped instances (terminate-on-shutdown missed)', async () => {
    const { cloud, reaper } = setup();
    const i = cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    cloud.simulateStopped(i.id);
    expect((await reaper.reap()).terminated.map((t) => t.reason)).toEqual(['stopped-instance']);
  });

  it('enforces max_instances_per_agent: keeps oldest, kills newest, reports offender', async () => {
    const { clock, cloud, reaper } = setup({ maxInstancesPerAgent: 2 });
    const ids: string[] = [];
    for (let i = 0; i < 4; i++) {
      ids.push(cloud.simulateExperiment('o1', 'a1', 'c7i.large').id);
      clock.advance(1000);
    }
    cloud.simulateExperiment('o1', 'a2', 'c7i.large'); // within quota
    const rep = await reaper.reap();
    expect(rep.terminated.map((t) => [t.instanceId, t.reason])).toEqual([
      [ids[2], 'over-quota'],
      [ids[3], 'over-quota'],
    ]);
    expect(rep.overQuotaAgents).toEqual([{ optimizationId: 'o1', agentId: 'a1', running: 4 }]);
  });

  it('quota ignores runners and bench; no quota configured means no enforcement', async () => {
    const { cloud, reaper } = setup({ maxInstancesPerAgent: 1 });
    const id = await cloud.createAgentIdentity('o1', 'a1');
    await cloud.launchRunner({ optimizationId: 'o1', agentId: 'a1', kind: 'mock', instanceType: 'c7i.large', imageRef: 'a', identity: id, secrets: [], repoUrl: 'r', branch: 'b' });
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    expect((await reaper.reap()).terminated).toEqual([]);
    const { cloud: c2, reaper: r2 } = setup();
    for (let i = 0; i < 10; i++) c2.simulateExperiment('o1', 'a1', 'c7i.large');
    expect((await r2.reap()).terminated).toEqual([]);
  });

  it('is idempotent: second run finds nothing and does not call terminate again', async () => {
    const { clock, cloud, reaper } = setup();
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    clock.advance(3 * HOUR);
    let terminateCalls = 0;
    const orig = cloud.terminate.bind(cloud);
    cloud.terminate = async (ids) => {
      terminateCalls++;
      return orig(ids);
    };
    expect((await reaper.reap()).terminated).toHaveLength(1);
    expect((await reaper.reap()).terminated).toEqual([]);
    expect(terminateCalls).toBe(1);
  });

  it('skips instances already shutting down; scopes to one optimization when asked', async () => {
    const { clock, cloud, reaper } = setup();
    const a = cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    const b = cloud.simulateExperiment('o2', 'a1', 'c7i.large');
    clock.advance(3 * HOUR);
    const rep = await reaper.reap('o1');
    expect(rep.terminated.map((t) => t.instanceId)).toEqual([a.id]);
    expect((await states(cloud))[b.id]).toBe('running');
  });

  it('records terminate failures in the report instead of throwing', async () => {
    const { clock, cloud, reaper } = setup();
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    clock.advance(3 * HOUR);
    cloud.terminate = async () => {
      throw new Error('UnauthorizedOperation');
    };
    const rep = await reaper.reap();
    expect(rep.terminated).toEqual([]);
    expect(rep.errors).toEqual(['terminate failed: UnauthorizedOperation']);
  });

  it('supports async predicates', async () => {
    const { cloud, reaper } = setup({ isOptimizationEnded: async () => true });
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    expect((await reaper.reap()).terminated).toHaveLength(1);
  });
});
