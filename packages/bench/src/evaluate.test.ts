import type { BenchOutcome, BenchRun } from '@ratchet/core';
import { describe, expect, it } from 'vitest';
import { benchStats, evaluateCandidate, metricsFromStats } from './evaluate.js';
import { makeConfig } from './testkit/repo.js';

const config = makeConfig();
const run = (value: number, extra: Record<string, number> = {}): BenchRun => ({ value, metrics: { 'http_req_duration.p95': value, ...extra }, durationMs: 10 });
const outcome = (values: number[], guardrails: BenchOutcome['guardrails'] = [], extra: Record<string, number> = {}): BenchOutcome => ({
  runs: values.map((v) => run(v, extra)),
  samples: values,
  guardrails,
  startedAt: 1000,
  finishedAt: 2000,
});
const base = { config, currentBest: 100, noiseFloor: 1, protectedViolations: [] };

describe('evaluateCandidate', () => {
  it('rejects a noise-level change', () => {
    const r = evaluateCandidate({ ...base, bench: outcome([99.5, 99.8, 99.6]) });
    expect(r).toMatchObject({ verdict: 'rejected', ledgerVerdict: 'rejected', needsConfirm: false, verified: false });
    expect(r.results.map((x) => x.phase)).toEqual(['bench']);
  });

  it('asks for a confirm run on a bench win, then verifies it', () => {
    const win = outcome([80, 81, 80]);
    const pending = evaluateCandidate({ ...base, bench: win });
    expect(pending).toMatchObject({ verdict: 'pending_confirm', ledgerVerdict: 'pending', needsConfirm: true, verified: false });

    const done = evaluateCandidate({ ...base, bench: win, confirm: outcome([85, 86]), confirmBest: 120 });
    expect(done).toMatchObject({ verdict: 'verified', ledgerVerdict: 'verified', verified: true, needsConfirm: false });
    expect(done.outcome.confirmImprovement).toBeCloseTo(34.5);
    expect(done.results.map((x) => x.phase)).toEqual(['bench', 'confirm']);
  });

  it('fails a bench win that does not hold on the hidden trace', () => {
    const r = evaluateCandidate({ ...base, bench: outcome([80, 80, 80]), confirm: outcome([130, 131]), confirmBest: 120 });
    expect(r).toMatchObject({ verdict: 'failed_confirm', ledgerVerdict: 'failed_confirm' });
  });

  it('blocks on a failed guardrail from either run, or a protected violation, whatever the speed', () => {
    const fast = [10, 10, 10];
    const g1 = evaluateCandidate({ ...base, bench: outcome(fast, [{ name: 'error_rate < 0.001', passed: false, detail: 'x' }]) });
    expect(g1).toMatchObject({ verdict: 'blocked', ledgerVerdict: 'blocked' });
    expect(g1.reasons[0]).toContain('error_rate < 0.001');

    const g2 = evaluateCandidate({ ...base, bench: outcome(fast), confirm: outcome(fast, [{ name: 'peak_rss < 1.2x', passed: false }]) });
    expect(g2.verdict).toBe('blocked');
    expect(g2.guardrails.map((g) => g.name)).toEqual(['peak_rss < 1.2x']);

    const v = evaluateCandidate({ ...base, bench: outcome(fast), protectedViolations: [{ path: 'bench/k6/search.js', pattern: 'bench/**' }] });
    expect(v.verdict).toBe('blocked');
    expect(v.reasons.join()).toContain('bench/k6/search.js');
  });

  it('blocks (without calling the rule) when the bench produced no samples, e.g. a command guardrail failed', () => {
    const r = evaluateCandidate({
      ...base,
      bench: { runs: [], samples: [], guardrails: [{ name: 'pnpm test', passed: false, detail: 'exited with status 1' }], startedAt: 1, finishedAt: 2 },
    });
    expect(r.verdict).toBe('blocked');
    expect(r.reasons).toEqual(['guardrail failed: pnpm test (exited with status 1)']);
    expect(Number.isNaN(r.outcome.candidateValue)).toBe(true);
    expect(r.outcome.requiredGain).toBe(3); // max(noise 1, 3% of 100)
    expect(r.results).toEqual([]); // nothing to store: a ledger row needs a finite value
  });

  it('maximize works through core', () => {
    const cfg = { objective: { ...config.objective, direction: 'maximize' as const } };
    const r = evaluateCandidate({ ...base, config: cfg, bench: outcome([120, 121, 120]), confirm: outcome([125]) });
    expect(r.verdict).toBe('verified');
  });
});

describe('benchStats', () => {
  it('has the headline value, summary and per-metric medians', () => {
    const s = benchStats(outcome([10, 12, 11], [], { peak_rss_mb: 200 }))!;
    expect(s).toMatchObject({ value: 11, n: 3, median: 11, min: 10, max: 12, 'metrics.peak_rss_mb': 200, 'metrics.http_req_duration.p95': 11 });
    expect(metricsFromStats(s)).toEqual({ peak_rss_mb: 200, 'http_req_duration.p95': 11 });
  });
  it('is undefined without samples and drops non-finite entries', () => {
    expect(benchStats(outcome([]))).toBeUndefined();
    expect(Object.values(benchStats(outcome([0, 0], [], {}))!).every(Number.isFinite)).toBe(true);
    const s = benchStats(outcome([-1, 1]))!; // mean 0 -> cv Infinity
    expect(s).not.toHaveProperty('cv');
  });
  it('produces ledger-shaped records', () => {
    const r = evaluateCandidate({ ...base, bench: outcome([80, 81, 80]) });
    expect(r.results[0]).toMatchObject({ phase: 'bench', samples: [80, 81, 80], startedAt: 1000, finishedAt: 2000, stats: { value: 80 } });
  });
});
