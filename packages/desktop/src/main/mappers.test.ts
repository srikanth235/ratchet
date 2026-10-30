import { openLedger, type Ledger } from '@ratchet/core';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  agentEventsPage, buildActivity, buildCandidates, buildDetail, buildKpis, buildPipeline, buildReport,
  buildSeries, buildSpend, downsample, median, relTime, toOptimization, toUiStatus, unitForMetric, validateConfigText, type MetaView,
} from './mappers.js';
import { seedDemo } from './seed.js';

const NOW = Date.UTC(2026, 9, 30, 12, 0, 0);
const meta: MetaView = { prUrl: () => null, teardownDone: () => false };
let ledger: Ledger;

beforeEach(() => {
  ledger = openLedger(':memory:', { now: () => NOW });
  seedDemo(ledger, NOW);
});

describe('pure helpers', () => {
  it('derives display units from metric keys', () => {
    expect(unitForMetric('http_req_duration.p95')).toBe('ms');
    expect(unitForMetric('cold_start.s')).toBe('s');
    expect(unitForMetric('throughput.events_per_s')).toBe('k/s');
    expect(unitForMetric('peak_rss_mb')).toBe('MB');
    expect(unitForMetric('score')).toBe('');
  });

  it('formats relative time', () => {
    expect(relTime(NOW - 10_000, NOW)).toBe('just now');
    expect(relTime(NOW - 4 * 60_000, NOW)).toBe('4m ago');
    expect(relTime(NOW - 3 * 3_600_000, NOW)).toBe('3h ago');
    expect(relTime(NOW - 26 * 3_600_000, NOW)).toBe('yesterday');
    expect(relTime(NOW - 72 * 3_600_000, NOW)).toBe('3d ago');
  });

  it('median / downsample', () => {
    expect(median([3, 1, 2])).toBe(2);
    expect(median([1, 2, 3, 4])).toBe(2.5);
    expect(downsample([1, 2, 3], 8)).toEqual([1, 2, 3]);
    const d = downsample(Array.from({ length: 100 }, (_, i) => i), 8);
    expect(d).toHaveLength(8);
    expect(d[0]).toBe(0);
    expect(d.at(-1)).toBe(99);
  });

  it('maps ledger status to the UI status, adding pr-open once a PR exists', () => {
    expect(toUiStatus('completed', null)).toBe('completed');
    expect(toUiStatus('completed', 'https://github.com/a/b/pull/1')).toBe('pr-open');
    expect(toUiStatus('running', 'x')).toBe('running');
  });
});

