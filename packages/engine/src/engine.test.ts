import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentEvent, EngineEvent } from '@ratchet/core';
import { EngineError } from './errors.js';
import { makeHarness, TICK_MS, type Harness } from './testing/harness.js';

const verdicts = (h: Harness, id: string): Record<string, string> =>
  Object.fromEntries(h.ledger.candidatesWithResults(id).map((c) => [c.sha, c.verdict]));

const ev = (type: AgentEvent['type'], extra: Partial<AgentEvent> = {}): AgentEvent => ({ at: 1, type, ...extra });

/** Script a candidate: the commit on the branch plus its trusted bench / confirm samples. */
function candidate(
  h: Harness,
  branch: string,
  sha: string,
  opts: { files?: string[]; bench?: number[]; confirm?: number[]; guardrailFail?: boolean } = {},
): void {
  h.git.push(branch, sha, opts.files ?? ['src/search.ts']);
  if (opts.bench) {
    h.bench.set(sha, 'bench', {
      samples: opts.bench,
      guardrails: opts.guardrailFail ? [{ name: 'pytest -q', passed: false, detail: 'exited with status 1' }] : [{ name: 'pytest -q', passed: true }],
    });
  }
  if (opts.confirm) h.bench.set(sha, 'confirm', { samples: opts.confirm });
}

let tmpDirs: string[] = [];
afterEach(() => {
  for (const d of tmpDirs) rmSync(d, { recursive: true, force: true });
  tmpDirs = [];
});

describe('createOptimization', () => {
  it('reads ratchet.yaml via the git host, snapshots it, and creates agents with branches', async () => {
    const h = makeHarness();
    const id = await h.engine.createOptimization({
      repoUrl: h.git.remoteUrl(),
      baseRef: 'main',
      agents: ['claude-code', 'codex'],
      budgetUsd: 50,
      timeLimitSeconds: 3600,
      plateauEvals: 5,
    });
    expect(id).toBe('opt-test');
    const opt = h.ledger.getOptimization(id)!;
    expect(opt).toMatchObject({ status: 'draft', metric: 'latency_ms', direction: 'minimize', minGain: 0.03, budgetUsd: 50, timeLimitS: 3600 });
    expect(opt.configYaml).toContain('confirm_trace');
    const agents = h.ledger.listAgents(id);
    expect(agents.map((a) => [a.id, a.kind, a.branch, a.status])).toEqual([
      ['opt-test-claude-code', 'claude-code', 'ratchet/opt-test/claude-code', 'pending'],
      ['opt-test-codex', 'codex', 'ratchet/opt-test/codex', 'pending'],
    ]);
    expect(h.log).toEqual([]); // nothing touched the cloud
  });

  it('gives two agents of the same kind distinct branches', async () => {
    const h = makeHarness();
    const id = await h.engine.createOptimization({
      repoUrl: 'x',
      baseRef: 'main',
      agents: ['claude-code', 'claude-code'],
      budgetUsd: 1,
      timeLimitSeconds: 60,
      plateauEvals: 1,
    });
    expect(h.ledger.listAgents(id).map((a) => a.branch)).toEqual(['ratchet/opt-test/claude-code', 'ratchet/opt-test/claude-code-2']);
  });

  it('validates ids, input and the config', async () => {
    const h = makeHarness();
    const base = { repoUrl: 'x', baseRef: 'main', agents: ['codex' as const], budgetUsd: 1, timeLimitSeconds: 60, plateauEvals: 1 };
    await expect(h.engine.createOptimization({ ...base, id: 'Bad_ID' })).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.createOptimization({ ...base, id: 'has/slash' })).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.createOptimization({ ...base, id: '-lead' })).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.createOptimization({ ...base, agents: [] })).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(h.engine.createOptimization({ ...base, budgetUsd: 0 })).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(h.engine.createOptimization({ ...base, plateauEvals: 0 })).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(h.engine.createOptimization({ ...base, configPath: '../etc/passwd' })).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(h.engine.createOptimization({ ...base, agents: ['gemini' as never] })).rejects.toMatchObject({ code: 'invalid_input' });
    h.git.files['ratchet.yaml'] = 'version: 0\nobjective: {}\n';
    await expect(h.engine.createOptimization(base)).rejects.toMatchObject({ code: 'invalid_config' });
    h.git.files['ratchet.yaml'] = makeHarness().git.files['ratchet.yaml']!;
    delete h.git.files['RATCHET.md'];
    await expect(h.engine.createOptimization(base)).rejects.toThrow(/RATCHET\.md/);
    expect(h.ledger.listOptimizations()).toEqual([]);
    // duplicate ids
    h.git.files['RATCHET.md'] = 'brief';
    await h.engine.createOptimization({ ...base, id: 'dup-1' });
    await expect(h.engine.createOptimization({ ...base, id: 'dup-1' })).rejects.toMatchObject({ code: 'invalid_input' });
  });

  it('rejects malformed ids on every public entry point', async () => {
    const h = makeHarness();
    await expect(h.engine.start('NOPE')).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.stop('a b')).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.pauseAgent('../x')).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.teardown('')).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.openPullRequest('UP')).rejects.toMatchObject({ code: 'invalid_id' });
    await expect(h.engine.start('unknown-opt')).rejects.toMatchObject({ code: 'not_found' });
    await expect(h.engine.pauseAgent('unknown-agent')).rejects.toMatchObject({ code: 'not_found' });
  });
});

