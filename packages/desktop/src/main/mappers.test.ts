import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { openLedger, type Ledger } from '@ratchet/core';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  agentEventsPage, buildActivity, buildCandidates, buildDetail, buildKpis, buildPipeline, buildReport,
  buildSeries, buildSpend, downsample, hunkExcerpt, median, relTime, toOptimization, toUiStatus, toWinningChange, unitForMetric, validateConfigText, type MetaView,
} from './mappers.js';
import { metricOptions } from './metrics.js';
import { seedDemo } from './seed.js';

const NOW = Date.UTC(2026, 9, 30, 12, 0, 0);
const meta: MetaView = { prUrl: () => null };
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
    const rows = ledger.listOptimizations().map((r) => toOptimization(r, ledger.getOptimization(r.id)!, meta, NOW));
    const search = rows.find((r) => r.id === 'opt_demo_search')!;
    expect(search).toMatchObject({ name: 'search-api p95 latency', unit: 'ms', direction: 'lower', baseline: 182, best: 131, status: 'running', updated: 'live', verifiedWins: 5, candidateCount: 11 });
    expect(search.gainPct).toBeCloseTo(28, 0);
    expect(search.agents).toEqual(['claude-code', 'codex']);
    const ingest = rows.find((r) => r.id === 'opt_demo_ingest')!;
    expect(ingest).toMatchObject({ direction: 'higher', baseline: 41.2, best: 58.9, status: 'completed' });
    expect(ingest.gainPct).toBeCloseTo(43, 0);
    const draft = rows.find((r) => r.id === 'opt_demo_web')!;
    expect(draft).toMatchObject({ status: 'draft', baseline: null, best: null, gainPct: null, value: null });
  });

  it('values the improvement from the ledger value columns (core estimateValue / returnOnSpend)', () => {
    const rows = ledger.listOptimizations().map((r) => toOptimization(r, ledger.getOptimization(r.id)!, meta, NOW));
    const search = rows.find((r) => r.id === 'opt_demo_search')!;
    // 182 → 131 ms at $1,200 per ms per year
    expect(search.value).toMatchObject({ usdPerUnit: 1200, unit: 'ms', period: 'year', basis: 'estimated', usdPerYear: 51 * 1200 });
    expect(search.value!.returnOnSpend).toBeCloseTo((51 * 1200) / search.spendUsd, 6);
    expect(search).toMatchObject({ pack: 'perf', evaluatorKind: 'benchmark' });
    // 41.2 → 58.9 k/s at $400 per k/s per year; the draft has no baseline to value
    expect(rows.find((r) => r.id === 'opt_demo_ingest')!.value).toMatchObject({ usdPerUnit: 400, unit: 'k/s', usdPerYear: expect.closeTo(17.7 * 400, 6) });
    expect(rows.find((r) => r.id === 'opt_demo_web')!.value).toBeNull();
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

  it('computes range KPIs with change chips vs the previous equal-length range', () => {
    const kpis = buildKpis(ledger, ledger.listOptimizations(), NOW, '30d');
    const k = Object.fromEntries(kpis.map((x) => [x.id, x]));
    expect(kpis.map((x) => x.id)).toEqual(['wins', 'gain', 'spend', 'cpw', 'value']);
    // last 30d: search (5 wins) + ingest (2); the 30d before: edge worker (1 win, 41 days ago)
    expect(k.wins).toMatchObject({ value: '7', unit: 'last 30d', change: { text: '+6', tone: 'good' }, foot: 'across 2 optimizations' });
    // median of 28.0% and 43.0% vs 32.8% before
    expect(k.gain).toMatchObject({ value: '35', change: { text: '+2 pts', tone: 'good' } });
    expect(k.spend!.value).toBe('$104');
    expect(k.spend!.change).toEqual({ text: '+167%', tone: 'neutral' });
    expect(k.cpw).toMatchObject({ value: '$15', change: { text: '−62%', tone: 'good' } });
    // 51 ms of p95 at $1,200 per ms per year + 17.7 k/s of ingest at $400; the edge worker (previous range) is valued too
    expect(k.value).toMatchObject({ value: '$68.3k', change: { text: '+2781%', tone: 'good' } });
    expect(k.value!.foot).toMatch(/^per year · \d+× ROI · estimated$/);
    expect(kpis.every((x) => x.trend.length > 1)).toBe(true);
  });

  it('omits change chips when the previous range has no data, and counts older wins in a longer range', () => {
    const week = buildKpis(ledger, ledger.listOptimizations(), NOW, '7d');
    expect(week.every((x) => x.change === null)).toBe(true);
    expect(week[0]!.value).toBe('7');
    const quarter = buildKpis(ledger, ledger.listOptimizations(), NOW, '90d');
    expect(quarter[0]).toMatchObject({ value: '8', unit: 'last 90d', change: null });
  });

  it('drops the value KPI when no optimization has a value model; empty ranges say so', () => {
    const empty = openLedger(':memory:', { now: () => NOW });
    const kpis = buildKpis(empty, [], NOW, '30d');
    expect(kpis.map((x) => x.id)).toEqual(['wins', 'gain', 'spend', 'cpw']);
    expect(kpis.map((x) => x.value)).toEqual(['0', '—', '$0.0', '—']);
    expect(kpis.every((x) => x.change === null && !/NaN|undefined/.test(`${x.value}${x.foot}`))).toBe(true);
  });

  it('lists the objective family as metric options: siblings for nested metrics, non-guardrail metrics otherwise', () => {
    const d = buildDetail(ledger, 'opt_demo_search', meta, NOW);
    expect(d.metrics.map((m) => m.label)).toEqual(['p50', 'p95', 'p99']);
    expect(d.metrics.find((m) => m.objective)!.key).toBe('http_req_duration.p95');
    const forecastYaml = readFileSync(resolve(__dirname, '../../../../examples/demo-forecast/ratchet.yaml'), 'utf8');
    const forecast = metricOptions('wape', forecastYaml, {
      value: 43.5, 'metric:wape': 43.5, 'metric:mape': 51, 'metric:bias': -2, 'metric:train_ms': 0.4, 'metric:leakage_violations': 0,
    });
    // top-level objective: the other top-level metrics, minus the guardrails (train_ms, leakage_violations)
    expect(forecast.map((m) => m.key)).toEqual(['wape', 'mape', 'bias']);
    const ingest = buildDetail(ledger, 'opt_demo_ingest', meta, NOW);
    expect(ingest.metrics).toHaveLength(1);
  });

  it('compares baseline vs best for the family and the guardrail metrics, and the span timings', () => {
    const d = buildDetail(ledger, 'opt_demo_search', meta, NOW);
    expect(d.compare.map((r) => r.key)).toEqual(['http_req_duration.p50', 'http_req_duration.p95', 'http_req_duration.p99', 'error_rate', 'peak_rss_mb']);
    const p95 = d.compare[1]!;
    expect(p95).toMatchObject({ label: 'p95', unit: 'ms', baseline: 182, best: 131, better: 'lower' });
    expect(p95.deltaPct).toBeCloseTo(-28.02, 1);
    expect(d.compare.find((r) => r.key === 'peak_rss_mb')).toMatchObject({ unit: 'MB', better: 'lower' });
    expect(d.spans!.map((r) => r.name)).toEqual(['parse', 'search', 'serialize', 'wait']);
    const search = d.spans!.find((r) => r.name === 'search')!;
    expect(search.baseline).toBeGreaterThan(search.best);
    // no win yet → nothing to compare, no spans
    const draft = buildDetail(ledger, 'opt_demo_web', meta, NOW);
    expect(draft).toMatchObject({ compare: [], spans: null, metrics: [] });
  });

  it('plots another metric of the family: best line follows the verified commits, no noise band', () => {
    const s = buildSeries(ledger, 'opt_demo_search', NOW, 'http_req_duration.p99');
    expect(s.metric).toBe('http_req_duration.p99');
    expect(s.options.map((o) => o.label)).toEqual(['p50', 'p95', 'p99']);
    expect(s.steps).toHaveLength(6);
    expect(s.steps[0]!.value).toBeCloseTo(182 * 1.58, 2);
    expect(s.noiseFloor).toBeNull();
    expect(buildSeries(ledger, 'opt_demo_search', NOW, 'not-a-metric').metric).toBe('http_req_duration.p95');
  });

  it('builds the winning change from a diff, or says why there is none', () => {
    const files = [
      { path: 'src/search.ts', additions: 40, deletions: 12, binary: false, patch: '@@ -1,3 +1,4 @@ ctx\n a\n-b\n+c\n+d\n\\ No newline at end of file' },
      { path: 'logo.png', additions: 0, deletions: 0, binary: true, patch: '' },
      { path: 'src/filter.ts', additions: 2, deletions: 1, binary: false, patch: '@@ -9 +9 @@\n-x\n+y' },
    ];
    const w = toWinningChange(ledger, 'opt_demo_search', files, null);
    expect(w).toMatchObject({ commits: 5, baseRef: 'main', additions: 42, deletions: 13, unavailable: null });
    expect(w.files.map((f) => f.path)).toEqual(['src/search.ts', 'logo.png', 'src/filter.ts']);
    expect(w.excerpt!.path).toBe('src/search.ts');
    expect(w.excerpt!.lines.map((l) => l.kind)).toEqual(['hunk', 'ctx', 'del', 'add', 'add']);
    expect(toWinningChange(ledger, 'opt_demo_search', null, 'no git')).toMatchObject({ commits: 5, files: [], unavailable: 'no git' });
    expect(toWinningChange(ledger, 'opt_demo_web', null, null)).toMatchObject({ commits: 0, sha: null, unavailable: 'No verified win yet.' });
    expect(hunkExcerpt('no hunk')).toEqual([]);
  });

  it('builds agent cards: hypothesis = latest agent message, recent evals, stats', () => {
    const d = buildDetail(ledger, 'opt_demo_search', meta, NOW);
    expect(d.agents).toHaveLength(2);
    const cc = d.agents.find((a) => a.kind === 'claude-code')!;
    expect(cc.hypothesis).toBe('Binary-search the sorted vocabulary for prefix terms');
    expect(cc.recent.at(-1)).toMatchObject({ value: 131, verdict: 'verified' });
    expect(cc.stats.verified).toBe(3);
    expect(cc.stats.spendUsd).toBeGreaterThan(0);
    expect(d.elapsedS).toBeGreaterThan(0);
    expect(d.receipt).toBeNull(); // the receipt comes from the engine, not the ledger
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
    expect(ok).toMatchObject({ ok: true, summary: { metric: 'a.b', minGain: 0.05, evaluator: { kind: 'benchmark', ref: 'r', run: './bench', repeats: 3, holdout: null }, value: null, name: null } });
    const bad = validateConfigText('version: 0\nobjective: { metric: a, direction: sideways, min_gain: 2 }');
    expect(bad.ok).toBe(false);
    if (!bad.ok) {
      expect(bad.errors.some((e) => e.path === 'objective.direction')).toBe(true);
      expect(bad.errors.length).toBeGreaterThan(1);
    }
  });
});
