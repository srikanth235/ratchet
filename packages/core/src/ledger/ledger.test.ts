import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Ledger, LedgerError, gainPercent, isImprovement, openLedger } from './index.js';

let t = 1_000;
const clock = () => ++t;

let ledger: Ledger;
beforeEach(() => {
  t = 1_000;
  ledger = openLedger(':memory:', { now: clock });
});
afterEach(() => ledger.close());

function mkOpt(over: Partial<Parameters<Ledger['createOptimization']>[0]> = {}) {
  return ledger.createOptimization({
    repo: 'acme/search',
    baseRef: 'main',
    metric: 'http_req_duration.p95',
    direction: 'minimize',
    minGain: 0.03,
    budgetUsd: 100,
    timeLimitS: 3600,
    configYaml: 'version: 0\n',
    ...over,
  });
}

function mkBaseline(optimizationId: string, value = 100) {
  return ledger.recordBaseline({
    optimizationId,
    samples: [value - 1, value, value + 1],
    noiseFloor: 0.02,
    stats: { value, mean: value, stddev: 1 },
  });
}

function mkCandidate(optimizationId: string, agentId: string, sha: string) {
  return ledger.submitCandidate({ optimizationId, agentId, sha, message: `msg ${sha}` });
}

function setup(over: Partial<Parameters<Ledger['createOptimization']>[0]> = {}) {
  const opt = mkOpt(over);
  const base = over.direction === 'maximize' ? 100 : 100;
  mkBaseline(opt.id, base);
  const claude = ledger.addAgent({ optimizationId: opt.id, kind: 'claude-code', branch: `ratchet/${opt.id}/claude` });
  const codex = ledger.addAgent({ optimizationId: opt.id, kind: 'codex', branch: `ratchet/${opt.id}/codex` });
  return { opt, claude, codex };
}