describe('start', () => {
  it('runs the baseline, records the noise floor, then launches each agent in order', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();

    const baseline = h.ledger.getBaseline(id)!;
    expect(baseline.value).toBe(100); // median
    expect(baseline.samples).toEqual([100, 102, 98]);
    expect(baseline.noiseFloor).toBeCloseTo(4); // 2 * stddev
    expect(baseline.stats['metric:peak_rss']).toBe(500);
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
    expect(h.ledger.getOptimization(id)!.startedAt).not.toBeNull();
    expect(h.bench.jobs[0]).toMatchObject({ phase: 'baseline', repeats: 3, sha: 'main-sha' });
    // the baseline on the hidden trace, so confirm compares like with like
    expect(h.bench.jobs[1]).toMatchObject({ phase: 'confirm', trace: 's3://sealed/trace.jsonl' });

    for (const a of Object.values(agents)) {
      expect(h.ledger.getAgent(a)).toMatchObject({ status: 'running' });
      expect(h.ledger.getAgent(a)!.runnerInstanceId).toMatch(/^i-/);
    }
    // per-agent order
    const claude = agents['claude-code']!;
    const order = h.log.filter((l) => l.endsWith(claude) || l.includes(`:${claude}:`) || l.includes('claude-code'));
    expect(order.map((l) => l.split(':')[0])).toEqual([
      'compute.createAgentIdentity',
      'compute.putSecret',
      'compute.putSecret',
      'git.createBranch',
      'compute.launchRunner',
      'runners.start',
    ]);

    const started = h.runners.started.find((s) => s.handle.agentId === claude)!;
    expect(started.config).toMatchObject({
      optimizationId: id,
      branch: 'ratchet/opt-test/claude-code',
      baseRef: 'main',
      baselineValue: 100,
      budgetUsd: 100,
    });
    expect(started.config.brief).toContain('Make it faster');
    expect(started.config.deadlineAt).toBe(h.clock.now() + 8 * 3600 * 1000);
    expect(started.handle.instanceId).toBe(h.ledger.getAgent(claude)!.runnerInstanceId);
  });

  it('never stores secret values in the ledger', async () => {
    const h = makeHarness();
    await h.begin();
    expect(h.compute.secrets.size).toBe(4);
    const dump = h.ledger.sqlite.serialize().toString('latin1');
    expect(dump).not.toContain('value-of-');
    expect(dump).toContain('/ratchet/opt-test/opt-test-codex/OPENAI_API_KEY'); // refs are fine
  });

  it('fails the optimization and cleans up when a secret is unavailable', async () => {
    const h = makeHarness({ settings: { secrets: (_k, name) => (name === 'GITHUB_TOKEN' ? undefined : 'v') } });
    await expect(h.begin()).rejects.toMatchObject({ code: 'missing_secret' });
    expect(h.ledger.getOptimization('opt-test')!.status).toBe('failed');
    expect(h.compute.live()).toEqual([]);
    expect(h.compute.identities.get('opt-test-claude-code')!.revoked).toBe(true);
  });

  it('fails when the baseline breaks its own command guardrail', async () => {
    const h = makeHarness();
    h.bench.set('main-sha', 'baseline', { samples: [10, 10, 10], guardrails: [{ name: 'pytest -q', passed: false }] });
    const id = await h.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 });
    await expect(h.engine.start(id)).rejects.toThrow(/baseline fails its own guardrails/);
    expect(h.ledger.getOptimization(id)!.status).toBe('failed');
    expect(h.ledger.getBaseline(id)).toBeUndefined();
  });

  it('can only be started once', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    await expect(h.engine.start(id)).rejects.toMatchObject({ code: 'invalid_state' });
  });

  it('a kill switch during the baseline wins and start() returns quietly', async () => {
    const h = makeHarness();
    h.bench.gate = new Promise(() => undefined); // baseline never finishes on its own
    h.bench.set('main-sha', 'baseline', { samples: [1, 1, 1] });
    const id = await h.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 });
    const starting = h.engine.start(id);
    await Promise.resolve();
    await h.engine.stop(id, 'changed my mind');
    await starting;
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
    expect(h.compute.live()).toEqual([]);
    expect(h.log.some((l) => l.startsWith('compute.launchRunner'))).toBe(false);
  });
});

