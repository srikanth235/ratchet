import { describe, expect, it } from 'vitest';
import { draftPassed, type EnvironmentScope } from '@ratchet/core';
import { EngineError } from '../errors.js';
import { FakeClock, FakeGit } from '../testing/fakes.js';
import { MockEnvironmentDrafter, scopeFromGoal } from './mock.js';
import { buildEnvironmentPullRequest } from './approve.js';
import { MOCK_CONFIG_YAML } from './samples.js';

const input = (goal: string, over: Record<string, unknown> = {}) => ({ repo: 'https://example.test/acme/repo.git', goal, agentConnector: 'claude-code' as const, ...over });
const ids = () => {
  let n = 0;
  return () => `env-${String(++n).padStart(4, '0')}`;
};

describe('MockEnvironmentDrafter: one deterministic draft per scope', () => {
  const cases: Array<[EnvironmentScope, string, number]> = [
    ['none', 'make the unit tests faster', 1],
    ['single', 'shrink the bundle', 1],
    ['full', 'cut checkout p95 latency of the api', 4],
  ];

  for (const [scope, goal, machines] of cases) {
    it(`${scope}: picks the scope from the goal and returns a validated, priced draft`, async () => {
      const d = new MockEnvironmentDrafter({ newId: ids() });
      const rec = await d.draft(input(goal));
      expect(rec).toMatchObject({ id: 'env-0001', status: 'ready', revision: 1 });
      const draft = rec.draft!;
      expect(draft.scope).toBe(scope);
      expect(draft.agent).toBe('claude-code');
      expect(draft.resources).toHaveLength(machines);
      expect(draftPassed(draft)).toBe(true); // every check ok, including the simulated plan
      expect(draft.checks.map((c) => c.id)).toContain('plan');
      // ratchet.yaml is always part of the draft and carries the scope
      expect(draft.files[0]!.path).toBe('ratchet.yaml');
      expect(draft.files[0]!.content).toContain(`scope: ${scope}`);
      // cost is computed from the resources, not canned
      const sum = draft.resources.reduce((a, r) => a + r.usdPerHour, 0);
      expect(draft.cost.usdPerHour).toBeCloseTo(sum, 2);
      // `none` is not free: the evaluator's pinned host is the one row. It just adds nothing beyond it.
      if (scope === 'none') expect(draft.resources[0]).toMatchObject({ name: 'Evaluator', role: 'evaluator' });
      expect(draft.cost.estimateForRunUsd).toBeGreaterThan(0);
    });
  }

  it('is deterministic: the same request yields byte-identical files and cost', async () => {
    const a = await new MockEnvironmentDrafter({ newId: ids() }).draft(input('cut p95 latency'));
    const b = await new MockEnvironmentDrafter({ newId: ids() }).draft(input('cut p95 latency'));
    expect(b.draft!.files).toEqual(a.draft!.files);
    expect(b.draft!.cost).toEqual(a.draft!.cost);
  });

  it('an explicit scope (value or function) overrides the goal heuristic', async () => {
    expect((await new MockEnvironmentDrafter({ scope: 'none' }).draft(input('p95 latency api'))).draft!.scope).toBe('none');
    const fn = new MockEnvironmentDrafter({ scope: (i) => (i.goal.includes('db') ? 'full' : 'single') });
    expect((await fn.draft(input('db thing'))).draft!.scope).toBe('full');
    expect((await fn.draft(input('other'))).draft!.scope).toBe('single');
  });

  it('prices the estimate for the run time limit it is given', async () => {
    const d = new MockEnvironmentDrafter({ scope: 'full' });
    const short = (await d.draft(input('x', { timeLimitSeconds: 3600 }))).draft!.cost;
    const long = (await d.draft(input('x', { timeLimitSeconds: 4 * 3600 }))).draft!.cost;
    expect(long.estimateForRunUsd).toBeGreaterThan(short.estimateForRunUsd);
    expect(long.usdPerHour).toBe(short.usdPerHour);
  });

  it('reports progress phases and rejects an empty goal / repo', async () => {
    const phases: string[] = [];
    await new MockEnvironmentDrafter().draft(input('goal', { onProgress: (p: { phase: string }) => phases.push(p.phase) }));
    expect(phases).toEqual(['reading', 'drafting', 'validating', 'done']);
    await expect(new MockEnvironmentDrafter().draft(input('  '))).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(new MockEnvironmentDrafter().draft(input('g', { repo: '' }))).rejects.toMatchObject({ code: 'invalid_input' });
  });

  it('scopeFromGoal', () => {
    expect(scopeFromGoal('reduce p95 of checkout')).toBe('full');
    expect(scopeFromGoal('speed up unit tests')).toBe('none');
    expect(scopeFromGoal('smaller bundle')).toBe('single');
  });
});

