import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { openLedger, type Ledger } from '@ratchet/core';
import { buildResultReport, buildSquashPlanFromLedger } from './index.js';

let t = 0;
let ledger: Ledger;
beforeEach(() => {
  t = 1_000;
  ledger = openLedger(':memory:', { now: () => ++t });
});
afterEach(() => ledger.close());

const CONFIG = `version: 0
objective: { metric: http_req_duration.p95, direction: minimize, min_gain: 0.03 }
bench: { ref: ratchet/bench, run: k6 run bench/k6/search.js, repeats: 5, hardware: c7i.2xlarge }
agent: { instructions: RATCHET.md, allowed_instance_types: [c7i.2xlarge] }
`;

const sha = (c: string) => c.repeat(40);

function seed() {
  const opt = ledger.createOptimization({
    id: 'opt1', name: 'Demo search p95', repo: 'acme/search', baseRef: 'main', metric: 'http_req_duration.p95',
    direction: 'minimize', minGain: 0.03, budgetUsd: 100, timeLimitS: 3600, configYaml: CONFIG, createdAt: 1,
  });
  ledger.recordBaseline({
    optimizationId: opt.id, samples: [410, 412, 414], noiseFloor: 0.02, recordedAt: 2,
    stats: { value: 412, p50: 240, p95: 412, p99: 690, mean: 260 },
  });
  const claude = ledger.addAgent({ id: 'a1', optimizationId: opt.id, kind: 'claude-code', branch: 'ratchet/opt1/claude' });
  const codex = ledger.addAgent({ id: 'a2', optimizationId: opt.id, kind: 'codex', branch: 'ratchet/opt1/codex' });

  const cand = (id: string, agentId: string, c: string, message: string, at: number) =>
    ledger.submitCandidate({ id, optimizationId: opt.id, agentId, sha: sha(c), message, submittedAt: at });
  const bench = (id: string, phase: 'bench' | 'confirm', stats: Record<string, number> & { value: number }) =>
    ledger.recordBenchResult({ candidateId: id, phase, samples: [stats.value], stats, startedAt: 10, finishedAt: 11 });

  // spend before the first win
  ledger.recordSpend({ optimizationId: opt.id, agentId: claude.id, category: 'tokens', usd: 1.5, source: 't', at: 100, estimated: false });
  ledger.recordSpend({ optimizationId: opt.id, agentId: codex.id, category: 'tokens', usd: 0.75, source: 't', at: 101 });
  cand('c1', claude.id, 'a', 'Cache tokenizer\n\nbody', 200);
  bench('c1', 'bench', { value: 300, p50: 200, p95: 300, p99: 500, mean: 210 });
  bench('c1', 'confirm', { value: 305 });
  ledger.setVerdict('c1', 'verified', [], 210);
  ledger.moveRatchet(opt.id, 'c1', 300, 211);

  ledger.recordSpend({ optimizationId: opt.id, agentId: claude.id, category: 'experiment', usd: 2.25, source: 'ec2', at: 300 });
  ledger.recordSpend({ optimizationId: opt.id, agentId: codex.id, category: 'agent_runner', usd: 3, source: 'ec2', at: 301 });
  cand('c2', codex.id, 'b', 'Index postings | fast', 400);
  bench('c2', 'bench', { value: 180, p50: 150, p95: 180, p99: 300, mean: 160 });
  bench('c2', 'confirm', { value: 185 });
  ledger.setVerdict('c2', 'verified', [], 410);
  ledger.moveRatchet(opt.id, 'c2', 180, 411);

  cand('c3', claude.id, 'c', 'Tweak', 500);
  bench('c3', 'bench', { value: 178 });
  ledger.setVerdict('c3', 'rejected', ['no gain'], 510);
  cand('c4', claude.id, 'd', 'Touch bench', 520);
  ledger.setVerdict('c4', 'blocked', ['protected path'], 530);
  cand('c5', codex.id, 'e', 'Overfit', 540);
  ledger.setVerdict('c5', 'failed_confirm', [], 550);
  ledger.recordSpend({ optimizationId: opt.id, category: 'bench_runner', usd: 0.5, source: 'bench', at: 600 });
  return opt;
}

