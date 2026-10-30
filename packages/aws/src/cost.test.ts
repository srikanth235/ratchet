import { describe, it, expect } from 'vitest';
import { BudgetGuard, CostMeter } from './cost.js';
import { MockCloudCompute } from './compute/mock.js';
import { StaticPriceBook } from './pricing.js';
import { ManualClock } from './util.js';

const HOUR = 3600_000;

async function setup() {
  const clock = new ManualClock(10 * HOUR);
  const cloud = new MockCloudCompute({ clock });
  const meter = new CostMeter({ compute: cloud, prices: new StaticPriceBook({ 'c7i.large': 0.1, 'c7i.2xlarge': 0.4, 'm7i.large': 0.2 }), clock });
  const id = await cloud.createAgentIdentity('o1', 'a1');
  const spec = { optimizationId: 'o1', agentId: 'a1', kind: 'mock' as const, instanceType: 'c7i.large', imageRef: 'ami', identity: id, secrets: [], repoUrl: 'r', branch: 'b' };
  return { clock, cloud, meter, spec };
}

describe('CostMeter', () => {
  it('computes live spend per optimization / agent / role from lifetimes', async () => {
    const { clock, cloud, meter, spec } = await setup();
    await cloud.launchRunner(spec); // 0.1/h
    cloud.simulateExperiment('o1', 'a1', 'm7i.large'); // 0.2/h
    await cloud.launchBenchInstance({ optimizationId: 'o1', instanceType: 'c7i.2xlarge', imageRef: 'a', purpose: 'bench' }); // 0.4/h
    clock.advance(1.5 * HOUR);
    const s = await meter.snapshot('o1');
    const o = s.byOptimization.o1!;
    expect(o.byCategory.agent_runner).toBeCloseTo(0.15);
    expect(o.byCategory.experiment).toBeCloseTo(0.3);
    expect(o.byCategory.bench_runner).toBeCloseTo(0.6);
    expect(o.byAgent.a1).toBeCloseTo(0.45);
    expect(s.totalUsd).toBeCloseTo(1.05);
    expect(s.unpriced).toEqual([]);
  });

  it('stops accruing at termination and applies the 60 s minimum', async () => {
    const { clock, cloud, meter, spec } = await setup();
    const r = await cloud.launchRunner(spec);
    clock.advance(10_000);
    await cloud.terminate([r.id]);
    clock.advance(5 * HOUR);
    const s = await meter.snapshot();
    expect(s.instances[0]!.billedSeconds).toBe(60);
    expect(s.totalUsd).toBeCloseTo((60 / 3600) * 0.1);
  });

  it('collect() emits only new spend, ledger-shaped, aggregated per (opt, agent, category)', async () => {
    const { clock, cloud, meter, spec } = await setup();
    await cloud.launchRunner(spec);
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    cloud.simulateExperiment('o1', 'a1', 'c7i.large');
    clock.advance(HOUR);
    const first = await meter.collect();
    expect(first).toHaveLength(2);
    const exp = first.find((d) => d.category === 'experiment')!;
    expect(exp).toEqual({ optimizationId: 'o1', agentId: 'a1', category: 'experiment', usd: expect.closeTo(0.2, 6), estimated: true, source: 'cost-meter', at: 11 * HOUR });
    expect(first.find((d) => d.category === 'agent_runner')!.usd).toBeCloseTo(0.1);

    expect(await meter.collect()).toEqual([]); // nothing elapsed
    clock.advance(0.5 * HOUR);
    const second = await meter.collect();
    expect(second.reduce((a, d) => a + d.usd, 0)).toBeCloseTo(0.15);
    // sum of deltas == cumulative snapshot
    const total = first.concat(second).reduce((a, d) => a + d.usd, 0);
    expect((await meter.snapshot()).totalUsd).toBeCloseTo(total);
  });

  it('does not double count instances that terminate between collects', async () => {
    const { clock, cloud, meter, spec } = await setup();
    const r = await cloud.launchRunner(spec);
    clock.advance(HOUR);
    const a = await meter.collect();
    clock.advance(0.5 * HOUR);
    await cloud.terminate([r.id]);
    clock.advance(3 * HOUR);
    const b = await meter.collect();
    expect(a[0]!.usd + b[0]!.usd).toBeCloseTo(0.15);
    expect(await meter.collect()).toEqual([]);
  });

  it('bench spend has no agentId; unpriced types are flagged, not silently priced', async () => {
    const { clock, cloud, meter } = await setup();
    await cloud.launchBenchInstance({ optimizationId: 'o1', instanceType: 'x9.huge', imageRef: 'a', purpose: 'bench' });
    clock.advance(HOUR);
    const s = await meter.snapshot();
    expect(s.unpriced).toEqual(['x9.huge']);
    expect(s.totalUsd).toBe(0);
    await cloud.launchBenchInstance({ optimizationId: 'o1', instanceType: 'c7i.2xlarge', imageRef: 'a', purpose: 'bench' });
    const d = (await meter.collect()).find((x) => x.category === 'bench_runner')!;
    expect(d.agentId).toBeUndefined();
    expect('agentId' in d).toBe(false);
  });

  it('stopped instance without timestamp stops accruing at first observation', async () => {
    const { clock, cloud, meter, spec } = await setup();
    const r = await cloud.launchRunner(spec);
    clock.advance(HOUR);
    cloud.simulateStopped(r.id);
    clock.advance(HOUR); // stop was HOUR ago; mock stamps terminatedAt so billing ends there
    expect((await meter.snapshot()).totalUsd).toBeCloseTo(0.1);
  });
});

describe('BudgetGuard', () => {
  it('warns at 80%, trips at 100%, each exactly once', () => {
    const events: string[] = [];
    const g = new BudgetGuard({ budgetUsd: 100, onWarn: () => events.push('warn'), onTrip: () => events.push('trip') });
    expect(g.update(79.99)).toMatchObject({ state: 'ok', crossed: null });
    expect(g.update(80)).toMatchObject({ state: 'warn', crossed: 'warn', fraction: 0.8 });
    expect(g.update(90)).toMatchObject({ state: 'warn', crossed: null });
    expect(g.update(100)).toMatchObject({ state: 'tripped', crossed: 'trip' });
    expect(g.update(150)).toMatchObject({ state: 'tripped', crossed: null });
    expect(g.update(10)).toMatchObject({ state: 'tripped' }); // never un-trips
    expect(events).toEqual(['warn', 'trip']);
    expect(g.current).toBe('tripped');
  });

  it('jumping straight past 100% trips without a warn', () => {
    const events: string[] = [];
    const g = new BudgetGuard({ budgetUsd: 10, onWarn: () => events.push('warn'), onTrip: () => events.push('trip') });
    expect(g.update(12).crossed).toBe('trip');
    expect(events).toEqual(['trip']);
  });

  it('rejects a non-positive budget; custom warn fraction', () => {
    expect(() => new BudgetGuard({ budgetUsd: 0 })).toThrow();
    expect(new BudgetGuard({ budgetUsd: 10, warnFraction: 0.5 }).update(5).crossed).toBe('warn');
  });
});
