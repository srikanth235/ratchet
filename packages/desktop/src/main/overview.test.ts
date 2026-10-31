import { openLedger, optimizations, type Ledger } from '@ratchet/core';
import { beforeEach, describe, expect, it } from 'vitest';
import { type MetaView } from './mappers.js';
import { buildOverview } from './overview.js';
import { seedDemo } from './seed.js';

const NOW = Date.UTC(2026, 9, 30, 12, 0, 0);
const DAY = 24 * 3600_000;
const noPr: MetaView = { prUrl: () => null };
let ledger: Ledger;

beforeEach(() => {
  ledger = openLedger(':memory:', { now: () => NOW });
  seedDemo(ledger, NOW);
});

const totalSpend = (id: string): number => ledger.listOptimizations().find((o) => o.id === id)!.spendUsd;

describe('buildOverview: the numbers', () => {
  it('sums spend and value over the range, and relates them (return per $1, payback in days)', () => {
    const o = buildOverview(ledger, noPr, NOW, '30d');
    // search + ingest finished or run inside 30 days; the edge worker is 41 days old
    expect(o.spent.usd).toBeCloseTo(totalSpend('opt_demo_search') + totalSpend('opt_demo_ingest'), 3);
    // search: 182 → 131 ms at $1,200 per ms; ingest: 41.2 → 58.9 k/s at $400 per k/s
    expect(o.value).toEqual({ usdPerYear: expect.closeTo(51 * 1200 + 17.7 * 400, 3), wins: 7, basis: 'estimated' });
    expect(o.returnOnSpend!.ratio).toBeCloseTo(o.value!.usdPerYear / o.spent.usd, 6);
    expect(o.returnOnSpend!.paybackDays).toBeCloseTo((o.spent.usd / o.value!.usdPerYear) * 365, 6);
    expect(o.spent.budgetUsd).toBe(120 + 100); // search and ingest (a draft has no budget to spend yet)
    expect(o.spent.optimizations).toBe(2);
    expect(o.empty).toBe(false);
  });

  it('filters by range: 90 days adds the older optimization, all time starts at the first activity', () => {
    const d30 = buildOverview(ledger, noPr, NOW, '30d');
    const d90 = buildOverview(ledger, noPr, NOW, '90d');
    const all = buildOverview(ledger, noPr, NOW, 'all');
    expect(d30.rows.map((r) => r.id)).toEqual(['opt_demo_search', 'opt_demo_ingest', 'opt_demo_web']);
    expect(d90.rows.map((r) => r.id)).toEqual(['opt_demo_search', 'opt_demo_ingest', 'opt_demo_web', 'opt_demo_edge']);
    expect(d90.value!.wins).toBe(8);
    expect(d90.value!.usdPerYear).toBeCloseTo(d30.value!.usdPerYear + 0.79 * 3000, 3);
    expect(d90.from).toBe(NOW - 90 * DAY);
    expect(all.from).toBe(NOW - 41 * DAY + 3 * 60_000); // the edge optimization started first (its baseline took 3 minutes)
    expect(all.spent.usd).toBeCloseTo(d90.spent.usd, 6);
    expect(buildOverview(ledger, noPr, NOW, '30d').range).toBe('30d');
    // a window with nothing in it
    const quiet = buildOverview(ledger, noPr, NOW + 400 * DAY, '30d');
    expect(quiet.spent.usd).toBe(0);
    expect(quiet.value).toMatchObject({ usdPerYear: 0, wins: 0 });
    expect(quiet.returnOnSpend).toBeNull();
    expect(quiet.wins).toEqual([]);
  });

  it('the table adds up to the hero: per-row spend and value sum to the totals', () => {
    for (const range of ['30d', '90d', 'all'] as const) {
      const o = buildOverview(ledger, noPr, NOW, range);
      expect(o.rows.reduce((a, r) => a + r.spentUsd, 0)).toBeCloseTo(o.spent.usd, 6);
      expect(o.rows.reduce((a, r) => a + (r.valueUsdPerYear ?? 0), 0)).toBeCloseTo(o.value!.usdPerYear, 6);
    }
  });

  it('the staircase is the cumulative value of the wins in the range, oldest first, naming each optimization', () => {
    const o = buildOverview(ledger, noPr, NOW, '90d');
    expect(o.wins).toHaveLength(8);
    expect(o.wins.every((w, i, a) => i === 0 || (w.at >= a[i - 1]!.at && w.cumulativeUsdPerYear > a[i - 1]!.cumulativeUsdPerYear))).toBe(true);
    expect(o.wins.at(-1)!.cumulativeUsdPerYear).toBeCloseTo(o.value!.usdPerYear, 6);
    expect(o.wins[0]).toMatchObject({ optimization: 'Edge worker cold start', usdPerYear: expect.closeTo(0.79 * 3000, 6) });
    expect(new Set(o.wins.map((w) => w.optimization))).toEqual(new Set(['Edge worker cold start', 'Ingest throughput', 'search-api p95 latency']));
  });

  it('a row says what happened: gain, baseline → best, wins, status word, time left while running', () => {
    const o = buildOverview(ledger, noPr, NOW, '30d');
    const search = o.rows.find((r) => r.id === 'opt_demo_search')!;
    expect(search).toMatchObject({ statusLabel: 'Running', tone: 'run', live: true, unit: 'ms', direction: 'lower', baseline: 182, best: 131, verifiedWins: 5, budgetUsd: 120 });
    expect(search.gainPct).toBeCloseTo(28, 0);
    expect(search.remainingS).toBe(8 * 3600 - 221 * 60); // time limit minus the 221 minutes since it went live
    expect(search.valueUsdPerYear).toBeCloseTo(51 * 1200, 3);
    expect(o.rows[0]!.id).toBe('opt_demo_search'); // live first
    expect(o.rows.find((r) => r.id === 'opt_demo_ingest')).toMatchObject({ statusLabel: 'Waiting on you', tone: 'wait', remainingS: null, live: false });
    expect(o.rows.find((r) => r.id === 'opt_demo_web')).toMatchObject({ statusLabel: 'Draft', tone: 'off', baseline: null, gainPct: null, valueUsdPerYear: null });
  });
});