describe('happy path', () => {
  it('baseline -> two agents -> verified, rejected, blocked and failed-confirm candidates -> monotonic ratchet -> PR', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    const C = 'ratchet/opt-test/claude-code';
    const X = 'ratchet/opt-test/codex';
    const events: EngineEvent[] = [];
    h.engine.subscribe((e) => events.push(e));

    candidate(h, C, 'sha-c1', { bench: [90, 91, 89], confirm: [88, 89, 90] }); // verified 100 -> 90
    candidate(h, C, 'sha-c2', { bench: [80, 81, 79], confirm: [95, 96, 97] }); // bench win, fails the hidden trace
    candidate(h, C, 'sha-c3', { bench: [60, 60, 60], guardrailFail: true }); // guardrail failed
    candidate(h, C, 'sha-c4', { bench: [69.5, 70, 70.5] }); // not better than 70
    candidate(h, X, 'sha-x1', { files: ['src/a.ts', 'bench/k6/search.js'] }); // protected path, never benched
    candidate(h, X, 'sha-x2', { bench: [89.5, 90, 90.5] }); // equal to the best: rejected
    candidate(h, X, 'sha-x3', { bench: [70, 70, 71], confirm: [72, 72, 73] }); // verified 90 -> 70

    await h.until(() => h.ledger.candidatesWithResults(id).length === 7 && h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending'));

    expect(verdicts(h, id)).toEqual({
      'sha-c1': 'verified',
      'sha-c2': 'failed_confirm',
      'sha-c3': 'blocked',
      'sha-c4': 'rejected',
      'sha-x1': 'blocked',
      'sha-x2': 'rejected',
      'sha-x3': 'verified',
    });
    const byId = (sha: string) => h.ledger.candidatesWithResults(id).find((c) => c.sha === sha)!;
    expect(byId('sha-x1').reasons[0]).toMatch(/protected path changed: bench\/k6\/search\.js \(matches bench\/\*\*\)/);
    expect(byId('sha-x1').results).toEqual([]); // blocked without spending a bench run
    expect(byId('sha-c3').reasons[0]).toMatch(/guardrail failed: pytest -q/);
    expect(byId('sha-c2').reasons.join(' ')).toMatch(/does not beat current best/);

    // ratchet only moved forward
    const hist = h.ledger.ratchetHistory(id);
    expect(hist.map((m) => [m.sha, m.fromValue, m.toValue])).toEqual([
      ['sha-c1', 100, 90],
      ['sha-x3', 90, 70],
    ]);
    expect(h.ledger.currentBest(id)).toBe(70);
    expect(byId('sha-c1').results.map((r) => r.phase)).toEqual(['bench', 'confirm']);
    expect(byId('sha-c2').results.map((r) => r.phase)).toEqual(['bench', 'confirm']);
    expect(byId('sha-x2').results.map((r) => r.phase)).toEqual(['bench']);

    // one bench job at a time, round-robin between agents, confirm right after its bench
    expect(h.bench.jobs.slice(2).map((j) => `${j.sha}:${j.phase}`)).toEqual([
      'sha-c1:bench',
      'sha-c1:confirm',
      'sha-x2:bench',
      'sha-c2:bench',
      'sha-c2:confirm',
      'sha-x3:bench',
      'sha-x3:confirm',
      'sha-c3:bench',
      'sha-c4:bench',
    ]);
    expect(h.bench.jobs.find((j) => j.sha === 'sha-c1' && j.phase === 'confirm')!.trace).toBe('s3://sealed/trace.jsonl');
    expect(h.bench.baselineArgs.at(-1)).toEqual({ metrics: { objective: 100, peak_rss: 500 } });

    // verdicts reach the submitting agent; ratchet moves reach everyone else
    const toClaude = h.runners.sentTo(agents['claude-code']!);
    const toCodex = h.runners.sentTo(agents.codex!);
    expect(toClaude.filter((m) => m.type === 'verdict' && m.sha === 'sha-c1')).toEqual([
      { type: 'verdict', sha: 'sha-c1', verdict: 'verified', reasons: expect.any(Array), currentBest: 90 },
    ]);
    expect(toClaude).toContainEqual({ type: 'ratchet', sha: 'sha-x3', branch: 'ratchet/opt-test/codex', value: 70 });
    expect(toCodex).toContainEqual({ type: 'ratchet', sha: 'sha-c1', branch: 'ratchet/opt-test/claude-code', value: 90 });
    // the winner gets its verdict, not a ratchet announcement about itself
    expect(toCodex.some((m) => m.type === 'ratchet' && m.sha === 'sha-x3')).toBe(false);
    expect(toClaude.some((m) => m.type === 'ratchet' && m.sha === 'sha-c1')).toBe(false);
    expect(toCodex.some((m) => m.type === 'verdict' && m.sha === 'sha-c1')).toBe(false);
    expect(toCodex.some((m) => m.type === 'verdict' && m.sha === 'sha-x1' && m.verdict === 'blocked')).toBe(true);
    // a rejection is private to its author
    expect(toClaude.some((m) => m.type === 'verdict' && m.sha === 'sha-x2')).toBe(false);

    // events
    expect(events.filter((e) => e.type === 'ratchet').map((e) => (e.type === 'ratchet' ? [e.from, e.to] : null))).toEqual([
      [100, 90],
      [90, 70],
    ]);
    expect(events.filter((e) => e.type === 'candidate' && e.verdict === 'pending')).toHaveLength(7);
    expect(events.filter((e) => e.type === 'candidate' && e.verdict === 'verified')).toHaveLength(2);
    expect(events.some((e) => e.type === 'spend')).toBe(true);
    // verdicts are in the per-agent log too
    expect(h.ledger.agentEvents(agents['claude-code']!).filter((e) => e.type === 'verdict')).toHaveLength(4);

    // end the run and open the PR
    await expect(h.engine.openPullRequest(id)).rejects.toMatchObject({ code: 'invalid_state' });
    await h.engine.stop(id, 'done');
    const pr = await h.engine.openPullRequest(id);
    expect(pr.url).toBe('https://example.test/pr/1');
    expect(h.git.prs[0]).toMatchObject({ base: 'main', head: 'ratchet/opt-test/result', shas: ['sha-c1', 'sha-x3'] });
    expect(h.git.prs[0]!.title).toMatch(/latency_ms 100 -> 70 \(30% lower, 2 verified\)/);
    expect(h.git.prs[0]!.body).toMatch(/Verified wins/);
    expect(h.git.prs[0]!.body).toMatch(/sha-c1/);
    expect((await h.engine.openPullRequest(id)).url).toBe(pr.url); // idempotent
    expect(h.git.prs).toHaveLength(1);

    await h.engine.teardown(id);
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
    expect(h.compute.live()).toEqual([]);
  });

  it('uses a custom report builder for the PR body', async () => {
    const h = makeHarness({ settings: { reportBuilder: (_l, optId) => `custom report for ${optId}` } });
    const { id } = await h.begin();
    candidate(h, 'ratchet/opt-test/codex', 'sha-1', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    await h.until(() => verdicts(h, id)['sha-1'] === 'verified');
    await h.engine.stop(id);
    await h.engine.openPullRequest(id);
    expect(h.git.prs[0]!.body).toBe('custom report for opt-test');
    expect(await h.artifacts.get('engine/opt-test/report.md')).toBe('custom report for opt-test');
  });

  it('refuses to open a PR without a verified candidate', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    await h.engine.stop(id);
    await expect(h.engine.openPullRequest(id)).rejects.toMatchObject({ code: 'no_verified' });
  });

  it('keeps the hidden-trace baseline separate from the bench scale', async () => {
    // Bench scale ~100 ms but the hidden trace runs at ~200 ms: a confirm of 190 beats 200, not 100.
    const h = makeHarness();
    const { id } = await h.begin({ confirmBaseline: [200, 200, 200] });
    candidate(h, 'ratchet/opt-test/codex', 'sha-1', { bench: [80, 80, 80], confirm: [190, 190, 190] });
    await h.until(() => verdicts(h, id)['sha-1'] === 'verified');
  });

  it('retries bench infrastructure failures, then rejects after the attempt limit', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    candidate(h, 'ratchet/opt-test/codex', 'sha-flaky');
    h.bench.set('sha-flaky', 'bench', { samples: [80, 80, 80], failTimes: 1 });
    h.bench.set('sha-flaky', 'confirm', { samples: [80, 80, 80] });
    candidate(h, 'ratchet/opt-test/codex', 'sha-dead');
    h.bench.set('sha-dead', 'bench', { samples: [1, 1, 1], failTimes: 99 });
    await h.until(() => Object.values(verdicts(h, id)).every((v) => v !== 'pending') && Object.keys(verdicts(h, id)).length === 2);
    expect(verdicts(h, id)).toEqual({ 'sha-flaky': 'verified', 'sha-dead': 'rejected' });
    const dead = h.ledger.candidatesWithResults(id).find((c) => c.sha === 'sha-dead')!;
    expect(dead.reasons[0]).toMatch(/bench infrastructure error after 2 attempts/);
  });

  it('a guardrail that fails before any bench run (no samples at all) blocks the candidate instead of wedging the queue', async () => {
    const h = makeHarness();
    const { id } = await h.begin({ agents: ['codex'] });
    h.git.push('ratchet/opt-test/codex', 'sha-brk', ['src/search.ts']);
    h.bench.set('sha-brk', 'bench', { samples: [], guardrails: [{ name: 'pnpm test:unit', passed: false, detail: 'exited with status 1' }] });
    candidate(h, 'ratchet/opt-test/codex', 'sha-next', { bench: [70, 70, 70], confirm: [70, 70, 70] });
    await h.until(() => h.ledger.candidatesWithResults(id).length === 2 && h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending'), 20);
    expect(verdicts(h, id)).toEqual({ 'sha-brk': 'blocked', 'sha-next': 'verified' });
    const brk = h.ledger.candidatesWithResults(id).find((c) => c.sha === 'sha-brk')!;
    expect(brk.reasons[0]).toMatch(/guardrail failed: pnpm test:unit \(exited with status 1\)/);
    expect(h.bench.jobs.filter((j) => j.sha === 'sha-brk')).toHaveLength(1); // not retried
  });

  it('an outcome with neither samples nor a failed guardrail counts as a failed attempt and is rejected after the attempt limit', async () => {
    const h = makeHarness();
    const { id } = await h.begin({ agents: ['codex'] });
    h.git.push('ratchet/opt-test/codex', 'sha-empty', ['src/search.ts']);
    h.bench.set('sha-empty', 'bench', { samples: [] });
    await h.until(() => h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending') && h.ledger.candidatesWithResults(id).length === 1, 20);
    expect(verdicts(h, id)).toEqual({ 'sha-empty': 'rejected' });
    expect(h.bench.jobs.filter((j) => j.sha === 'sha-empty')).toHaveLength(2);
  });

  it('retries a candidate whose changed files cannot be read yet, without losing order', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    candidate(h, 'ratchet/opt-test/codex', 'sha-a', { bench: [90, 90, 90], confirm: [90, 90, 90] });
    candidate(h, 'ratchet/opt-test/codex', 'sha-b', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    h.git.failChangedFiles.add('sha-a');
    await h.cycle(2);
    expect(h.ledger.candidatesWithResults(id)).toEqual([]);
    h.git.failChangedFiles.clear();
    await h.until(() => Object.values(verdicts(h, id)).length === 2 && Object.values(verdicts(h, id)).every((v) => v !== 'pending'));
    expect(h.ledger.ratchetHistory(id).map((m) => m.sha)).toEqual(['sha-a', 'sha-b']);
  });

  it('ignores duplicate candidates the git host re-lists', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    candidate(h, 'ratchet/opt-test/codex', 'sha-a', { bench: [90, 90, 90], confirm: [90, 90, 90] });
    await h.cycle(6);
    h.git.listCandidates = async (branch) => (h.git.commits.get(branch) ?? []).map(({ files: _f, ...c }) => c); // ignores `sinceSha`
    await h.cycle(3);
    expect(h.ledger.candidatesWithResults(id)).toHaveLength(1);
  });
});

