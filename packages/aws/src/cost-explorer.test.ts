import { beforeEach, describe, expect, it } from 'vitest';
import { mockClient } from 'aws-sdk-client-mock';
import { CostExplorerClient, GetCostAndUsageCommand } from '@aws-sdk/client-cost-explorer';
import { AwsCostReconciler, categoryForTags } from './cost-explorer.js';
import { MockCostReconciler, billed } from './cost-explorer.mock.js';

const ce = mockClient(CostExplorerClient);
beforeEach(() => ce.reset());

const group = (role: string, agent: string, amount: string) => ({
  Keys: [`ratchet:role$${role}`, `ratchet:agent$${agent}`],
  Metrics: { UnblendedCost: { Amount: amount, Unit: 'USD' } },
});
const day = (start: string, groups: ReturnType<typeof group>[], estimated = false) => ({
  TimePeriod: { Start: start, End: start },
  Estimated: estimated,
  Total: {},
  Groups: groups,
});

const DAY = 86_400_000;
const T = Date.UTC(2026, 9, 28, 13, 30); // 2026-10-28 13:30 UTC
const query = { optimizationId: 'opt-1', startMs: T, endMs: T + DAY / 2 }; // ends 2026-10-29 01:30 UTC

const make = (o: ConstructorParameters<typeof AwsCostReconciler>[0] = {}) => new AwsCostReconciler({ client: new CostExplorerClient({}), ...o });

describe('categoryForTags', () => {
  it('maps role/agent tags to ledger categories', () => {
    expect(categoryForTags('agent_runner', 'a1')).toBe('agent_runner');
    expect(categoryForTags('bench', '')).toBe('bench_runner');
    expect(categoryForTags('experiment', 'a1')).toBe('experiment');
    expect(categoryForTags('', 'a1')).toBe('experiment'); // agent-launched: opt + agent tags only
    expect(categoryForTags('', '')).toBe('storage_egress');
    expect(categoryForTags('mystery', '')).toBe('storage_egress');
  });
});

describe('AwsCostReconciler', () => {
  it('sends a DAILY UnblendedCost query filtered by ratchet:opt and grouped by role + agent tags', async () => {
    ce.on(GetCostAndUsageCommand).resolves({ ResultsByTime: [] });
    await make().actuals(query);
    const input = ce.commandCalls(GetCostAndUsageCommand)[0]!.args[0].input;
    expect(input).toEqual({
      TimePeriod: { Start: '2026-10-28', End: '2026-10-30' }, // end is exclusive: the day after the window's last day
      Granularity: 'DAILY',
      Metrics: ['UnblendedCost'],
      Filter: { Tags: { Key: 'ratchet:opt', Values: ['opt-1'], MatchOptions: ['EQUALS'] } },
      GroupBy: [
        { Type: 'TAG', Key: 'ratchet:role' },
        { Type: 'TAG', Key: 'ratchet:agent' },
      ],
    });
  });

  it('can additionally require the sandbox tag', async () => {
    ce.on(GetCostAndUsageCommand).resolves({ ResultsByTime: [] });
    await make({ sandboxId: 'rt-test' }).actuals(query);
    const filter = ce.commandCalls(GetCostAndUsageCommand)[0]!.args[0].input.Filter;
    expect(filter).toEqual({
      And: [
        { Tags: { Key: 'ratchet:opt', Values: ['opt-1'], MatchOptions: ['EQUALS'] } },
        { Tags: { Key: 'ratchet:sandbox', Values: ['rt-test'], MatchOptions: ['EQUALS'] } },
      ],
    });
  });

  it('sums days and pages into ledger categories', async () => {
    ce.on(GetCostAndUsageCommand)
      .resolvesOnce({
        ResultsByTime: [day('2026-10-28', [group('agent_runner', 'a1', '0.50'), group('bench', '', '0.25'), group('', 'a1', '0.125')])],
        NextPageToken: 'p2',
      })
      .resolvesOnce({
        ResultsByTime: [day('2026-10-29', [group('agent_runner', 'a2', '0.25'), group('', '', '0.0625'), group('bench', '', '0')])],
      });
    const r = await make().actuals(query);
    expect(r.byCategory).toEqual({ agent_runner: 0.75, bench_runner: 0.25, experiment: 0.125, storage_egress: 0.0625 });
    expect(r.groups).toBe(5); // the zero-cost group is not counted
    expect(r.complete).toBe(true);
    expect(r.note).toBeUndefined();
    const calls = ce.commandCalls(GetCostAndUsageCommand);
    expect(calls).toHaveLength(2);
    expect(calls[1]!.args[0].input.NextPageToken).toBe('p2');
  });

  it('reports incomplete data while days are flagged Estimated, unless acceptEstimated', async () => {
    ce.on(GetCostAndUsageCommand).resolves({ ResultsByTime: [day('2026-10-28', [group('bench', '', '1')]), day('2026-10-29', [group('bench', '', '1')], true)] });
    const r = await make().actuals(query);
    expect(r.complete).toBe(false);
    expect(r.note).toMatch(/1 day\(s\) as estimated \(2026-10-29\)/);
    expect((await make({ acceptEstimated: true }).actuals(query)).complete).toBe(true);
  });

  it('explains empty results (tags not activated / data not there yet)', async () => {
    ce.on(GetCostAndUsageCommand).resolves({ ResultsByTime: [day('2026-10-28', [])] });
    const r = await make().actuals(query);
    expect(r).toMatchObject({ groups: 0, byCategory: {}, complete: true });
    expect(r.note).toMatch(/cost-allocation tags/);
  });

  it('rejects ids that are not plain, non-numeric amounts, and surfaces SDK errors', async () => {
    await expect(make().actuals({ ...query, optimizationId: 'a*b' })).rejects.toThrow(/invalid optimization id/);
    await expect(make().actuals({ ...query, endMs: query.startMs - 1 })).rejects.toThrow(/ends before/);
    expect(ce.commandCalls(GetCostAndUsageCommand)).toHaveLength(0);
    ce.on(GetCostAndUsageCommand).resolves({ ResultsByTime: [day('2026-10-28', [group('bench', '', 'NaN')])] });
    await expect(make().actuals(query)).rejects.toThrow(/non-numeric/);
    ce.reset();
    ce.on(GetCostAndUsageCommand).rejects(Object.assign(new Error('not authorized to perform: ce:GetCostAndUsage'), { name: 'AccessDeniedException' }));
    await expect(make().actuals(query)).rejects.toThrow(/ce:GetCostAndUsage/);
  });
});

describe('MockCostReconciler', () => {
  it('returns canned or computed actuals and records queries', async () => {
    const m = new MockCostReconciler({ actuals: billed({ bench_runner: 2 }) });
    expect(await m.actuals(query)).toEqual({ byCategory: { bench_runner: 2 }, groups: 1, complete: true });
    expect(m.calls).toEqual([query]);
    expect(await new MockCostReconciler().actuals(query)).toEqual({ byCategory: {}, groups: 0, complete: true });
    const fn = new MockCostReconciler({ actuals: (q) => billed({ experiment: q.endMs - q.startMs }) });
    expect((await fn.actuals(query)).byCategory.experiment).toBe(DAY / 2);
    await expect(new MockCostReconciler({ error: new Error('boom') }).actuals(query)).rejects.toThrow('boom');
  });
});