describe('buildOverview: no value model, no optimizations', () => {
  it('shows no value and no return when nothing has a value model, but still adds up the spend', () => {
    ledger.db.update(optimizations).set({ valueUsdPerUnit: null }).run();
    const o = buildOverview(ledger, noPr, NOW, '90d');
    expect(o.value).toBeNull();
    expect(o.returnOnSpend).toBeNull();
    expect(o.wins).toEqual([]);
    expect(o.spent.usd).toBeGreaterThan(100);
    expect(o.rows.every((r) => r.valueUsdPerYear === null)).toBe(true);
    expect(o.rows.find((r) => r.id === 'opt_demo_search')!.verifiedWins).toBe(5);
  });

  it('an empty ledger is all zeros and empty lists, over any range', () => {
    const empty = openLedger(':memory:', { now: () => NOW });
    for (const range of ['30d', '90d', 'all'] as const) {
      const o = buildOverview(empty, noPr, NOW, range);
      expect(o).toMatchObject({ range, empty: true, spent: { usd: 0, budgetUsd: 0, optimizations: 0 }, value: null, returnOnSpend: null, wins: [], needsYou: [], rows: [] });
      expect(o.from).toBeLessThan(o.to);
    }
  });

  it('measured only when every contributing model is measured', () => {
    ledger.db.update(optimizations).set({ valueBasis: 'measured' }).run();
    expect(buildOverview(ledger, noPr, NOW, '90d').value!.basis).toBe('measured');
    (ledger.db as unknown as { $client: { prepare(sql: string): { run(): void } } }).$client.prepare("update optimizations set value_basis = 'estimated' where id = 'opt_demo_ingest'").run();
    expect(buildOverview(ledger, noPr, NOW, '90d').value!.basis).toBe('estimated');
  });
});

describe('buildOverview: needs you', () => {
  it('lists finished runs with verified wins and no pull request, whatever the range; "Waiting on you" in the table', () => {
    const o = buildOverview(ledger, noPr, NOW, '30d');
    expect(o.needsYou.map((n) => [n.kind, n.optimizationId, n.title, n.sub])).toEqual([
      ['ship', 'opt_demo_ingest', 'Open a pull request for 2 verified wins', 'Ingest throughput · +43%'],
      ['ship', 'opt_demo_edge', 'Open a pull request for 1 verified win', 'Edge worker cold start · −33%'],
    ]);
    expect(o.needsYou.every((n) => n.action === 'Review')).toBe(true);
  });

  it('a pull request takes it off the list and the status reads "PR open"', () => {
    const withPr: MetaView = { prUrl: (id) => (id === 'opt_demo_ingest' ? 'https://github.com/acme/ingest/pull/7' : null) };
    const o = buildOverview(ledger, withPr, NOW, '30d');
    expect(o.needsYou.map((n) => n.optimizationId)).toEqual(['opt_demo_edge']);
    expect(o.rows.find((r) => r.id === 'opt_demo_ingest')).toMatchObject({ status: 'pr-open', statusLabel: 'PR open', tone: 'ok' });
  });

  it('a recent failed run asks for a look; an old one stops asking', () => {
    ledger.setOptimizationStatus('opt_demo_ingest', 'failed', NOW - 2 * DAY);
    const o = buildOverview(ledger, noPr, NOW, '30d');
    expect(o.needsYou.find((n) => n.kind === 'failed')).toMatchObject({ optimizationId: 'opt_demo_ingest', title: 'A run failed', sub: 'Ingest throughput', action: 'View' });
    expect(o.rows.find((r) => r.id === 'opt_demo_ingest')).toMatchObject({ statusLabel: 'Failed', tone: 'bad' });
    ledger.setOptimizationStatus('opt_demo_ingest', 'failed', NOW - 20 * DAY);
    expect(buildOverview(ledger, noPr, NOW, '30d').needsYou.some((n) => n.kind === 'failed')).toBe(false);
  });

  it('a finished run without a win reads "No win" and is not waiting on anyone', () => {
    ledger.setOptimizationStatus('opt_demo_web', 'completed', NOW - DAY);
    const o = buildOverview(ledger, noPr, NOW, '30d');
    expect(o.needsYou.some((n) => n.optimizationId === 'opt_demo_web')).toBe(false);
    expect(o.rows.find((r) => r.id === 'opt_demo_web')).toMatchObject({ statusLabel: 'No win', tone: 'off', verifiedWins: 0 });
  });
});