describe('ledger rows → view models', () => {
  it('maps optimization summaries (direction, baseline→best, gain, agents)', () => {
    const rows = ledger.listOptimizations().map((r) => toOptimization(r, meta, NOW));
    const search = rows.find((r) => r.id === 'opt_demo_search')!;
    expect(search).toMatchObject({ name: 'search-api p95 latency', unit: 'ms', direction: 'lower', baseline: 182, best: 131, status: 'running', updated: 'live', verifiedWins: 5, candidateCount: 11 });
    expect(search.gainPct).toBeCloseTo(28, 0);
    expect(search.agents).toEqual(['claude-code', 'codex']);
    const ingest = rows.find((r) => r.id === 'opt_demo_ingest')!;
    expect(ingest).toMatchObject({ direction: 'higher', baseline: 41.2, best: 58.9, status: 'completed' });
    expect(ingest.gainPct).toBeCloseTo(43, 0);
    const draft = rows.find((r) => r.id === 'opt_demo_web')!;
    expect(draft).toMatchObject({ status: 'draft', baseline: null, best: null, gainPct: null });
  });

  it('builds the ratchet series: baseline step + one step per move, scatter for every benched candidate', () => {
    const s = buildSeries(ledger, 'opt_demo_search', NOW);
    expect(s.steps.map((x) => x.value)).toEqual([182, 168, 156, 149, 138, 131]);
    expect(s.steps[0]!.t).toBe(0);
    expect(s.steps.every((x, i, a) => i === 0 || x.t >= a[i - 1]!.t)).toBe(true);
    expect(s.points).toHaveLength(11);
    expect(s.points.filter((p) => p.verified)).toHaveLength(5);
    expect(s.noiseFloor).toBe(3.1);
    expect(s.tMax).toBeGreaterThanOrEqual(214);
    expect(buildSeries(ledger, 'opt_demo_web', NOW).steps).toEqual([]);
  });

  it('maps candidates with verdicts, vs-baseline % and short shas', () => {
    const c = buildCandidates(ledger, 'opt_demo_search');
    expect(c).toHaveLength(11);
    expect(c.map((x) => x.verdict).filter((v) => v === 'verified')).toHaveLength(5);
    const blocked = c.find((x) => x.verdict === 'blocked')!;
    expect(blocked.reasons[0]).toContain('protected path');
    const last = c.at(-1)!;
    expect(last.sha).toHaveLength(7);
    expect(last.vsBaselinePct).toBeCloseTo(28, 0);
    expect(last.movedRatchet).toBe(true);
    expect(c.find((x) => x.verdict === 'failed_confirm')!.confirmValue).toBeGreaterThan(150);
  });

  it('maps spend with category labels, per-agent totals and a cumulative series', () => {
    const s = buildSpend(ledger, 'opt_demo_search');
    expect(s.total).toBeCloseTo(42.6, 1);
    expect(s.byCategory[0]!.label).toBe('Model tokens');
    expect(s.byAgent.map((a) => a.kind).sort()).toEqual(['claude-code', 'codex']);
    expect(s.budgetUsd).toBe(120);
    expect(s.series.at(-1)!.usd).toBeCloseTo(s.total, 4);
    expect(s.series.every((p, i, a) => i === 0 || p.usd >= a[i - 1]!.usd)).toBe(true);
  });

  it('merges activity newest-first without duplicating verified verdicts', () => {
    const a = buildActivity(ledger, NOW, 6, 'opt_demo_search');
    expect(a[0]!.text).toContain('Verified win');
    expect(a[0]!.text).toContain('138 → 131 ms');
    expect(a[0]!.tone).toBe('mint');
    expect(a.filter((x) => x.text.startsWith('Candidate verified'))).toHaveLength(0);
    const all = a.map((x) => x.atMs);
    expect([...all].sort((x, y) => y - x)).toEqual(all);
  });

  it('computes KPIs from all optimizations', () => {
    const kpis = buildKpis(ledger, ledger.listOptimizations(), NOW);
    expect(kpis.map((k) => k.id)).toEqual(['wins', 'gain', 'spend', 'cpw']);
    expect(kpis[0]!.value).toBe('8');
    expect(kpis[2]!.value).toBe('$143');
    expect(kpis[3]!.value).toBe('$17.8');
    expect(kpis.every((k) => k.trend.length > 1)).toBe(true);
  });

  it('builds agent cards: hypothesis = latest agent message, recent evals, stats', () => {
    const d = buildDetail(ledger, 'opt_demo_search', meta, NOW);
    expect(d.agents).toHaveLength(2);
    const cc = d.agents.find((a) => a.kind === 'claude-code')!;
    expect(cc.hypothesis).toBe('Short-circuit empty queries before the index');
    expect(cc.recent.at(-1)).toMatchObject({ value: 131, verdict: 'verified' });
    expect(cc.stats.verified).toBe(3);
    expect(cc.stats.spendUsd).toBeGreaterThan(0);
    expect(d.elapsedS).toBeGreaterThan(0);
    expect(d.teardown.find((t) => t.id === 'agents')!.done).toBe(false);
  });

  it('ignores user messages when picking the hypothesis', () => {
    const agent = ledger.listAgents('opt_demo_search')[0]!;
    ledger.appendAgentEvent({ agentId: agent.id, type: 'message', payload: { text: 'You: try caching', data: { from: 'user' } }, at: NOW });
    const d = buildDetail(ledger, 'opt_demo_search', meta, NOW);
    expect(d.agents[0]!.hypothesis).not.toContain('You:');
  });

  it('pages agent events with a cursor (tail first, then only newer)', () => {
    const agent = ledger.listAgents('opt_demo_search')[0]!;
    const first = agentEventsPage(ledger, agent.id, undefined, 5);
    expect(first.events).toHaveLength(5);
    expect(first.events[0]!.id).toBeLessThan(first.events[4]!.id);
    expect(agentEventsPage(ledger, agent.id, first.cursor).events).toEqual([]);
    ledger.appendAgentEvent({ agentId: agent.id, type: 'status', payload: { text: 'hello' }, at: NOW });
    const next = agentEventsPage(ledger, agent.id, first.cursor);
    expect(next.events.map((e) => e.text)).toEqual(['hello']);
    expect(next.cursor).toBeGreaterThan(first.cursor);
    expect(() => agentEventsPage(ledger, 'nope', 0)).toThrow(/not found/);
  });

  it('derives the bench pipeline from the newest candidate', () => {
    const idle = buildPipeline([]);
    expect(idle.stages.every((s) => s.state === 'idle')).toBe(true);
    const cands = ledger.candidatesWithResults('opt_demo_search');
    const done = buildPipeline(cands);
    expect(done.candidate?.agent).toBe('claude-code');
    expect(done.stages.map((s) => s.state)).toEqual(['done', 'done', 'done', 'done', 'done']);

    const agent = ledger.listAgents('opt_demo_search')[0]!;
    const c = ledger.submitCandidate({ optimizationId: 'opt_demo_search', agentId: agent.id, sha: 'f'.repeat(40), message: 'wip', submittedAt: NOW });
    const active = buildPipeline(ledger.candidatesWithResults('opt_demo_search'));
    expect(active.candidate?.id).toBe(c.id);
    expect(active.stages.find((s) => s.id === 'guardrails')!.state).toBe('active');
    expect(active.stages.find((s) => s.id === 'verdict')!.state).toBe('active');
  });

  it('renders a markdown report', () => {
    const md = buildReport(ledger, 'opt_demo_search', meta, NOW);
    expect(md).toContain('# Ratchet report: search-api p95 latency');
    expect(md).toContain('182 ms → 131 ms');
    expect(md).toContain('5 verified wins out of 11 candidates');
    expect(md).toMatch(/\| [0-9a-f]{7} \| codex \| blocked/);
  });
});

describe('validateConfigText', () => {
  it('summarises a valid config and reports path-qualified errors for a bad one', () => {
    const ok = validateConfigText(`version: 0
objective: { metric: a.b, direction: minimize, min_gain: 0.05 }
bench: { ref: r, run: ./bench, repeats: 3, hardware: c7i.large }
agent: { instructions: RATCHET.md, allowed_instance_types: [c7i.large] }
`);
    expect(ok).toMatchObject({ ok: true, summary: { metric: 'a.b', repeats: 3, minGain: 0.05 } });
    const bad = validateConfigText('version: 0\nobjective: { metric: a, direction: sideways, min_gain: 2 }');
    expect(bad.ok).toBe(false);
    if (!bad.ok) {
      expect(bad.errors.some((e) => e.path === 'objective.direction')).toBe(true);
      expect(bad.errors.length).toBeGreaterThan(1);
    }
  });
});