describe('cost and stop rules', () => {
  it('meters instances x price by category with estimated=true, from a per-instance watermark', async () => {
    const h = makeHarness({ hourly: 3.6 }); // $0.001 per second
    const { id, agents } = await h.begin();
    const exp = h.compute.launchExperiment(id, agents.codex!);
    await h.cycle(1); // +60 s
    const b = h.ledger.spendBreakdown(id);
    expect(b.byCategory.agent_runner).toBeCloseTo(2 * 60 * 0.001, 6);
    expect(b.byCategory.experiment).toBeCloseTo(60 * 0.001, 6);
    expect(b.actual).toBe(0);
    const codexExp = b.byAgent.find((a) => a.agentId === agents.codex)!;
    expect(codexExp.byCategory.experiment).toBeCloseTo(0.06, 6);

    // no double counting when nothing time-wise happened
    await h.engine.tick();
    expect(h.ledger.totalSpend(id)).toBeCloseTo(b.total, 9);
    await h.cycle(1);
    expect(h.ledger.spendBreakdown(id).byCategory.experiment).toBeCloseTo(0.12, 6);
    expect(exp.id).toBeTruthy();
  });

  it('counts bench instances as bench_runner spend', async () => {
    const h = makeHarness({ hourly: 3.6 });
    const { id } = await h.begin();
    await h.compute.launchBenchInstance({ optimizationId: id, instanceType: 'c7i.2xlarge', imageRef: 'x', purpose: 'bench' });
    await h.cycle(1);
    expect(h.ledger.spendBreakdown(id).byCategory.bench_runner).toBeCloseTo(0.06, 6);
  });

  it('records token usage from runner events as spend category tokens', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.runners.emit(
      agents['claude-code']!,
      ev('usage', { usage: { inputTokens: 1_000_000, outputTokens: 100_000 } }), // 3 + 1.5
      ev('usage', { usage: { inputTokens: 0, outputTokens: 0, costUsd: 0.25 } }),
    );
    await h.cycle(1);
    const b = h.ledger.spendBreakdown(id);
    expect(b.byCategory.tokens).toBeCloseTo(4.75, 9);
    expect(b.byAgent.find((a) => a.agentId === agents['claude-code'])!.byCategory.tokens).toBeCloseTo(4.75, 9);
    await h.cycle(1);
    expect(h.ledger.spendBreakdown(id).byCategory.tokens).toBeCloseTo(4.75, 9); // not re-counted
  });

  it('warns at 80% and trips the kill switch at 100% in the right order, within one metering interval', async () => {
    const h = makeHarness({ hourly: 10 }); // two runners: $0.333 per tick
    const { id, agents } = await h.begin({ budgetUsd: 1.2 });
    const events: EngineEvent[] = [];
    h.engine.subscribe((e) => events.push(e));
    const mark = h.log.length;

    await h.cycle(3); // $1.0 = 83%
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
    expect(events.filter((e) => e.type === 'notice' && e.level === 'warning')).toHaveLength(1);
    await h.cycle(1); // $1.333 >= $1.2
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
    expect(h.engine.stopReason(id)).toMatch(/budget exhausted/);
    expect(events.filter((e) => e.type === 'notice' && e.level === 'warning')).toHaveLength(1);

    const after = h.log.slice(mark);
    const idx = (prefix: string) => after.findIndex((l) => l.startsWith(prefix));
    const lastIdx = (prefix: string) => after.map((l) => l.startsWith(prefix)).lastIndexOf(true);
    expect(after.filter((l) => l.startsWith('compute.revoke'))).toEqual([`compute.revoke:${agents['claude-code']}`, `compute.revoke:${agents.codex}`]);
    expect(lastIdx('compute.revoke')).toBeLessThan(idx('runners.stop'));
    expect(lastIdx('runners.stop')).toBeLessThan(idx('compute.terminate'));
    expect(h.compute.live()).toEqual([]);
    expect(h.ledger.listAgents(id).map((a) => a.status)).toEqual(['stopped', 'stopped']);
    expect(h.compute.identities.get(agents.codex!)!.revoked).toBe(true);
    const spend = h.ledger.totalSpend(id);
    expect(spend).toBeGreaterThanOrEqual(1.2);
    expect(spend - 1.2).toBeLessThanOrEqual((2 * 10 * TICK_MS) / 3_600_000 + 1e-9); // at most one interval over
    // no more cost accrues afterwards
    await h.cycle(3);
    expect(h.ledger.totalSpend(id)).toBeCloseTo(spend, 9);
  });

  it('a token-spend overrun also trips the kill switch', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin({ budgetUsd: 2 });
    h.runners.emit(agents.codex!, ev('usage', { usage: { inputTokens: 0, outputTokens: 0, costUsd: 5 } }));
    await h.cycle(1);
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
    expect(h.compute.live()).toEqual([]);
  });

  it('stops gracefully at the deadline: runners stop, queued candidates are still evaluated, then everything is torn down', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin({ timeLimitSeconds: 10 * 60 });
    await h.cycle(8);
    candidate(h, 'ratchet/opt-test/codex', 'sha-late', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    await h.cycle(1); // submitted + bench started + collected into the job
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
    await h.cycle(1); // deadline reached: bench result processed, graceful stop begins
    expect(h.ledger.getOptimization(id)!.status).toBe('stopping');
    expect(h.log.filter((l) => l.startsWith('runners.stop'))).toHaveLength(2);
    expect(h.compute.identities.get(agents.codex!)!.revoked).toBe(true);
    expect(h.ledger.listAgents(id).map((a) => a.status)).toEqual(['stopped', 'stopped']);
    expect(h.ledger.candidatesWithResults(id)[0]!.verdict).toBe('pending'); // confirm still due

    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(verdicts(h, id)).toEqual({ 'sha-late': 'verified' });
    expect(h.engine.stopReason(id)).toBe('time limit reached');
    expect(h.compute.live()).toEqual([]);
    expect(h.ledger.getOptimization(id)!.endedAt).not.toBeNull();
  });

  it('picks up a candidate pushed right before a graceful stop (final poll)', async () => {
    const h = makeHarness();
    const { id } = await h.begin({ timeLimitSeconds: 5 * 60 });
    await h.cycle(4);
    candidate(h, 'ratchet/opt-test/claude-code', 'sha-last', { bench: [85, 85, 85], confirm: [85, 85, 85] });
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(verdicts(h, id)).toEqual({ 'sha-last': 'verified' });
  });

  describe('graceful stop drains every candidate', () => {
    const CC = 'ratchet/opt-test/claude-code';
    const CX = 'ratchet/opt-test/codex';

    it('all agents done in the same tick as their last push: the final git poll submits it and the drain verifies it', async () => {
      const h = makeHarness();
      const { id, agents } = await h.begin();
      candidate(h, CX, 'sha-last', { bench: [85, 85, 85], confirm: [85, 85, 85] });
      h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
      h.runners.emit(agents.codex!, ev('status', { data: { state: 'done' } }));
      await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
      expect(h.engine.stopReason(id)).toBe('all agents finished');
      expect(verdicts(h, id)).toEqual({ 'sha-last': 'verified' });
      expect(h.ledger.candidatesWithResults(id)[0]!.results.map((r) => r.phase)).toEqual(['bench', 'confirm']);
      expect(h.ledger.currentBest(id)).toBe(85);
    });

    it('a runner that pushes its last commit while it is being stopped is still collected and evaluated', async () => {
      const h = makeHarness({ settings: {} });
      const { id, agents } = await h.begin({ timeLimitSeconds: 5 * 60 });
      const origStop = h.runners.stop.bind(h.runners);
      h.runners.stop = async (handle) => {
        await origStop(handle);
        if (handle.agentId === agents.codex) {
          candidate(h, CX, 'sha-in-flight', { bench: [70, 70, 70], confirm: [70, 70, 70] });
          h.runners.emit(handle.agentId, ev('status', { data: { state: 'done' } }));
        }
      };
      await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
      expect(verdicts(h, id)).toEqual({ 'sha-in-flight': 'verified' });
    });

    it('reports done only AFTER the push becomes visible: the drain keeps polling git until runners are done and the branch is quiet', async () => {
      const h = makeHarness();
      const { id, agents } = await h.begin({ timeLimitSeconds: 5 * 60 });
      // The branch listing lags: the first two polls after the stop see nothing, then the commit appears.
      candidate(h, CC, 'sha-lagging', { bench: [60, 60, 60], confirm: [60, 60, 60] });
      const real = h.git.listCandidates.bind(h.git);
      let hidden = 0;
      h.git.listCandidates = async (branch, since) => {
        if (h.ledger.getOptimization(id)!.status === 'stopping' && hidden < 2) {
          hidden++;
          return [];
        }
        return real(branch, since);
      };
      // Make the commit invisible to the normal running polls too, so only the drain can find it.
      const running = h.git.commits.get(CC)!.splice(0);
      h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
      h.runners.emit(agents.codex!, ev('status', { data: { state: 'done' } }));
      await h.cycle(1);
      h.git.commits.set(CC, running);
      await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
      expect(verdicts(h, id)).toEqual({ 'sha-lagging': 'verified' });
    });

    it('survives a transient git failure during the final poll: it retries while draining', async () => {
      const h = makeHarness();
      const { id, agents } = await h.begin();
      candidate(h, CX, 'sha-flaky', { bench: [88, 88, 88], confirm: [88, 88, 88] });
      const real = h.git.listCandidates.bind(h.git);
      let failures = 0;
      h.git.listCandidates = async (branch, since) => {
        if (failures < 3) {
          failures++;
          throw new Error('git fetch lock contention');
        }
        return real(branch, since);
      };
      h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
      h.runners.emit(agents.codex!, ev('status', { data: { state: 'done' } }));
      await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
      expect(verdicts(h, id)).toEqual({ 'sha-flaky': 'verified' });
    });

    it('an agent that finishes while the others keep running still gets its last push submitted in that tick', async () => {
      const h = makeHarness();
      const { id, agents } = await h.begin();
      candidate(h, CC, 'sha-final', { bench: [75, 75, 75], confirm: [75, 75, 75] });
      h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
      await h.cycle(1);
      expect(h.ledger.getAgent(agents['claude-code']!)!.status).toBe('done');
      expect(h.ledger.getOptimization(id)!.status).toBe('running');
      expect(Object.keys(verdicts(h, id))).toEqual(['sha-final']);
      await h.cycle(4);
      expect(verdicts(h, id)).toEqual({ 'sha-final': 'verified' });
    });

    it('evaluates a backlog queued from several agents (bench + confirm) before terminating', async () => {
      const h = makeHarness();
      const { id, agents } = await h.begin({ timeLimitSeconds: 5 * 60 });
      candidate(h, CC, 'sha-b1', { bench: [90, 90, 90], confirm: [90, 90, 90] });
      candidate(h, CX, 'sha-b2', { bench: [80, 80, 80], confirm: [80, 80, 80] });
      candidate(h, CC, 'sha-b3', { bench: [70, 70, 70], confirm: [70, 70, 70] });
      h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
      h.runners.emit(agents.codex!, ev('status', { data: { state: 'done' } }));
      await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
      expect(Object.values(verdicts(h, id)).filter((v) => v === 'pending')).toEqual([]);
      expect(Object.keys(verdicts(h, id)).sort()).toEqual(['sha-b1', 'sha-b2', 'sha-b3']);
      expect(h.ledger.currentBest(id)).toBe(70);
      expect(h.compute.live()).toEqual([]);
    });

    it('the drain is bounded by drainTimeoutMs', async () => {
      const h = makeHarness({ settings: { drainTimeoutMs: 3 * TICK_MS } });
      const { id, agents } = await h.begin({ timeLimitSeconds: 5 * 60 });
      let release!: () => void;
      h.bench.gate = new Promise<void>((r) => (release = r));
      candidate(h, CX, 'sha-slow', { bench: [50, 50, 50], confirm: [50, 50, 50] });
      h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
      h.runners.emit(agents.codex!, ev('status', { data: { state: 'done' } }));
      for (let i = 0; i < 12 && h.ledger.getOptimization(id)!.status !== 'stopped'; i++) {
        h.clock.advance(TICK_MS);
        await h.engine.tick();
      }
      release();
      expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
      expect(h.engine.stopReason(id)).toMatch(/drain timeout/);
    });
  });

  describe('identical shas from different agents', () => {
    it('keys candidates per (agent, sha): both get rows and verdicts; the second is rejected after the first moves the ratchet', async () => {
      const h = makeHarness();
      const { id, agents } = await h.begin();
      const CC = 'ratchet/opt-test/claude-code';
      const CX = 'ratchet/opt-test/codex';
      h.git.push(CC, 'sha-same', ['src/search.ts']);
      h.git.push(CX, 'sha-same', ['src/search.ts']);
      h.bench.set('sha-same', 'bench', { samples: [80, 80, 80], guardrails: [{ name: 'pytest -q', passed: true }] });
      h.bench.set('sha-same', 'confirm', { samples: [80, 80, 80] });
      await h.until(() => h.ledger.candidatesWithResults(id).length === 2 && h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending'));

      const rows = h.ledger.candidatesWithResults(id);
      expect(new Set(rows.map((c) => c.id)).size).toBe(2);
      expect(rows.map((c) => [c.agentId, c.sha]).sort()).toEqual(
        [[agents['claude-code'], 'sha-same'], [agents.codex, 'sha-same']].sort(),
      );
      const first = rows.find((c) => c.agentId === agents['claude-code'])!;
      const second = rows.find((c) => c.agentId === agents.codex)!;
      expect(first.verdict).toBe('verified');
      expect(second.verdict).toBe('rejected');
      expect(second.reasons.join(' ')).toMatch(/not better than current best 80/);
      expect(second.results.map((r) => r.phase)).toEqual(['bench']);
      expect(h.ledger.ratchetHistory(id).map((m) => m.sha)).toEqual(['sha-same']);
      expect(h.ledger.currentBest(id)).toBe(80);
      // each agent hears about its own candidate only
      expect(h.runners.sentTo(agents['claude-code']!).filter((m) => m.type === 'verdict')).toEqual([
        expect.objectContaining({ sha: 'sha-same', verdict: 'verified' }),
      ]);
      expect(h.runners.sentTo(agents.codex!).filter((m) => m.type === 'verdict')).toEqual([
        expect.objectContaining({ sha: 'sha-same', verdict: 'rejected' }),
      ]);
      // re-listing does not duplicate either row
      await h.cycle(3);
      expect(h.ledger.candidatesWithResults(id)).toHaveLength(2);
    });

    it('a later commit on one branch is found even when another agent already submitted the same sha', async () => {
      const h = makeHarness();
      const { id } = await h.begin();
      const CC = 'ratchet/opt-test/claude-code';
      const CX = 'ratchet/opt-test/codex';
      h.git.push(CC, 'sha-same', ['src/search.ts']);
      h.bench.set('sha-same', 'bench', { samples: [90, 90, 90] });
      h.bench.set('sha-same', 'confirm', { samples: [90, 90, 90] });
      await h.cycle(6);
      h.git.push(CX, 'sha-same', ['src/search.ts']);
      candidate(h, CX, 'sha-next', { bench: [70, 70, 70], confirm: [70, 70, 70] });
      await h.until(() => h.ledger.candidatesWithResults(id).length === 3 && h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending'));
      expect(verdicts(h, id)['sha-next']).toBe('verified');
    });
  });

  it('stops after plateauEvals consecutive evaluations without a verified win; a win resets the count', async () => {
    const h = makeHarness();
    const { id } = await h.begin({ agents: ['claude-code'], plateauEvals: 3 });
    const C = 'ratchet/opt-test/claude-code';
    candidate(h, C, 'sha-r1', { bench: [100, 100, 100] });
    candidate(h, C, 'sha-r2', { bench: [101, 101, 101] });
    candidate(h, C, 'sha-v1', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    candidate(h, C, 'sha-r3', { bench: [80, 80, 80] });
    candidate(h, C, 'sha-r4', { bench: [81, 81, 81] });
    await h.until(() => h.ledger.candidatesWithResults(id).length === 5 && h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending'));
    expect(h.ledger.getOptimization(id)!.status).toBe('running'); // streak is 2, not 3
    candidate(h, C, 'sha-r5', { bench: [82, 82, 82] });
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(h.engine.stopReason(id)).toMatch(/plateau: 3 consecutive/);
    expect(verdicts(h, id)['sha-r5']).toBe('rejected'); // the third eval was decided before stopping
    expect(h.compute.live()).toEqual([]);
  });

  it('counts blocked candidates toward the plateau', async () => {
    const h = makeHarness();
    const { id } = await h.begin({ agents: ['codex'], plateauEvals: 2 });
    candidate(h, 'ratchet/opt-test/codex', 'sha-b1', { files: ['ratchet.yaml'] });
    candidate(h, 'ratchet/opt-test/codex', 'sha-b2', { files: ['bench/x'] });
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(h.engine.stopReason(id)).toMatch(/plateau/);
  });

  it('stops when every agent reports done', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
    h.runners.emit(agents.codex!, ev('status', { data: { state: 'failed' } }));
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(h.engine.stopReason(id)).toBe('all agents finished');
    expect(h.ledger.getAgent(agents['claude-code']!)!.status).toBe('done');
    expect(h.ledger.getAgent(agents.codex!)!.status).toBe('failed');
  });
});

describe('runner finish events', () => {
  it("treats the runner loop's closing status (reason, no state) as done / failed, and stops once every agent is finished", async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    const fin = (reason: string): AgentEvent => ev('status', { text: `runner finished: ${reason}`, data: { reason, turns: 3, candidates: 1, spendUsd: 0.1 } });
    h.runners.emit(agents['claude-code']!, fin('idle'));
    h.runners.emit(agents.codex!, fin('agent-unavailable'));
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(h.engine.stopReason(id)).toBe('all agents finished');
    expect(h.ledger.getAgent(agents['claude-code']!)!.status).toBe('done');
    expect(h.ledger.getAgent(agents.codex!)!.status).toBe('failed');
  });

  it('a runner that finished because the engine stopped it is not treated as done early', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.runners.emit(agents['claude-code']!, ev('status', { text: 'runner finished: stopped', data: { reason: 'stopped', turns: 1, candidates: 0 } }));
    await h.cycle(2);
    expect(h.ledger.getAgent(agents['claude-code']!)!.status).toBe('running');
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
  });
});

