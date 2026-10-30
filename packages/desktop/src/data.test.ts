import { describe, expect, it } from 'vitest';
import { mockSnapshot } from './main/mock';
import { gainPct } from './renderer/format';

describe('mock optimizations snapshot (IPC payload shape)', () => {
  it('has 4 KPIs with sparkline trends', () => {
    expect(mockSnapshot.kpis).toHaveLength(4);
    for (const k of mockSnapshot.kpis) expect(k.trend.length).toBeGreaterThan(1);
  });

  it('contains the four expected optimizations', () => {
    const byName = Object.fromEntries(mockSnapshot.optimizations.map((o) => [o.name, o]));
    expect(byName['search-api p95 latency']).toMatchObject({ baseline: 182, best: 131, status: 'running' });
    expect(byName['Ingest throughput']).toMatchObject({ baseline: 41.2, best: 58.9, status: 'pr-open' });
    expect(byName['Edge worker cold start']).toMatchObject({ baseline: 2.41, best: 1.62, status: 'merged' });
    expect(byName['Web render p99']).toMatchObject({ status: 'draft' });
  });

  it('features an existing optimization and is JSON-serialisable (structured-clone safe)', () => {
    expect(mockSnapshot.optimizations.some((o) => o.id === mockSnapshot.featuredId)).toBe(true);
    expect(JSON.parse(JSON.stringify(mockSnapshot))).toEqual(mockSnapshot);
  });

  it('computes direction-aware gain', () => {
    const [search, ingest] = mockSnapshot.optimizations;
    expect(gainPct(search!)).toBeCloseTo(28, 0);
    expect(gainPct(ingest!)).toBeCloseTo(43, 0);
  });
});