describe('buildResultReport', () => {
  it('summarizes the run as data', () => {
    const opt = seed();
    const { data } = buildResultReport(ledger, opt.id);
    expect(data.baseline?.value).toBe(412);
    expect(data.best).toMatchObject({ value: 180, sha: sha('b') });
    expect(data.gainPct).toBeCloseTo(56.31, 1);
    expect(data.wins.map((w) => [w.agentKind, w.benchValue, w.confirmValue, w.costUsd])).toEqual([
      ['claude-code', 300, 305, 1.5],
      ['codex', 180, 185, 3.75],
    ]);
    expect(data.verdicts).toEqual({ pending: 0, verified: 2, rejected: 1, failed_confirm: 1, blocked: 1 });
    expect(data.spend.total).toBeCloseTo(8, 5);
    expect(data.costPerVerifiedWinUsd).toBeCloseTo(4, 5);
    expect(data.agents.map((a) => [a.kind, a.candidates, a.verified])).toEqual([['claude-code', 3, 1], ['codex', 2, 1]]);
  });

  it('renders markdown', () => {
    const opt = seed();
    expect(buildResultReport(ledger, opt.id).markdown).toMatchSnapshot();
  });

  it('renders json that round-trips', () => {
    const opt = seed();
    const r = buildResultReport(ledger, opt.id);
    expect(JSON.parse(r.json)).toEqual(JSON.parse(JSON.stringify(r.data)));
    expect(r.json).toMatchSnapshot();
  });

  it('handles an optimization with no wins', () => {
    const opt = ledger.createOptimization({
      id: 'empty', repo: 'acme/x', baseRef: 'main', metric: 'm', direction: 'maximize',
      minGain: 0.01, budgetUsd: 1, timeLimitS: 60, configYaml: 'not: valid', createdAt: 1,
    });
    ledger.recordBaseline({ optimizationId: opt.id, samples: [10], noiseFloor: 0, stats: { value: 10 } });
    const { data, markdown } = buildResultReport(ledger, opt.id);
    expect(data.wins).toEqual([]);
    expect(data.costPerVerifiedWinUsd).toBeNull();
    expect(markdown).toContain('_None._');
    expect(markdown).toContain('git checkout main');
  });

  it('throws for an unknown optimization', () => {
    expect(() => buildResultReport(ledger, 'nope')).toThrow(/unknown optimization/);
  });
});

describe('buildSquashPlanFromLedger', () => {
  it('uses verified wins in ratchet order', () => {
    const opt = seed();
    const plan = buildSquashPlanFromLedger(ledger, opt.id);
    expect(plan.shas).toEqual([sha('a'), sha('b')]);
    expect(plan.finalSha).toBe(sha('b'));
    expect(plan.title).toBe('ratchet: http_req_duration.p95 412 → 180 (56.3%, 2 verified wins)');
    expect(plan.message).toContain('Co-authored-by: Codex <noreply@openai.com>');
  });
});

describe('direction-aware change', () => {
  it('renders a throughput gain as +%', () => {
    const opt = ledger.createOptimization({
      id: 'tp', repo: 'a/b', baseRef: 'main', metric: 'rps', direction: 'maximize',
      minGain: 0.01, budgetUsd: 1, timeLimitS: 60, configYaml: CONFIG, createdAt: 1,
    });
    ledger.recordBaseline({ optimizationId: opt.id, samples: [100], noiseFloor: 0, stats: { value: 100 } });
    const a = ledger.addAgent({ optimizationId: opt.id, kind: 'mock', branch: 'b' });
    const c = ledger.submitCandidate({ optimizationId: opt.id, agentId: a.id, sha: sha('f'), message: 'x' });
    ledger.recordBenchResult({ candidateId: c.id, phase: 'bench', samples: [110], stats: { value: 110 }, startedAt: 1, finishedAt: 2 });
    ledger.setVerdict(c.id, 'verified');
    ledger.moveRatchet(opt.id, c.id, 110);
    expect(buildResultReport(ledger, opt.id).markdown).toContain('rps: 100 → 110 (+10.0%)');
  });
});