describe('kill switch', () => {
  it('stop() revokes first, then stops runners, then terminates; and aborts the in-flight bench', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.compute.launchExperiment(id, agents.codex!);
    candidate(h, 'ratchet/opt-test/codex', 'sha-1', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    h.bench.gate = new Promise(() => undefined);
    h.clock.advance(TICK_MS);
    await h.engine.tick();
    expect(h.engine.busy).toBe(true);
    const mark = h.log.length;
    await h.engine.stop(id, 'user pressed stop');
    await h.engine.settle(); // the aborted job unwinds

    const after = h.log.slice(mark).map((l) => l.split(':')[0]);
    expect(after).toEqual(['compute.revoke', 'compute.revoke', 'runners.stop', 'runners.stop', 'compute.terminate']);
    expect(h.compute.live()).toEqual([]);
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
    expect(h.engine.stopReason(id)).toBe('user pressed stop');
    expect(h.ledger.candidatesWithResults(id)[0]!.verdict).toBe('pending'); // left undecided, not mis-scored
    // the aborted bench never produces a verdict afterwards
    h.bench.gate = undefined;
    await h.cycle(3);
    expect(h.ledger.candidatesWithResults(id)[0]!.verdict).toBe('pending');
  });

  it('retries a failed revocation on later ticks and keeps terminating late instances (reaper)', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.compute.failRevoke.add(agents.codex!);
    await h.engine.stop(id);
    expect(h.compute.identities.get(agents.codex!)!.revoked).toBe(false);
    expect(h.compute.identities.get(agents['claude-code']!)!.revoked).toBe(true);
    h.compute.failRevoke.clear();
    // an experiment that was mid-launch when the switch was thrown shows up afterwards
    h.compute.launchExperiment(id, agents.codex!);
    await h.cycle(1);
    expect(h.compute.identities.get(agents.codex!)!.revoked).toBe(true);
    expect(h.compute.live()).toEqual([]);
  });

  it('stop() is idempotent and a stopped optimization stays stopped', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    await h.engine.stop(id);
    const n = h.log.filter((l) => l.startsWith('runners.stop')).length;
    await h.engine.stop(id);
    expect(h.log.filter((l) => l.startsWith('runners.stop')).length).toBe(n);
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
  });
});

