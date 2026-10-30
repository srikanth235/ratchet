import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { openLedger, type Ledger } from '../ledger/index.js';
import type { CostActuals, CostActualsQuery, CostReconciler } from '../ports/index.js';
import { COST_EXPLORER_LAG_MS, reconcileSpend } from './reconcile.js';

const HOUR = 3_600_000;
const T0 = 1_700_000_000_000;

let ledger: Ledger;
let optId: string;
beforeEach(() => {
  ledger = openLedger(':memory:', { now: () => T0 });
  optId = ledger.createOptimization({
    repo: 'acme/search', baseRef: 'main', metric: 'p95', direction: 'minimize', minGain: 0.03, budgetUsd: 100, timeLimitS: 3600, configYaml: 'version: 0\n',
  }).id;
  ledger.setOptimizationStatus(optId, 'running', T0);
  ledger.recordSpend({ optimizationId: optId, category: 'agent_runner', usd: 1, source: 'ec2:i-1', at: T0 + 1 });
  ledger.recordSpend({ optimizationId: optId, category: 'experiment', usd: 2, source: 'ec2:i-2', at: T0 + 2 });
  ledger.recordSpend({ optimizationId: optId, category: 'tokens', usd: 5, source: 'agent', at: T0 + 3 });
});
afterEach(() => ledger.close());

function reconciler(actuals: CostActuals | ((q: CostActualsQuery) => CostActuals)) {
  const calls: CostActualsQuery[] = [];
  const r: CostReconciler = {
    async actuals(q) {
      calls.push(q);
      return typeof actuals === 'function' ? actuals(q) : actuals;
    },
  };
  return { r, calls };
}

const final = (byCategory: CostActuals['byCategory']): CostActuals => ({ byCategory, groups: Object.keys(byCategory).length, complete: true });
const corrections = () =>
  ledger.sqlite.prepare("select category, usd, estimated, source, agent_id as agentId from spend_events where source = 'cost-explorer' order by id").all() as Array<{
    category: string; usd: number; estimated: number; source: string; agentId: string | null;
  }>;
const endAt = T0 + 2 * HOUR;
const ready = endAt + COST_EXPLORER_LAG_MS + 1;

describe('reconcileSpend', () => {
  it('writes positive and negative corrections as estimated=false / source=cost-explorer, leaving estimates and tokens alone', async () => {
    ledger.setOptimizationStatus(optId, 'stopped', endAt);
    const { r, calls } = reconciler(final({ agent_runner: 1.5, experiment: 1.25, bench_runner: 0.3 }));
    const rep = await reconcileSpend(ledger, r, optId, { now: ready });
    expect(rep.status).toBe('reconciled');
    expect(calls).toEqual([{ optimizationId: optId, startMs: T0, endMs: endAt }]);
    const rows = corrections();
    expect(rows.map((x) => [x.category, Number(x.usd.toFixed(9))])).toEqual([
      ['agent_runner', 0.5],
      ['experiment', -0.75],
      ['bench_runner', 0.3],
      // storage_egress: billed 0, ledger 0 -> nothing
    ]);
    expect(rows.every((x) => x.estimated === 0 && x.source === 'cost-explorer' && x.agentId === null)).toBe(true);
    const b = ledger.spendBreakdown(optId);
    expect(b.byCategory).toMatchObject({ agent_runner: 1.5, experiment: 1.25, bench_runner: 0.3, tokens: 5 });
    expect(b.estimated).toBe(8);
    expect(rep.writtenUsd).toBeCloseTo(0.05, 9);
  });

  it('is idempotent: re-running with the same data writes nothing; later data writes only the remainder', async () => {
    ledger.setOptimizationStatus(optId, 'stopped', endAt);
    let usd = 1.5;
    const { r } = reconciler(() => final({ agent_runner: usd, experiment: 2 }));
    expect((await reconcileSpend(ledger, r, optId, { now: ready })).status).toBe('reconciled');
    const n = corrections().length;
    for (let i = 0; i < 3; i++) {
      const again = await reconcileSpend(ledger, r, optId, { now: ready + i });
      expect(again.status).toBe('unchanged');
      expect(again.writtenUsd).toBe(0);
    }
    expect(corrections()).toHaveLength(n);
    usd = 1.75; // billing finalised a little higher
    await reconcileSpend(ledger, r, optId, { now: ready + 10 });
    const rows = corrections();
    expect(rows).toHaveLength(n + 1);
    expect(rows.at(-1)!.usd).toBeCloseTo(0.25, 9);
    expect(ledger.spendBreakdown(optId).byCategory.agent_runner).toBeCloseTo(1.75, 9);
  });

  it('is pending (no rows, no billing query) while the optimization runs or inside the billing lag', async () => {
    const { r, calls } = reconciler(final({ agent_runner: 9 }));
    const running = await reconcileSpend(ledger, r, optId, { now: ready });
    expect(running).toMatchObject({ status: 'pending', writtenUsd: 0 });
    expect(running.reason).toMatch(/running/);
    ledger.setOptimizationStatus(optId, 'stopped', endAt);
    const lagging = await reconcileSpend(ledger, r, optId, { now: endAt + 3 * HOUR });
    expect(lagging.status).toBe('pending');
    expect(lagging.reason).toMatch(/lags up to 24 h.*21 h/);
    expect(calls).toHaveLength(0);
    expect(corrections()).toEqual([]);
  });

  it('is pending, writing nothing, when billing is flagged incomplete or returns no data (tags not activated)', async () => {
    ledger.setOptimizationStatus(optId, 'stopped', endAt);
    const incomplete = await reconcileSpend(ledger, reconciler({ byCategory: { agent_runner: 0.1 }, groups: 1, complete: false, note: 'still estimated' }).r, optId, { now: ready });
    expect(incomplete).toMatchObject({ status: 'pending', reason: 'still estimated' });
    expect(incomplete.lines.find((l) => l.category === 'agent_runner')).toMatchObject({ ledgerUsd: 1, actualUsd: 0.1, written: false });

    const empty = await reconcileSpend(ledger, reconciler({ byCategory: {}, groups: 0, complete: true }).r, optId, { now: ready });
    expect(empty.status).toBe('pending');
    expect(empty.reason).toMatch(/no cost data/);
    expect(corrections()).toEqual([]);
    expect(ledger.spendBreakdown(optId).byCategory.agent_runner).toBe(1); // not zeroed out
  });

  it('empty billing is fine when the ledger holds no infrastructure estimate', async () => {
    const o = ledger.createOptimization({ repo: 'a/b', baseRef: 'main', metric: 'p95', direction: 'minimize', minGain: 0.03, budgetUsd: 1, timeLimitS: 60, configYaml: 'v' }).id;
    ledger.setOptimizationStatus(o, 'stopped', endAt);
    const rep = await reconcileSpend(ledger, reconciler({ byCategory: {}, groups: 0, complete: true }).r, o, { now: ready });
    expect(rep.status).toBe('unchanged');
  });

  it('honours lagMs and propagates billing errors', async () => {
    ledger.setOptimizationStatus(optId, 'stopped', endAt);
    const ok = await reconcileSpend(ledger, reconciler(final({ agent_runner: 1, experiment: 2 })).r, optId, { now: endAt + 1, lagMs: 0 });
    expect(ok.status).toBe('unchanged');
    const boom: CostReconciler = { actuals: async () => { throw new Error('AccessDenied'); } };
    await expect(reconcileSpend(ledger, boom, optId, { now: ready })).rejects.toThrow('AccessDenied');
    await expect(reconcileSpend(ledger, boom, 'nope', { now: ready })).rejects.toThrow(/not found/);
  });
});