describe('openLedger', () => {
  it('applies migrations and enables foreign keys', () => {
    const tables = ledger.sqlite
      .prepare("select name from sqlite_master where type='table' and name not like '%drizzle%' and name not like 'sqlite_%'")
      .all()
      .map((r) => (r as { name: string }).name)
      .sort();
    expect(tables).toEqual([
      'agent_events',
      'agents',
      'baselines',
      'bench_results',
      'candidates',
      'optimizations',
      'ratchet_moves',
      'spend_events',
    ]);
    expect(ledger.sqlite.pragma('foreign_keys', { simple: true })).toBe(1);
  });

  it('uses WAL for file databases and is idempotent on reopen', () => {
    const dir = mkdtempSync(join(tmpdir(), 'ratchet-ledger-'));
    try {
      const path = join(dir, 'ledger.db');
      const a = openLedger(path);
      expect(a.sqlite.pragma('journal_mode', { simple: true })).toBe('wal');
      const opt = a.createOptimization({
        id: 'opt_x', repo: 'r', baseRef: 'main', metric: 'm', direction: 'minimize',
        minGain: 0.01, budgetUsd: 1, timeLimitS: 1, configYaml: '',
      });
      a.close();
      const b = openLedger(path);
      expect(b.getOptimization(opt.id)?.repo).toBe('r');
      b.close();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('enforces foreign keys', () => {
    expect(() =>
      ledger.sqlite.prepare("insert into agents (id, optimization_id, kind, branch, status, created_at) values ('a','nope','codex','b','pending',1)").run(),
    ).toThrow(/FOREIGN KEY/);
  });
});

describe('optimizations', () => {
  it('creates and fetches an optimization with defaults', () => {
    const o = mkOpt({ name: 'search p95' });
    expect(o.id).toMatch(/^opt_/);
    expect(o).toMatchObject({ status: 'draft', startedAt: null, endedAt: null, name: 'search p95', direction: 'minimize' });
    expect(ledger.getOptimization(o.id)).toEqual(o);
    expect(ledger.getOptimization('missing')).toBeUndefined();
  });

  it('validates input and rejects bad enums at the DB level', () => {
    expect(() => mkOpt({ budgetUsd: -1 })).toThrow(LedgerError);
    expect(() => mkOpt({ minGain: Number.NaN })).toThrow(/finite/);
    expect(() => mkOpt({ direction: 'sideways' as never })).toThrow(/CHECK/);
  });

  it('stamps started/ended timestamps on status changes', () => {
    const o = mkOpt();
    const running = ledger.setOptimizationStatus(o.id, 'running', 5000);
    expect(running).toMatchObject({ status: 'running', startedAt: 5000, endedAt: null });
    expect(ledger.setOptimizationStatus(o.id, 'running', 6000).startedAt).toBe(5000);
    expect(ledger.setOptimizationStatus(o.id, 'completed', 9000)).toMatchObject({ status: 'completed', endedAt: 9000 });
    expect(() => ledger.setOptimizationStatus('missing', 'running')).toThrow(/not found/);
  });
});

describe('baseline', () => {
  it('records, reads and re-records the baseline before the ratchet moves', () => {
    const o = mkOpt();
    expect(ledger.getBaseline(o.id)).toBeUndefined();
    const b = mkBaseline(o.id, 120);
    expect(b).toMatchObject({ value: 120, noiseFloor: 0.02, samples: [119, 120, 121] });
    expect(b.stats.value).toBe(120);
    mkBaseline(o.id, 110);
    expect(ledger.getBaseline(o.id)?.value).toBe(110);
    expect(ledger.currentBest(o.id)).toBe(110);
  });

  it('refuses to change the baseline after a move', () => {
    const { opt, claude } = setup();
    const c = mkCandidate(opt.id, claude.id, 'a1');
    ledger.moveRatchet(opt.id, c.id, 90);
    expect(() => mkBaseline(opt.id, 50)).toThrow(/after the ratchet has moved/);
  });

  it('honours an explicit value override', () => {
    const o = mkOpt();
    const b = ledger.recordBaseline({ optimizationId: o.id, samples: [1], noiseFloor: 0, stats: { value: 1 }, value: 7 });
    expect(b.value).toBe(7);
  });
});

describe('agents', () => {
  it('adds, lists and updates agents', () => {
    const o = mkOpt();
    const a = ledger.addAgent({ optimizationId: o.id, kind: 'claude-code', branch: 'ratchet/x/claude' });
    expect(a).toMatchObject({ status: 'pending', runnerInstanceId: null, kind: 'claude-code' });
    const b = ledger.addAgent({ optimizationId: o.id, kind: 'codex', branch: 'ratchet/x/codex', runnerInstanceId: 'i-1' });
    expect(ledger.listAgents(o.id).map((x) => x.id)).toEqual([a.id, b.id]);
    expect(ledger.setAgentStatus(a.id, 'running', 'i-9')).toMatchObject({ status: 'running', runnerInstanceId: 'i-9' });
    expect(ledger.setAgentStatus(a.id, 'paused').runnerInstanceId).toBe('i-9');
    expect(ledger.getAgent(a.id)?.status).toBe('paused');
    expect(() => ledger.addAgent({ optimizationId: 'nope', kind: 'codex', branch: 'b' })).toThrow(/not found/);
    expect(() => ledger.addAgent({ optimizationId: o.id, kind: 'grok' as never, branch: 'b' })).toThrow(/CHECK/);
    expect(() => ledger.setAgentStatus('nope', 'running')).toThrow(/not found/);
  });
});

describe('candidates, bench results and verdicts', () => {
  it('submits a candidate and rejects foreign agents / duplicate shas', () => {
    const { opt, claude } = setup();
    const other = mkOpt();
    const c = mkCandidate(opt.id, claude.id, 'abc');
    expect(c).toMatchObject({ verdict: 'pending', verdictAt: null, reasons: [], message: 'msg abc' });
    expect(ledger.getCandidate(c.id)).toEqual(c);
    expect(() => ledger.submitCandidate({ optimizationId: other.id, agentId: claude.id, sha: 'z' })).toThrow(/does not belong/);
    expect(() => mkCandidate(opt.id, claude.id, 'abc')).toThrow(/UNIQUE/);
    expect(() => ledger.submitCandidate({ optimizationId: opt.id, agentId: 'nope', sha: 'q' })).toThrow(/not found/);
  });

  it('records bench results and sets verdicts', () => {
    const { opt, claude } = setup();
    const c = mkCandidate(opt.id, claude.id, 'abc');
    const r = ledger.recordBenchResult({
      candidateId: c.id,
      phase: 'bench',
      samples: [90, 91, 89],
      stats: { value: 90, stddev: 1 },
      guardrails: [{ name: 'pytest', passed: true }],
      startedAt: 10,
      finishedAt: 20,
    });
    expect(r).toMatchObject({ phase: 'bench', guardrails: [{ name: 'pytest', passed: true }] });
    const noGuards = ledger.recordBenchResult({
      candidateId: c.id, phase: 'confirm', samples: [92], stats: { value: 92 }, startedAt: 30, finishedAt: 40,
    });
    expect(noGuards.guardrails).toEqual([]);
    expect(() => ledger.recordBenchResult({ candidateId: 'nope', phase: 'bench', samples: [], stats: { value: 1 }, startedAt: 1, finishedAt: 2 })).toThrow(/not found/);
    expect(() => ledger.recordBenchResult({ candidateId: c.id, phase: 'warmup' as never, samples: [], stats: { value: 1 }, startedAt: 1, finishedAt: 2 })).toThrow(/CHECK/);

    const v = ledger.setVerdict(c.id, 'failed_confirm', ['hidden trace regressed'], 77);
    expect(v).toMatchObject({ verdict: 'failed_confirm', reasons: ['hidden trace regressed'], verdictAt: 77 });
    expect(ledger.setVerdict(c.id, 'pending').verdictAt).toBeNull();
    expect(() => ledger.setVerdict('nope', 'verified')).toThrow(/not found/);
    expect(() => ledger.setVerdict(c.id, 'great' as never)).toThrow(/CHECK/);
  });

  it('candidatesWithResults joins agent, results, headline values and ratchet flag', () => {
    const { opt, claude, codex } = setup();
    const c1 = ledger.submitCandidate({ optimizationId: opt.id, agentId: claude.id, sha: 's1', submittedAt: 100 });
    const c2 = ledger.submitCandidate({ optimizationId: opt.id, agentId: codex.id, sha: 's2', submittedAt: 200 });
    ledger.recordBenchResult({ candidateId: c1.id, phase: 'bench', samples: [80], stats: { value: 80 }, startedAt: 1, finishedAt: 2 });
    ledger.recordBenchResult({ candidateId: c1.id, phase: 'confirm', samples: [82], stats: { value: 82 }, startedAt: 3, finishedAt: 4 });
    ledger.setVerdict(c1.id, 'verified');
    ledger.moveRatchet(opt.id, c1.id, 82);
    const list = ledger.candidatesWithResults(opt.id);
    expect(list.map((c) => c.sha)).toEqual(['s1', 's2']);
    expect(list[0]).toMatchObject({ agentKind: 'claude-code', benchValue: 80, confirmValue: 82, movedRatchet: true, verdict: 'verified' });
    expect(list[0]?.results.map((r) => r.phase)).toEqual(['bench', 'confirm']);
    expect(list[1]).toMatchObject({ agentKind: 'codex', benchValue: null, confirmValue: null, movedRatchet: false, results: [] });
    expect(c2.id).toBe(list[1]?.id);
    expect(ledger.candidatesWithResults(mkOpt().id)).toEqual([]);
  });
});

describe('ratchet', () => {
  it('moves strictly forward when minimizing', () => {
    const { opt, claude } = setup();
    const c1 = mkCandidate(opt.id, claude.id, 'a');
    const c2 = mkCandidate(opt.id, claude.id, 'b');
    const c3 = mkCandidate(opt.id, claude.id, 'c');
    const c4 = mkCandidate(opt.id, claude.id, 'd');

    expect(ledger.currentBest(opt.id)).toBe(100);
    const m1 = ledger.moveRatchet(opt.id, c1.id, 90);
    expect(m1).toMatchObject({ fromValue: 100, toValue: 90 });
    expect(() => ledger.moveRatchet(opt.id, c2.id, 90)).toThrow(/not strictly better/); // equal
    expect(() => ledger.moveRatchet(opt.id, c2.id, 95)).toThrow(/not strictly better/); // worse
    try {
      ledger.moveRatchet(opt.id, c2.id, 95);
    } catch (e) {
      expect((e as LedgerError).code).toBe('ratchet_not_improved');
    }
    expect(ledger.moveRatchet(opt.id, c3.id, 85)).toMatchObject({ fromValue: 90, toValue: 85 });
    expect(() => ledger.moveRatchet(opt.id, c4.id, 85.0001)).toThrow(LedgerError);
    expect(ledger.currentBest(opt.id)).toBe(85);
    expect(ledger.ratchetHistory(opt.id).map((m) => [m.fromValue, m.toValue])).toEqual([[100, 90], [90, 85]]);
  });

  it('moves strictly forward when maximizing', () => {
    const { opt, claude } = setup({ direction: 'maximize', metric: 'rps' });
    const c1 = mkCandidate(opt.id, claude.id, 'a');
    const c2 = mkCandidate(opt.id, claude.id, 'b');
    const c3 = mkCandidate(opt.id, claude.id, 'c');
    expect(() => ledger.moveRatchet(opt.id, c1.id, 90)).toThrow(/not strictly better/);
    expect(() => ledger.moveRatchet(opt.id, c1.id, 100)).toThrow(/not strictly better/);
    ledger.moveRatchet(opt.id, c1.id, 120);
    expect(() => ledger.moveRatchet(opt.id, c2.id, 110)).toThrow(/not strictly better/);
    expect(ledger.moveRatchet(opt.id, c3.id, 130)).toMatchObject({ fromValue: 120, toValue: 130 });
    expect(ledger.currentBest(opt.id)).toBe(130);
  });

  it('rejects moves without a baseline, with foreign/unknown candidates, non-finite values and repeats', () => {
    const o = mkOpt();
    const a = ledger.addAgent({ optimizationId: o.id, kind: 'codex', branch: 'b' });
    const c = mkCandidate(o.id, a.id, 'a');
    expect(ledger.currentBest(o.id)).toBeNull();
    expect(() => ledger.moveRatchet(o.id, c.id, 1)).toThrow(/no baseline/);
    mkBaseline(o.id, 100);
    const { opt: o2, claude } = setup();
    const foreign = mkCandidate(o2.id, claude.id, 'f');
    expect(() => ledger.moveRatchet(o.id, foreign.id, 50)).toThrow(/does not belong/);
    expect(() => ledger.moveRatchet(o.id, 'nope', 50)).toThrow(/not found/);
    expect(() => ledger.moveRatchet('nope', c.id, 50)).toThrow(/not found/);
    expect(() => ledger.moveRatchet(o.id, c.id, Number.NaN)).toThrow(/finite/);
    ledger.moveRatchet(o.id, c.id, 50);
    expect(() => ledger.moveRatchet(o.id, c.id, 40)).toThrow(/UNIQUE/); // a candidate moves the ratchet once
    expect(ledger.currentBest(o.id)).toBe(50);
  });

  it('ratchetHistory carries candidate, agent and step gain', () => {
    const { opt, codex } = setup();
    const c = ledger.submitCandidate({ optimizationId: opt.id, agentId: codex.id, sha: 'deadbeef' });
    ledger.moveRatchet(opt.id, c.id, 80, 4242);
    const [h] = ledger.ratchetHistory(opt.id);
    expect(h).toMatchObject({ candidateId: c.id, sha: 'deadbeef', agentId: codex.id, agentKind: 'codex', at: 4242 });
    expect(h?.gainPct).toBeCloseTo(20);
    expect(() => ledger.ratchetHistory('nope')).toThrow(/not found/);
  });

  it('helpers: isImprovement and gainPercent', () => {
    expect(isImprovement('minimize', 10, 9)).toBe(true);
    expect(isImprovement('minimize', 10, 10)).toBe(false);
    expect(isImprovement('maximize', 10, 11)).toBe(true);
    expect(isImprovement('maximize', 10, 9)).toBe(false);
    expect(gainPercent('minimize', 100, 75)).toBe(25);
    expect(gainPercent('maximize', 100, 150)).toBe(50);
    expect(gainPercent('minimize', 0, 0)).toBeNull();
  });
});

describe('append-only enforcement', () => {
  function seed() {
    const { opt, claude } = setup();
    const c = mkCandidate(opt.id, claude.id, 'a');
    ledger.moveRatchet(opt.id, c.id, 90);
    ledger.recordSpend({ optimizationId: opt.id, agentId: claude.id, category: 'tokens', usd: 1.5, source: 'claude-stream' });
    return opt;
  }

  it('blocks UPDATE and DELETE on ratchet_moves', () => {
    seed();
    const raw = ledger.sqlite;
    expect(() => raw.prepare('update ratchet_moves set to_value = 1').run()).toThrow(/ratchet_moves is append-only/);
    expect(() => raw.prepare('delete from ratchet_moves').run()).toThrow(/ratchet_moves is append-only/);
    expect(raw.prepare('select count(*) n from ratchet_moves').get()).toEqual({ n: 1 });
  });

  it('blocks UPDATE and DELETE on spend_events', () => {
    seed();
    const raw = ledger.sqlite;
    expect(() => raw.prepare('update spend_events set usd = 0').run()).toThrow(/spend_events is append-only/);
    expect(() => raw.prepare('delete from spend_events').run()).toThrow(/spend_events is append-only/);
    expect(raw.prepare('select usd from spend_events').get()).toEqual({ usd: 1.5 });
  });

  it('still allows INSERT, and mutable tables remain mutable', () => {
    const opt = seed();
    ledger.recordSpend({ optimizationId: opt.id, category: 'experiment', usd: -0.5, source: 'correction' });
    expect(ledger.totalSpend(opt.id)).toBeCloseTo(1);
    expect(() => ledger.setOptimizationStatus(opt.id, 'running')).not.toThrow();
  });
});

describe('spend', () => {
  it('records spend with defaults and validates', () => {
    const { opt, claude } = setup();
    const s = ledger.recordSpend({ optimizationId: opt.id, agentId: claude.id, category: 'agent_runner', usd: 0.25, source: 'meter' });
    expect(s).toMatchObject({ estimated: true, agentId: claude.id, usd: 0.25 });
    expect(ledger.recordSpend({ optimizationId: opt.id, category: 'storage_egress', usd: 1, estimated: false, source: 'ce' })).toMatchObject({ estimated: false, agentId: null });
    expect(() => ledger.recordSpend({ optimizationId: opt.id, category: 'tokens', usd: Number.POSITIVE_INFINITY, source: 'x' })).toThrow(/finite/);
    expect(() => ledger.recordSpend({ optimizationId: 'nope', category: 'tokens', usd: 1, source: 'x' })).toThrow(/not found/);
    expect(() => ledger.recordSpend({ optimizationId: opt.id, agentId: 'nope', category: 'tokens', usd: 1, source: 'x' })).toThrow(/not found/);
    expect(() => ledger.recordSpend({ optimizationId: opt.id, category: 'coffee' as never, usd: 1, source: 'x' })).toThrow(/CHECK/);
    const other = mkOpt();
    expect(() => ledger.recordSpend({ optimizationId: other.id, agentId: claude.id, category: 'tokens', usd: 1, source: 'x' })).toThrow(/does not belong/);
  });

  it('aggregates totals and breakdowns by category and agent', () => {
    const { opt, claude, codex } = setup();
    const other = mkOpt();
    const rec = (agentId: string | null, category: Parameters<Ledger['recordSpend']>[0]['category'], usd: number, estimated = true) =>
      ledger.recordSpend({ optimizationId: opt.id, agentId, category, usd, estimated, source: 't' });
    rec(claude.id, 'agent_runner', 2);
    rec(claude.id, 'agent_runner', 1);
    rec(claude.id, 'tokens', 4);
    rec(codex.id, 'tokens', 3, false);
    rec(codex.id, 'experiment', 0.5);
    rec(null, 'bench_runner', 5);
    ledger.recordSpend({ optimizationId: other.id, category: 'tokens', usd: 999, source: 't' });

    expect(ledger.totalSpend(opt.id)).toBeCloseTo(15.5);
    expect(ledger.totalSpend(other.id)).toBe(999);
    expect(ledger.totalSpend(mkOpt().id)).toBe(0);

    const b = ledger.spendBreakdown(opt.id);
    expect(b.total).toBeCloseTo(15.5);
    expect(b.estimated).toBeCloseTo(12.5);
    expect(b.actual).toBeCloseTo(3);
    expect(b.byCategory).toEqual({ agent_runner: 3, experiment: 0.5, bench_runner: 5, storage_egress: 0, tokens: 7 });
    expect(b.byAgent.map((a) => [a.agentId, a.kind, a.total])).toEqual([
      [claude.id, 'claude-code', 7],
      [codex.id, 'codex', 3.5],
      [null, null, 5],
    ]);
    expect(b.byAgent[0]?.byCategory).toMatchObject({ agent_runner: 3, tokens: 4, experiment: 0 });

    const empty = ledger.spendBreakdown(mkOpt().id);
    expect(empty).toMatchObject({ total: 0, estimated: 0, actual: 0, byAgent: [] });
    expect(Object.keys(empty.byCategory)).toHaveLength(5);
  });
});

describe('listOptimizations', () => {
  it('returns the exact table shape with best value, gain, spend and agent kinds', () => {
    const { opt, claude, codex } = setup({ name: 'search p95' });
    ledger.setOptimizationStatus(opt.id, 'running', 2000);
    const c1 = mkCandidate(opt.id, claude.id, 'a');
    const c2 = mkCandidate(opt.id, codex.id, 'b');
    mkCandidate(opt.id, codex.id, 'c');
    ledger.moveRatchet(opt.id, c1.id, 90);
    ledger.moveRatchet(opt.id, c2.id, 80);
    ledger.recordSpend({ optimizationId: opt.id, agentId: claude.id, category: 'tokens', usd: 12.5, source: 't' });
    ledger.recordSpend({ optimizationId: opt.id, category: 'bench_runner', usd: 2.5, source: 't' });

    const [row, ...rest] = ledger.listOptimizations();
    expect(rest).toEqual([]);
    expect(row).toEqual({
      id: opt.id,
      name: 'search p95',
      repo: 'acme/search',
      metric: 'http_req_duration.p95',
      direction: 'minimize',
      status: 'running',
      budgetUsd: 100,
      createdAt: opt.createdAt,
      startedAt: 2000,
      endedAt: null,
      baselineValue: 100,
      bestValue: 80,
      gainPct: 20,
      spendUsd: 15,
      agentKinds: ['claude-code', 'codex'],
      agentCount: 2,
      candidateCount: 3,
      verifiedWins: 2,
    });
  });

  it('computes gain for maximize, handles no baseline, empty and ordering/filtering', () => {
    expect(ledger.listOptimizations()).toEqual([]);

    const fresh = mkOpt({ name: 'fresh' });
    expect(ledger.listOptimizations()[0]).toMatchObject({
      id: fresh.id, baselineValue: null, bestValue: null, gainPct: null, spendUsd: 0, agentKinds: [], agentCount: 0, candidateCount: 0, verifiedWins: 0,
    });

    const { opt, claude } = setup({ direction: 'maximize', metric: 'rps', name: 'throughput' });
    const c = mkCandidate(opt.id, claude.id, 'a');
    ledger.moveRatchet(opt.id, c.id, 150);
    ledger.setOptimizationStatus(opt.id, 'completed', 9);

    const all = ledger.listOptimizations();
    expect(all.map((o) => o.name)).toEqual(['throughput', 'fresh']); // newest first
    expect(all[0]).toMatchObject({ bestValue: 150, gainPct: 50, agentKinds: ['claude-code', 'codex'], status: 'completed' });

    // baseline only: best == baseline, gain 0
    const onlyBase = mkOpt({ name: 'base-only' });
    mkBaseline(onlyBase.id, 10);
    expect(ledger.listOptimizations().find((o) => o.id === onlyBase.id)).toMatchObject({ bestValue: 10, gainPct: 0 });

    expect(ledger.listOptimizations({ status: 'completed' }).map((o) => o.name)).toEqual(['throughput']);
    expect(ledger.listOptimizations({ status: ['draft', 'completed'] })).toHaveLength(3);
  });
});

describe('recentActivity', () => {
  it('merges verdicts and moves newest-first with limit and filter', () => {
    const { opt, claude, codex } = setup();
    const other = setup();
    const c1 = ledger.submitCandidate({ optimizationId: opt.id, agentId: claude.id, sha: 'a', message: 'first' });
    const c2 = ledger.submitCandidate({ optimizationId: opt.id, agentId: codex.id, sha: 'b', message: 'second' });
    const c3 = ledger.submitCandidate({ optimizationId: opt.id, agentId: codex.id, sha: 'c' });
    const o1 = ledger.submitCandidate({ optimizationId: other.opt.id, agentId: other.claude.id, sha: 'o' });

    ledger.setVerdict(c1.id, 'verified', [], 100);
    ledger.moveRatchet(opt.id, c1.id, 90, 100);
    ledger.setVerdict(c2.id, 'rejected', ['no gain'], 200);
    ledger.setVerdict(o1.id, 'blocked', ['tests failed'], 250);
    // c3 stays pending: must not appear

    const feed = ledger.recentActivity(10);
    expect(feed.map((i) => [i.kind, i.at])).toEqual([
      ['verdict', 250],
      ['verdict', 200],
      ['ratchet_move', 100],
      ['verdict', 100],
    ]);
    expect(feed[1]).toMatchObject({ verdict: 'rejected', reasons: ['no gain'], agentKind: 'codex', message: 'second', sha: 'b' });
    expect(feed[2]).toMatchObject({ fromValue: 100, toValue: 90, agentKind: 'claude-code', candidateId: c1.id });
    expect(feed.find((i) => i.candidateId === c3.id)).toBeUndefined();

    expect(ledger.recentActivity(2)).toHaveLength(2);
    expect(ledger.recentActivity(0)).toEqual([]);
    const scoped = ledger.recentActivity(10, { optimizationId: opt.id });
    expect(scoped).toHaveLength(3);
    expect(scoped.every((i) => i.optimizationId === opt.id)).toBe(true);
  });
});

describe('agent events', () => {
  it('appends and pages by id', () => {
    const { claude, codex } = setup();
    const e1 = ledger.appendAgentEvent({ agentId: claude.id, type: 'text', payload: { text: 'hi' }, at: 1 });
    const e2 = ledger.appendAgentEvent({ agentId: claude.id, type: 'tool_use', payload: { name: 'bash', args: [1, 2] }, at: 2 });
    ledger.appendAgentEvent({ agentId: codex.id, type: 'text' });
    const e4 = ledger.appendAgentEvent({ agentId: claude.id, type: 'result', payload: null });

    expect(e1.payload).toEqual({ text: 'hi' });
    expect(ledger.appendAgentEvent({ agentId: claude.id, type: 'n' }).payload).toBeNull();
    expect(ledger.agentEvents(claude.id).map((e) => e.type)).toEqual(['text', 'tool_use', 'result', 'n']);
    expect(ledger.agentEvents(claude.id, e1.id).map((e) => e.id)).toEqual([e2.id, e4.id, e4.id + 1]);
    expect(ledger.agentEvents(claude.id, e2.id, 1).map((e) => e.id)).toEqual([e4.id]);
    expect(ledger.agentEvents(claude.id, 10_000)).toEqual([]);
    expect(ledger.agentEvents(codex.id)).toHaveLength(1);
    expect(() => ledger.appendAgentEvent({ agentId: 'nope', type: 'x' })).toThrow(/not found/);
  });
});

describe('transaction', () => {
  it('rolls back on error', () => {
    expect(() =>
      ledger.transaction(() => {
        mkOpt();
        throw new Error('boom');
      }),
    ).toThrow('boom');
    expect(ledger.listOptimizations()).toEqual([]);
  });
});