describe('reaper', () => {
  it('terminates experiments past their max lifetime, and instances of a stopped or finished agent', async () => {
    const h = makeHarness({ hourly: 0.01 });
    const { id, agents } = await h.begin();
    const old = h.compute.launchExperiment(id, agents.codex!);
    await h.cycle(1);
    expect(h.compute.instances.get(old.id)!.state).toBe('running');
    h.clock.advance(2 * 3_600_000 + 1);
    await h.engine.tick();
    expect(h.compute.instances.get(old.id)!.state).toBe('terminated');

    // an agent that reports done: its experiments go with it
    const exp2 = h.compute.launchExperiment(id, agents['claude-code']!);
    h.runners.emit(agents['claude-code']!, ev('status', { data: { state: 'done' } }));
    await h.cycle(1);
    await h.cycle(1);
    expect(h.compute.instances.get(exp2.id)!.state).toBe('terminated');
    expect(h.compute.instances.get(h.ledger.getAgent(agents.codex!)!.runnerInstanceId!)!.state).toBe('running');
  });

  it('terminates agent runners that outlive the time limit plus grace', async () => {
    const h = makeHarness({ hourly: 0.01, settings: { reaper: { runnerGraceMs: 0 } } });
    // pause the deadline rule by giving the loop no chance to see it first: jump straight past it
    const { id } = await h.begin({ timeLimitSeconds: 3600 });
    h.clock.advance(3600 * 1000 + 1);
    await h.engine.tick();
    expect(h.ledger.getOptimization(id)!.status).toBe('stopping'); // graceful stop first
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(h.compute.live()).toEqual([]);
  });
});