describe('MockEnvironmentDrafter: revise', () => {
  it('"smaller" steps the scope down, "bigger" steps it up, and each revision bumps the revision and accumulates effort', async () => {
    const d = new MockEnvironmentDrafter({ scope: 'full', newId: ids() });
    const first = await d.draft(input('x'));
    const spent1 = first.draft!.usdSpent;
    const smaller = await d.revise('env-0001', 'make it smaller and cheaper');
    expect(smaller).toMatchObject({ revision: 2, status: 'ready' });
    expect(smaller.draft!.scope).toBe('single');
    expect(smaller.draft!.usdSpent).toBeGreaterThan(spent1);
    expect((await d.revise('env-0001', 'even simpler')).draft!.scope).toBe('none');
    expect((await d.revise('env-0001', 'even simpler')).draft!.reason).toMatch(/as small as it gets/);
    const bigger = await d.revise('env-0001', 'add a database and a load generator');
    expect(bigger.draft!.scope).toBe('single');
    expect(draftPassed(bigger.draft!)).toBe(true);
    expect(bigger.messages.filter((m) => m.from === 'user')).toHaveLength(5);
  });

  it('explains why a managed database (RDS) is not used inside the isolated sandbox', async () => {
    const d = new MockEnvironmentDrafter({ scope: 'full' });
    const rec = await d.draft(input('x'));
    const after = await d.revise(rec.id, 'use RDS instead of the container');
    expect(after.draft!.scope).toBe('full');
    expect(after.draft!.reason).toMatch(/RDS/);
    expect(after.draft!.reason).toMatch(/no route out/);
  });

  it('refuses unknown drafts, empty feedback and (after approval) any change', async () => {
    const d = new MockEnvironmentDrafter({ scope: 'single' });
    await expect(d.revise('nope', 'x')).rejects.toMatchObject({ code: 'not_found' });
    const rec = await d.draft(input('x'));
    await expect(d.revise(rec.id, ' ')).rejects.toMatchObject({ code: 'invalid_input' });
    await d.approve(rec.id);
    await expect(d.revise(rec.id, 'smaller')).rejects.toMatchObject({ code: 'invalid_state' });
    expect(() => d.discard(rec.id)).toThrow(EngineError);
  });
});

