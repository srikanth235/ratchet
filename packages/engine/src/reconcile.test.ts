import { describe, expect, it } from 'vitest';
import type { CostActuals } from '@ratchet/core';
import { makeHarness, type Harness } from './testing/harness.js';

const HOUR = 3_600_000;

const spendRows = (h: Harness, id: string) =>
  h.ledger.sqlite
    .prepare('select category, usd, estimated, source from spend_events where optimization_id = ? and source = ? order by id')
    .all(id, 'cost-explorer') as Array<{ category: string; usd: number; estimated: number; source: string }>;

function billing() {
  return {
    calls: [] as unknown[],
    async actuals(q: unknown): Promise<CostActuals> {
      this.calls.push(q);
      return { byCategory: { agent_runner: 0.5, experiment: 0.001, bench_runner: 0.25 }, groups: 3, complete: true };
    },
  };
}

describe('reconcileCosts', () => {
  it('is unavailable without a cost reconciler, and teardown then records no reconcile', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    await expect(h.engine.reconcileCosts(id)).rejects.toMatchObject({ code: 'unavailable' });
    await h.engine.teardown(id);
    expect(h.engine.teardownReport(id)!.costReconcile).toBeUndefined();
  });

  it('teardown reconciles best effort: pending inside the billing lag, nothing written, no billing query', async () => {
    const costs = billing();
    const h = makeHarness({ costs });
    const { id, agents } = await h.begin();
    h.compute.launchExperiment(id, agents.codex!);
    await h.cycle(2);
    await h.engine.teardown(id);
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
    expect(h.engine.teardownReport(id)!.costReconcile).toMatchObject({ status: 'pending', writtenUsd: 0 });
    expect(spendRows(h, id)).toEqual([]);
    expect(costs.calls).toHaveLength(0);
  });

  it('writes correction rows once billing is final, and a second run changes nothing', async () => {
    const costs = billing();
    const h = makeHarness({ costs });
    const { id, agents } = await h.begin();
    h.compute.launchExperiment(id, agents.codex!);
    await h.cycle(3);
    await h.engine.teardown(id);
    const before = h.ledger.spendBreakdown(id);
    expect(before.actual).toBe(0);

    h.clock.advance(25 * HOUR);
    const r1 = await h.engine.reconcileCosts(id);
    expect(r1.status).toBe('reconciled');
    const rows = spendRows(h, id);
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.every((r) => r.estimated === 0 && r.source === 'cost-explorer')).toBe(true);
    const after = h.ledger.spendBreakdown(id);
    expect(after.byCategory.agent_runner).toBeCloseTo(0.5, 9);
    expect(after.byCategory.experiment).toBeCloseTo(0.001, 9);
    expect(after.byCategory.bench_runner).toBeCloseTo(0.25, 9);
    expect(after.estimated).toBeCloseTo(before.estimated, 9); // estimates are never rewritten
    expect(h.engine.costReconcileReport(id)!.status).toBe('reconciled');
    expect(h.events.some((e) => (e as { type?: string }).type === 'notice' && /reconciled/.test((e as { text?: string }).text ?? ''))).toBe(true);

    const r2 = await h.engine.reconcileCosts(id);
    expect(r2.status).toBe('unchanged');
    expect(spendRows(h, id)).toHaveLength(rows.length);
    expect(h.ledger.totalSpend(id)).toBeCloseTo(after.total, 9);
  });

  it('a failing reconciler never fails teardown', async () => {
    const h = makeHarness({
      costs: {
        actuals: async () => {
          throw new Error('ce down');
        },
      },
      settings: { costLagMs: 0 },
    });
    const { id } = await h.begin();
    await h.engine.teardown(id);
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
    expect(h.engine.teardownReport(id)!.costReconcile).toEqual({ status: 'error', reason: 'ce down' });
  });
});
