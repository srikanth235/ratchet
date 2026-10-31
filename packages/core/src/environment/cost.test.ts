import { describe, expect, it } from 'vitest';
import type { PriceBook } from '../ports/index.js';
import { BASE_STARTUP_MINUTES, GP3_USD_PER_GB_MONTH, INSTANCE_USD_PER_HOUR, STARTUP_MINUTES_PER_MACHINE, defaultMachines, estimateEnvironmentCostLive, estimateMachinesCost, staticHourlyUsd, volumeUsdPerHour } from './cost.js';

const m = (machine: string, over: Partial<Parameters<typeof estimateMachinesCost>[0][number]> = {}) => ({ name: machine, role: 'app' as const, machine, detail: '', rootVolumeGb: 0, ...over });

describe('estimateMachinesCost', () => {
  it('sums the instance prices and prices the run for its time limit plus start-up', () => {
    const e = estimateMachinesCost([m('c7i.2xlarge'), m('r7g.large'), m('t4g.medium'), m('c7i.large')], { timeLimitSeconds: 2 * 3600 });
    // 0.357 + 0.1071 + 0.0336 + 0.08925 = 0.58695
    expect(e.cost.usdPerHour).toBeCloseTo(0.587, 3);
    expect(e.cost.startupMinutes).toBe(BASE_STARTUP_MINUTES + 4 * STARTUP_MINUTES_PER_MACHINE); // ~6 min, as the design shows
    expect(e.cost.estimateForRunUsd).toBeCloseTo(0.58695 * (2 + 6 / 60), 2);
    expect(e.unpriced).toEqual([]);
    expect(e.resources.map((r) => r.usdPerHour)).toEqual([0.357, 0.1071, 0.0336, 0.0893]);
  });

  it('adds gp3 for the root and data volumes', () => {
    const e = estimateMachinesCost([m('c7i.large', { rootVolumeGb: 30, dataVolumeGb: 970 })], { timeLimitSeconds: 3600 });
    expect(e.resources[0]!.usdPerHour).toBeCloseTo(0.08925 + (1000 * GP3_USD_PER_GB_MONTH) / 730, 4);
    expect(volumeUsdPerHour(0)).toBe(0);
    expect(volumeUsdPerHour(-5)).toBe(0);
    expect(volumeUsdPerHour(730)).toBeCloseTo(GP3_USD_PER_GB_MONTH, 10);
  });

  it('defaults the root volume to 30 GB', () => {
    const e = estimateMachinesCost([{ name: 'a', role: 'app', machine: 'c7i.large', detail: '' }], { timeLimitSeconds: 3600 });
    expect(e.resources[0]!.usdPerHour).toBeCloseTo(0.08925 + (30 * GP3_USD_PER_GB_MONTH) / 730, 4);
  });

  it('has no start-up and no cost without machines', () => {
    expect(estimateMachinesCost([], { timeLimitSeconds: 7200 }).cost).toEqual({ usdPerHour: 0, startupMinutes: 0, estimateForRunUsd: 0 });
  });

  it('scales with the time limit', () => {
    const one = estimateMachinesCost([m('c7i.large')], { timeLimitSeconds: 3600 }).cost.estimateForRunUsd;
    const four = estimateMachinesCost([m('c7i.large')], { timeLimitSeconds: 4 * 3600 }).cost.estimateForRunUsd;
    expect(four).toBeGreaterThan(one * 3);
    expect(estimateMachinesCost([m('c7i.large')], { timeLimitSeconds: -5 }).cost.estimateForRunUsd).toBeGreaterThanOrEqual(0);
  });

  it('reports instance types it cannot price, once, and prices them 0 (the validator then fails the cost check)', () => {
    const e = estimateMachinesCost([m('x9z.huge'), m('x9z.huge'), m('c7i.large')], { timeLimitSeconds: 3600 });
    expect(e.unpriced).toEqual(['x9z.huge']);
    expect(e.resources[0]!.usdPerHour).toBe(0);
  });

  it('takes an explicit price table', () => {
    const e = estimateMachinesCost([m('foo.bar')], { timeLimitSeconds: 3600, prices: { 'foo.bar': 2 } });
    expect(e.cost.usdPerHour).toBe(2);
    expect(e.unpriced).toEqual([]);
  });
});

describe('price table', () => {
  it('covers the families a draft uses, with sane relative prices', () => {
    for (const t of ['c7i.large', 'c7i.2xlarge', 'm7i.xlarge', 'r7g.large', 'r7i.large', 't4g.medium', 'm7g.large', 'c7g.large']) expect(staticHourlyUsd(t), t).toBeGreaterThan(0);
    expect(staticHourlyUsd('c7i.xlarge')!).toBeCloseTo(2 * staticHourlyUsd('c7i.large')!, 3);
    expect(staticHourlyUsd('c7i.2xlarge')!).toBeCloseTo(2 * staticHourlyUsd('c7i.xlarge')!, 3);
    expect(staticHourlyUsd('nope.nope')).toBeUndefined();
    expect(Object.values(INSTANCE_USD_PER_HOUR).every((p) => p > 0)).toBe(true);
  });
});

describe('estimateEnvironmentCostLive', () => {
  it('prefers the live price book and falls back to the table per instance type', async () => {
    const book: PriceBook = {
      async hourlyUsd(t) {
        if (t === 'c7i.large') return 1;
        throw new Error('no price');
      },
    };
    const e = await estimateEnvironmentCostLive([m('c7i.large'), m('r7g.large')], { timeLimitSeconds: 3600 }, book);
    expect(e.resources.map((r) => r.usdPerHour)).toEqual([1, 0.1071]);
    expect(e.unpriced).toEqual([]);
  });
});

describe('defaultMachines', () => {
  it('none is the evaluator host, single is one pinned machine, full has none of its own', () => {
    expect(defaultMachines('none', 'c7i.large')).toMatchObject([{ role: 'evaluator', machine: 'c7i.large' }]);
    expect(defaultMachines('single', 'c7i.xlarge')).toMatchObject([{ role: 'app', machine: 'c7i.xlarge' }]);
    expect(defaultMachines('full', 'c7i.xlarge')).toEqual([]);
  });
});