describe('agent control', () => {
  it('routes pause / resume / message to the right runner and tracks status', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    const claude = agents['claude-code']!;
    const codex = agents.codex!;

    await h.engine.pauseAgent(claude);
    expect(h.ledger.getAgent(claude)!.status).toBe('paused');
    expect(h.runners.sentTo(claude)).toEqual([{ type: 'pause' }]);
    expect(h.runners.sentTo(codex)).toEqual([]);
    await h.engine.pauseAgent(claude); // no-op
    expect(h.runners.sentTo(claude)).toHaveLength(1);

    await h.engine.messageAgent(codex, 'try caching the tokenizer');
    expect(h.runners.sentTo(codex)).toEqual([{ type: 'user', text: 'try caching the tokenizer' }]);
    expect(h.ledger.agentEvents(codex).some((e) => e.type === 'message')).toBe(true);

    // a paused agent still has its events and candidates collected
    candidate(h, 'ratchet/opt-test/claude-code', 'sha-p', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    h.runners.emit(claude, ev('message', { text: 'still thinking' }));
    await h.cycle(4);
    expect(verdicts(h, id)['sha-p']).toBe('verified');
    expect(h.ledger.agentEvents(claude).some((e) => e.type === 'message' && JSON.stringify(e.payload).includes('still thinking'))).toBe(true);

    await h.engine.resumeAgent(claude);
    expect(h.ledger.getAgent(claude)!.status).toBe('running');
    expect(h.runners.sentTo(claude).map((m) => m.type)).toEqual(['pause', 'verdict', 'resume'].filter((t) => h.runners.sentTo(claude).some((m) => m.type === t)));
    expect(h.runners.sentTo(claude).at(-1)).toEqual({ type: 'resume' });

    await expect(h.engine.messageAgent(codex, '  ')).rejects.toMatchObject({ code: 'invalid_input' });
    await h.engine.stop(id);
    await expect(h.engine.pauseAgent(codex)).rejects.toMatchObject({ code: 'invalid_state' });
    await expect(h.engine.messageAgent(codex, 'hi')).rejects.toMatchObject({ code: 'invalid_state' });
  });

  it('survives a runner that cannot be reached', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.runners.failPoll.add(agents.codex!);
    h.runners.failSend.add(agents.codex!);
    h.runners.emit(agents['claude-code']!, ev('message', { text: 'hello' }));
    candidate(h, 'ratchet/opt-test/claude-code', 'sha-1', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    await h.until(() => verdicts(h, id)['sha-1'] === 'verified'); // announcement to codex fails silently
    expect(h.engine.errors.some((e) => e.context.startsWith('poll'))).toBe(true);
    expect(h.ledger.agentEvents(agents.codex!).some((e) => e.type === 'error')).toBe(true);
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
  });
});

describe('restart and resume', () => {
  it('re-polls runners from their stored cursors: no lost and no replayed events', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    const claude = agents['claude-code']!;
    h.runners.emit(claude, ev('message', { text: 'one' }), ev('message', { text: 'two' }));
    await h.cycle(1);
    expect(h.ledger.agentEvents(claude).filter((e) => e.type === 'message')).toHaveLength(2);

    const e2 = h.restart();
    await e2.ready();
    h.runners.emit(claude, ev('message', { text: 'three' }));
    h.runners.pollCalls.length = 0;
    await h.cycle(2);
    expect(h.runners.pollCalls.find((c) => c.agentId === claude)!.cursor).toBe('2');
    const texts = h.ledger.agentEvents(claude).filter((e) => e.type === 'message').map((e) => (e.payload as AgentEvent).text);
    expect(texts).toEqual(['one', 'two', 'three']);
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
  });

  it('finishes a bench that was in flight when the process died (candidate re-queued)', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    candidate(h, 'ratchet/opt-test/codex', 'sha-1', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    h.clock.advance(TICK_MS);
    await h.engine.tick(); // candidate submitted, bench job started...
    await h.engine.settle(); // ...and finished, but the outcome was never collected
    expect(h.ledger.candidatesWithResults(id)[0]!.results).toEqual([]);
    const benchRuns = h.bench.jobs.length;

    h.restart();
    await h.until(() => verdicts(h, id)['sha-1'] === 'verified');
    expect(h.bench.jobs.length).toBe(benchRuns + 2); // bench re-run once + confirm
    expect(h.ledger.ratchetHistory(id)).toHaveLength(1);
  });

  it('continues with the confirm run when the crash came between bench and confirm', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    candidate(h, 'ratchet/opt-test/codex', 'sha-1', { bench: [80, 80, 80], confirm: [80, 80, 80] });
    await h.cycle(2); // bench recorded, confirm job done but uncollected
    const c = h.ledger.candidatesWithResults(id)[0]!;
    expect(c.results.map((r) => r.phase)).toEqual(['bench']);
    const jobs = h.bench.jobs.length;
    h.restart();
    await h.until(() => verdicts(h, id)['sha-1'] === 'verified');
    expect(h.bench.jobs.slice(jobs).map((j) => j.phase)).toEqual(['confirm']); // bench is not repeated
  });

  it('settles a candidate whose results were all written but whose verdict was not', async () => {
    // the crash window: both results are in the ledger, the verdict and ratchet move were lost
    const h2 = makeHarness({ ledgerPath: join(mkTmp(), 'l.db') });
    const { id: id2 } = await h2.begin();
    candidate(h2, 'ratchet/opt-test/codex', 'sha-2');
    h2.bench.set('sha-2', 'bench', { samples: [80, 80, 80] });
    h2.clock.advance(TICK_MS);
    await h2.engine.tick();
    await h2.engine.settle();
    const cand = h2.ledger.candidatesWithResults(id2)[0]!;
    const now = h2.clock.now();
    h2.ledger.recordBenchResult({ candidateId: cand.id, phase: 'bench', samples: [80, 80, 80], stats: { value: 80 }, startedAt: now, finishedAt: now });
    h2.ledger.recordBenchResult({ candidateId: cand.id, phase: 'confirm', samples: [79, 79, 79], stats: { value: 79 }, startedAt: now, finishedAt: now });
    h2.restart(true);
    const jobs = h2.bench.jobs.length;
    await h2.cycle(2);
    expect(verdicts(h2, id2)['sha-2']).toBe('verified');
    expect(h2.bench.jobs.length).toBe(jobs); // nothing re-run
    expect(h2.ledger.currentBest(id2)).toBe(80);
  });

  it('resumes from a SQLite file after the process dies mid-run', async () => {
    const dir = mkTmp();
    const h = makeHarness({ ledgerPath: join(dir, 'ledger.db') });
    const { id, agents } = await h.begin();
    const C = 'ratchet/opt-test/claude-code';
    candidate(h, C, 'sha-c1', { bench: [90, 90, 90], confirm: [90, 90, 90] });
    candidate(h, C, 'sha-c2', { bench: [70, 70, 70], confirm: [70, 70, 70] });
    h.runners.emit(agents.codex!, ev('message', { text: 'a' }));
    await h.until(() => verdicts(h, id)['sha-c1'] === 'verified');

    const e2 = h.restart(true);
    expect(await e2.ready()).toEqual({ attached: [id] });
    await h.until(() => verdicts(h, id)['sha-c2'] === 'verified');
    expect(h.ledger.ratchetHistory(id).map((m) => m.toValue)).toEqual([90, 70]);
    expect(h.ledger.agentEvents(agents.codex!).filter((e) => e.type === 'message')).toHaveLength(1);

    // the deadline and plateau settings survived too
    h.clock.advance(9 * 3600 * 1000);
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped');
    expect(h.engine.stopReason(id)).toBe('time limit reached');
    expect(h.ledger.getBaseline(id)!.value).toBe(100);
  });

  it('finishes launching half-launched agents after a hard crash, without repeating completed steps', async () => {
    const dir = mkTmp();
    const h = makeHarness({ ledgerPath: join(dir, 'ledger.db') });
    h.bench.set('main-sha', 'baseline', { samples: [100, 100, 100] });
    h.bench.set('main-sha', 'confirm', { samples: [100, 100, 100] });
    const id = await h.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['claude-code', 'codex'], budgetUsd: 100, timeLimitSeconds: 3600, plateauEvals: 5 });
    // the process dies while the second runner is being launched
    const realLaunch = h.compute.launchRunner.bind(h.compute);
    let n = 0;
    h.compute.launchRunner = async (spec) => {
      if (++n === 2) return new Promise(() => undefined);
      return realLaunch(spec);
    };
    void h.engine.start(id);
    for (let i = 0; i < 20 && n < 2; i++) await new Promise((r) => setTimeout(r, 5));
    expect(h.ledger.getOptimization(id)!.status).toBe('baselining');
    expect(h.ledger.getBaseline(id)).toBeDefined();

    h.compute.launchRunner = realLaunch;
    h.restart(true);
    await h.engine.ready();
    // the resumed launch does real file I/O (it re-reads the brief), so wait in real time
    for (let i = 0; i < 100 && h.ledger.getOptimization(id)!.status !== 'running'; i++) await new Promise((r) => setTimeout(r, 10));
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
    expect(h.ledger.listAgents(id).map((a) => a.status)).toEqual(['running', 'running']);
    expect(h.log.filter((l) => l.startsWith('compute.createAgentIdentity'))).toHaveLength(2); // not repeated
    expect(h.log.filter((l) => l.startsWith('runners.start'))).toHaveLength(2);
    expect(h.log.filter((l) => l.startsWith('git.createBranch'))).toHaveLength(2);
    expect(h.bench.jobs.filter((j) => j.phase === 'baseline')).toHaveLength(1); // baseline not repeated
  });

  it('completes a kill switch that was interrupted', async () => {
    const dir = mkTmp();
    const h = makeHarness({ ledgerPath: join(dir, 'ledger.db') });
    const { id } = await h.begin();
    // a crash right after the stop marker was written
    h.ledger.setOptimizationStatus(id, 'stopping');
    h.ledger.sqlite.prepare("INSERT INTO engine_kv (key, value) VALUES (?, 'kill') ON CONFLICT(key) DO UPDATE SET value = excluded.value").run(`opt:${id}:stop_mode`);
    h.restart(true);
    await h.engine.ready();
    await h.until(() => h.ledger.getOptimization(id)!.status === 'stopped', 5);
    expect(h.compute.live()).toEqual([]);
    expect([...h.compute.identities.values()].every((i) => i.revoked)).toBe(true);
  });
});

