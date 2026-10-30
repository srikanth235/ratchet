import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { estimateValue, openLedger, returnOnSpend, type Ledger } from '@ratchet/core';
import { buildSimulationSummary, prToPath, writeSimulationSummary, type SummaryInput } from './summary.js';

let dir: string;
let ledger: Ledger;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'ratchet-summary-'));
  ledger = openLedger(join(dir, 'ratchet.db'));
});
afterEach(() => {
  ledger.close();
  rmSync(dir, { recursive: true, force: true });
});

/** A finished run: baseline 100, one verified win (80), one rejected, one blocked, some spend. */
function finishedRun(): SummaryInput {
  const opt = ledger.createOptimization({ id: 'opt-1', repo: 'a/b', baseRef: 'main', metric: 'p95', direction: 'minimize', minGain: 0.03, budgetUsd: 50, timeLimitS: 600, configYaml: 'v' });
  ledger.recordBaseline({ optimizationId: opt.id, samples: [99, 100, 101], noiseFloor: 1, stats: { value: 100 } });
  const agent = ledger.addAgent({ optimizationId: opt.id, kind: 'claude-code', branch: 'ratchet/opt-1/cc' });
  const win = ledger.submitCandidate({ optimizationId: opt.id, agentId: agent.id, sha: 'aaa' });
  ledger.setVerdict(win.id, 'verified');
  ledger.moveRatchet(opt.id, win.id, 80);
  ledger.setVerdict(ledger.submitCandidate({ optimizationId: opt.id, agentId: agent.id, sha: 'bbb' }).id, 'rejected', ['no gain']);
  ledger.setVerdict(ledger.submitCandidate({ optimizationId: opt.id, agentId: agent.id, sha: 'ccc' }).id, 'blocked', ['protected path']);
  ledger.recordSpend({ optimizationId: opt.id, category: 'agent_runner', usd: 0.25, source: 'ec2:i-1' });
  ledger.recordSpend({ optimizationId: opt.id, category: 'tokens', usd: 0.75, source: 'agent' });
  ledger.recordSpend({ optimizationId: opt.id, category: 'agent_runner', usd: -0.05, estimated: false, source: 'cost-explorer' });
  ledger.setOptimizationStatus(opt.id, 'completed');

  const summary = ledger.listOptimizations().find((s) => s.id === opt.id)!;
  const config = { objective: { direction: 'minimize' as const, value: { usd_per_unit: 1000, period: 'month' as const, basis: 'estimated' as const, unit: 'ms' } } };
  const value = estimateValue(config as never, summary.baselineValue!, summary.bestValue!);
  return {
    project: 'search',
    pack: 'perf',
    objective: 'p95 latency',
    summary,
    candidates: ledger.candidatesWithResults(opt.id),
    spend: ledger.spendBreakdown(opt.id),
    value,
    roi: value ? returnOnSpend(value.usdPerYear, summary.spendUsd) : null,
    pr: pathToFileURL(join(dir, 'prs', 'opt-1.md')).href,
    workRoot: join(dir, '.ratchet-sim-x'),
    db: join(dir, 'ratchet.db'),
  };
}

describe('buildSimulationSummary', () => {
  it('captures project, baseline, best, gain, verdict counts, spend, value/ROI and the PR path', () => {
    const s = buildSimulationSummary(finishedRun());
    expect(s).toMatchObject({
      project: 'search',
      pack: 'perf',
      optimizationId: 'opt-1',
      objective: 'p95 latency',
      metric: 'p95',
      status: 'completed',
      baseline: 100,
      best: 80,
      wins: 1,
      candidates: 3,
      verdicts: { pending: 0, verified: 1, rejected: 1, failed_confirm: 0, blocked: 1 },
      spendUsd: 0.95,
      spend: { estimatedUsd: 1, actualUsd: -0.05, byCategory: { agent_runner: 0.2, tokens: 0.75, experiment: 0, bench_runner: 0, storage_egress: 0 } },
      value: { improvement: 20, unit: 'ms', usdPerYear: 240_000, basis: 'estimated' },
      prPath: join(dir, 'prs', 'opt-1.md'),
      db: join(dir, 'ratchet.db'),
    });
    expect(s.gainPct).toBeCloseTo(20, 9);
    expect(s.value!.returnOnSpend).toBeCloseTo(240_000 / 0.95, 6);
    expect(s.pr).toMatch(/^file:\/\//);
  });

  it('handles a run with no win, no value model and no PR', () => {
    const opt = ledger.createOptimization({ repo: 'a/b', baseRef: 'main', metric: 'p95', direction: 'minimize', minGain: 0.03, budgetUsd: 5, timeLimitS: 60, configYaml: 'v' });
    const summary = ledger.listOptimizations().find((s) => s.id === opt.id)!;
    const s = buildSimulationSummary({ project: 'forecast', pack: undefined, objective: 'p95', summary, candidates: [], spend: ledger.spendBreakdown(opt.id), value: null, roi: null, workRoot: 'w', db: 'd' });
    expect(s).toMatchObject({ pack: null, baseline: null, best: null, gainPct: null, wins: 0, value: null, pr: null, prPath: null, spendUsd: 0 });
    expect(Object.values(s.verdicts)).toEqual([0, 0, 0, 0, 0]);
  });

  it('prToPath converts file URLs and passes other URLs through', () => {
    expect(prToPath('file:///tmp/a%20b/pr.md')).toBe('/tmp/a b/pr.md');
    expect(prToPath('https://github.com/a/b/pull/7')).toBe('https://github.com/a/b/pull/7');
  });
});

describe('writeSimulationSummary', () => {
  it('writes pretty JSON, creating parent directories, and leaves no temp file behind', () => {
    const summary = buildSimulationSummary(finishedRun());
    const target = join(dir, 'out', 'deep', 'summary.json');
    const written = writeSimulationSummary(target, summary);
    expect(written).toBe(target);
    const text = readFileSync(target, 'utf8');
    expect(text.endsWith('\n')).toBe(true);
    expect(text).toContain('\n  "project": "search"');
    expect(JSON.parse(text)).toEqual(JSON.parse(JSON.stringify(summary)));
    expect(readdirSync(join(dir, 'out', 'deep'))).toEqual(['summary.json']);
    // overwriting works
    writeSimulationSummary(target, { ...summary, project: 'forecast' });
    expect(JSON.parse(readFileSync(target, 'utf8')).project).toBe('forecast');
  });
});