describe('MockEnvironmentDrafter: approve', () => {
  it('without a git host the approval is synthetic: a locked pr by default, a sha for direct commits', async () => {
    const d = new MockEnvironmentDrafter({ scope: 'full', newId: ids() });
    const a = await d.approve((await d.draft(input('x'))).id);
    expect(a.status).toBe('approved');
    expect(a.approval).toMatchObject({ mode: 'pr', locked: true });
    expect(a.approval!.prUrl).toMatch(/^https:\/\//);
    expect(a.approval!.files).toContain('infra/ratchet/main.tf');
    const b = await d.approve((await d.draft(input('y'))).id, { mode: 'commit' });
    expect(b.approval).toMatchObject({ mode: 'commit', locked: true });
    expect(b.approval!.prUrl).toBeUndefined();
    // approving twice is idempotent
    expect(await d.approve(a.id)).toBe(a);
  });

  it('with a git host it really commits the draft files: a branch + pull request, or straight onto the base ref', async () => {
    const log: string[] = [];
    const git = new FakeGit(log, new FakeClock(), { 'ratchet.yaml': MOCK_CONFIG_YAML });
    const d = new MockEnvironmentDrafter({ scope: 'full', git, newId: ids() });

    const pr = await d.approve((await d.draft(input('x'))).id);
    expect(pr.approval).toMatchObject({ mode: 'pr', branch: 'ratchet/environment-0001', sha: 'files-1', locked: true });
    expect(git.prs).toHaveLength(1);
    expect(git.prs[0]!.body).toContain('Scope: full');
    const committed = git.filesCommitted[0]!.input;
    expect(committed.baseRef).toBe('main');
    expect(committed.files.map((f) => f.path)).toEqual(expect.arrayContaining(['ratchet.yaml', 'infra/ratchet/main.tf', 'infra/ratchet/scripts/api.sh']));

    const direct = await d.approve((await d.draft(input('y'))).id, { mode: 'commit' });
    expect(direct.approval).toMatchObject({ mode: 'commit', sha: 'files-2' });
    expect(git.filesCommitted[1]!.input).toMatchObject({ branch: 'main', update: true });
    expect(git.prs).toHaveLength(1); // a direct commit opens no pull request
  });

  it('retries once with a fresh branch name when the PR branch exists', async () => {
    const git = new FakeGit([], new FakeClock(), { 'ratchet.yaml': MOCK_CONFIG_YAML });
    git.existingBranches.add('ratchet/environment-0001');
    const d = new MockEnvironmentDrafter({ scope: 'single', git, newId: ids() });
    const rec = await d.approve((await d.draft(input('x'))).id);
    expect(rec.approval!.branch).toMatch(/^ratchet\/environment-0001-/);
  });

  it('refuses a draft with a failing check, or a git host that cannot commit files', async () => {
    const d = new MockEnvironmentDrafter({ scope: 'full' });
    const rec = await d.draft(input('x'));
    rec.draft!.checks.push({ id: 'internet', label: 'No internet on measuring hosts', ok: false, detail: 'tampered' });
    await expect(d.approve(rec.id)).rejects.toMatchObject({ code: 'invalid_state' });
    expect(rec.status).toBe('ready');

    const noCommit = new FakeGit([], new FakeClock(), {});
    (noCommit as unknown as { commitFiles?: unknown }).commitFiles = undefined;
    const d2 = new MockEnvironmentDrafter({ scope: 'none', git: noCommit, configYaml: MOCK_CONFIG_YAML });
    await expect(d2.approve((await d2.draft(input('x'))).id)).rejects.toMatchObject({ code: 'unavailable' });
  });

  it('list/get/discard manage drafts that are not approved', async () => {
    const clock = new FakeClock();
    const d = new MockEnvironmentDrafter({ clock, newId: ids() });
    const a = await d.draft(input('one'));
    clock.advance(1000);
    const b = await d.draft(input('two'));
    expect(d.list().map((r) => r.id)).toEqual([b.id, a.id]);
    d.discard(a.id);
    expect(d.get(a.id)).toBeUndefined();
  });
});

describe('buildEnvironmentPullRequest', () => {
  it('says what is created, what it costs, which checks passed and that the files are locked', async () => {
    const rec = await new MockEnvironmentDrafter({ scope: 'full' }).draft(input('cut p95'));
    const pr = buildEnvironmentPullRequest(rec, rec.draft!);
    expect(pr.title).toContain('full-stack environment');
    expect(pr.body).toContain('| Postgres'.slice(0, 2)); // the resource table
    expect(pr.body).toMatch(/\*\*Cost:\*\* \$\d+\.\d{2}\/h/);
    expect(pr.body).toContain('passed: ');
    expect(pr.body).toContain('infra/ratchet/**');
    expect(pr.body).toContain('blocked');
  });
});