describe('teardown', () => {
  it('deletes secrets and identities, terminates leftovers, verifies zero instances and completes', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    h.compute.launchExperiment(id, agents.codex!);
    await h.engine.teardown(id); // stops a running optimization first
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
    expect(h.compute.live()).toEqual([]);
    expect(h.compute.secrets.size).toBe(0);
    expect([...h.compute.identities.values()].every((i) => i.revoked && i.deleted)).toBe(true);
    const report = h.engine.teardownReport(id)!;
    expect(report).toMatchObject({ orphans: [], errors: [], deletedSecretsFor: [agents['claude-code'], agents.codex] });
    expect(await h.artifacts.get('engine/opt-test/teardown.json')).toContain('"orphans": []');
    // idempotent
    await h.engine.teardown(id);
  });

  it('refuses to report success while an instance survives termination', async () => {
    const h = makeHarness();
    const { id, agents } = await h.begin();
    const stuck = h.compute.launchExperiment(id, agents.codex!);
    h.compute.stubborn.add(stuck.id);
    await expect(h.engine.teardown(id)).rejects.toMatchObject({ code: 'orphans' });
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
    expect(h.engine.teardownReport(id)!.orphans.map((o) => o.id)).toEqual([stuck.id]);
    h.compute.stubborn.clear();
    await h.engine.teardown(id); // retry succeeds
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
    expect(h.compute.live()).toEqual([]);
  });

  it('reports a failed secret deletion instead of completing', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    h.compute.deleteSecrets = async () => {
      throw new Error('ssm denied');
    };
    await expect(h.engine.teardown(id)).rejects.toMatchObject({ code: 'teardown_incomplete' });
    expect(h.ledger.getOptimization(id)!.status).toBe('stopped');
  });

  it('can tear down a draft that never started', async () => {
    const h = makeHarness();
    const id = await h.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 1, timeLimitSeconds: 60, plateauEvals: 1 });
    await h.engine.teardown(id);
    expect(h.ledger.getOptimization(id)!.status).toBe('completed');
  });
});

describe('run loop and events', () => {
  it('run() ticks on the injected clock until aborted', async () => {
    const h = makeHarness({ hourly: 3.6 });
    const { id } = await h.begin();
    const ac = new AbortController();
    let sleeps = 0;
    h.clock.sleep = async (ms: number) => {
      h.clock.advance(ms);
      if (++sleeps === 3) ac.abort();
    };
    const spends: number[] = [];
    h.engine.subscribe((e) => {
      if (e.type === 'spend') spends.push(e.totalUsd);
    });
    await h.engine.run(ac.signal);
    expect(sleeps).toBe(3);
    expect(spends.length).toBeGreaterThanOrEqual(2);
    expect(spends).toEqual([...spends].sort((a, b) => a - b));
    expect(h.ledger.totalSpend(id)).toBeGreaterThan(0);
  });

  it('subscribe returns an unsubscribe, and a throwing listener does not break the loop', async () => {
    const h = makeHarness();
    const seen: string[] = [];
    const off = h.engine.subscribe((e) => seen.push(e.type));
    h.engine.subscribe(() => {
      throw new Error('bad listener');
    });
    const { id } = await h.begin();
    expect(seen).toContain('optimization');
    off();
    const n = seen.length;
    await h.engine.stop(id);
    expect(seen.length).toBe(n);
  });

  it('exposes its mode', () => {
    expect(makeHarness().engine.mode()).toBe('simulation');
  });
});

function mkTmp(): string {
  const d = mkdtempSync(join(tmpdir(), 'ratchet-engine-test-'));
  tmpDirs.push(d);
  return d;
}

void EngineError;
