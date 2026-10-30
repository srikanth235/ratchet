import { describe, expect, it } from 'vitest';
import { CALIBRATION, type EngineEvent } from '@ratchet/core';
import { EngineError } from './errors.js';
import { makeHarness, type Harness } from './testing/harness.js';

const AGENT = `agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.xlarge]
`;

/** No repeats, no min_gain, no protected list: everything is derived. */
const AUTO_YAML = `version: 0
objective: { metric: latency_ms, direction: minimize }
evaluator:
  run: node eval/score.mjs
  hardware: c7i.2xlarge
  holdout: s3://sealed/trace.jsonl
guardrails:
  - run: pytest -q
${AGENT}`;

const TEMPLATE_YAML = `version: 0
objective: { metric: http_req_duration.p95, direction: minimize }
evaluator:
  template: http-latency
  holdout: s3://sealed/trace.jsonl
${AGENT}`;

const BRANCH = 'ratchet/opt-test/claude-code';

function candidate(h: Harness, sha: string, opts: { files?: string[]; bench?: number[]; confirm?: number[] } = {}, branch = BRANCH): void {
  h.git.push(branch, sha, opts.files ?? ['src/search.ts']);
  if (opts.bench) h.bench.set(sha, 'bench', { samples: opts.bench, guardrails: [{ name: 'pytest -q', passed: true }] });
  if (opts.confirm) h.bench.set(sha, 'confirm', { samples: opts.confirm });
}

const verdicts = (h: Harness, id: string): Record<string, string> =>
  Object.fromEntries(h.ledger.candidatesWithResults(id).map((c) => [c.sha, c.verdict]));

const settled = (h: Harness, id: string, n: number) => () =>
  h.ledger.candidatesWithResults(id).length === n && h.ledger.candidatesWithResults(id).every((c) => c.verdict !== 'pending');

describe('auto-calibrated statistics', () => {
  it('derives repeats and the win threshold from the baseline and says so in plain words', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML });
    const events: EngineEvent[] = [];
    h.engine.subscribe((e) => events.push(e));
    const { id } = await h.begin({ agents: ['claude-code'], baseline: [100, 102, 98] });

    // the baseline had no repeats to go by: the default number of runs
    expect(h.bench.jobs.find((j) => j.phase === 'baseline')!.repeats).toBe(CALIBRATION.baselineRepeats);
    // CV = 2% -> noise +-4%, win threshold 4%, 5 runs per candidate
    const cal = (await h.engine.calibration(id))!;
    expect(cal).toMatchObject({ baselineRuns: 3, repeats: 5, repeatsSource: 'auto', minGain: 0.04, minGainSource: 'auto', repeatsCapped: false });
    expect(cal.cv).toBeCloseTo(0.02, 6);
    expect(cal.explanation).toBe('this metric is noisy (±4%); wins must beat 4%; each candidate is measured 5 times');

    // the UI hears about it
    const notice = events.find((e) => e.type === 'notice' && /Calibrated/.test(e.text));
    expect(notice).toMatchObject({ optimizationId: id, level: 'info' });
    expect((notice as { text: string }).text).toContain(cal.explanation);

    // candidates and the confirm run use the calibrated repeats; the ledger row keeps a provisional threshold
    candidate(h, 'sha-1', { bench: [90, 90, 90], confirm: [91, 91, 91] });
    await h.until(settled(h, id, 1));
    const jobs = h.bench.jobs.filter((j) => j.candidateId);
    expect(jobs.map((j) => [j.phase, j.repeats])).toEqual([['bench', 5], ['confirm', 5]]);
    expect(h.bench.jobs.find((j) => j.phase === 'confirm' && !j.candidateId)!.repeats).toBe(5); // the baseline's own confirm run
    expect(h.ledger.getOptimization(id)!.minGain).toBe(CALIBRATION.fallbackGain);
  });

  it('the calibrated threshold decides verdicts (noise floor 4 and 4% of 100 = 4: a gain of exactly 4 is not a win)', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML });
    const { id } = await h.begin({ agents: ['claude-code'], baseline: [100, 102, 98] });
    expect(h.ledger.getBaseline(id)!.noiseFloor).toBe(4);
    candidate(h, 'sha-tie', { bench: [96, 96, 96] });
    candidate(h, 'sha-win', { bench: [95, 95, 95], confirm: [94, 94, 94] });
    await h.until(settled(h, id, 2));
    expect(verdicts(h, id)).toEqual({ 'sha-tie': 'rejected', 'sha-win': 'verified' });
    expect(h.ledger.candidatesWithResults(id).find((c) => c.sha === 'sha-tie')!.reasons[0]).toMatch(/does not exceed required gain 4/);
  });

  it('a noisy baseline raises the threshold instead of crowning noise', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML });
    const { id } = await h.begin({ agents: ['claude-code'], baseline: [100, 130, 70, 120, 80] });
    const cal = (await h.engine.calibration(id))!;
    expect(cal.cv).toBeCloseTo(0.255, 3);
    expect(cal.minGain).toBe(0.55);
    expect(cal.repeats).toBe(5);
    expect(cal.explanation).toMatch(/^this metric is very noisy \(±51%\); wins must beat 55%/);
    candidate(h, 'sha-1', { bench: [60, 60, 60] }); // 40% better than 100, still inside the noise
    await h.until(settled(h, id, 1));
    expect(verdicts(h, id)).toEqual({ 'sha-1': 'rejected' });
  });

  it('explicit repeats and min_gain in ratchet.yaml are respected as they were', async () => {
    const h = makeHarness(); // CONFIG_YAML: repeats 3, min_gain 0.03
    const { id } = await h.begin({ agents: ['claude-code'] });
    const cal = (await h.engine.calibration(id))!;
    expect(cal).toMatchObject({ repeats: 3, repeatsSource: 'config', minGain: 0.03, minGainSource: 'config' });
    expect(cal.explanation).toContain('(set in ratchet.yaml)');
    expect(h.bench.jobs.every((j) => j.repeats === 3)).toBe(true);
  });

  it('noise_factor in ratchet.yaml sets the k of the noise floor', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML.replace('  hardware: c7i.2xlarge', '  hardware: c7i.2xlarge\n  noise_factor: 3') });
    const { id } = await h.begin({ agents: ['claude-code'], baseline: [100, 102, 98] });
    expect(h.ledger.getBaseline(id)!.noiseFloor).toBe(6);
    expect((await h.engine.calibration(id))!).toMatchObject({ noiseFactor: 3, noiseFactorSource: 'config' });
  });

  it('survives a restart, and is re-derived from the ledger when the remembered copy is missing', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML });
    const { id } = await h.begin({ agents: ['claude-code'], baseline: [100, 102, 98] });
    const before = await h.engine.calibration(id);
    const e2 = h.restart();
    expect(await e2.calibration(id)).toEqual(before);
    h.ledger.sqlite.prepare('DELETE FROM engine_kv WHERE key = ?').run(`opt:${id}:calibration`);
    expect(await h.restart().calibration(id)).toEqual(before);
  });

  it('is null before the baseline has run, and unknown ids are refused', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML });
    const id = await h.engine.createOptimization({ repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['claude-code'], budgetUsd: 10, timeLimitSeconds: 3600, plateauEvals: 5 });
    expect(await h.engine.calibration(id)).toBeNull();
    await expect(h.engine.calibration('nope')).rejects.toBeInstanceOf(EngineError);
  });
});

describe('automatic protected paths', () => {
  it('blocks lockfile and install-config changes without any protected: list, before anything is benched', async () => {
    const h = makeHarness({ configYaml: AUTO_YAML });
    const { id } = await h.begin({ agents: ['claude-code'] });
    candidate(h, 'sha-lock', { files: ['src/a.ts', 'pnpm-lock.yaml'] });
    candidate(h, 'sha-npmrc', { files: ['packages/api/.npmrc'] });
    candidate(h, 'sha-cfg', { files: ['ratchet.yaml'] });
    candidate(h, 'sha-eval', { files: ['eval/score.mjs'] }); // referenced by evaluator.run
    candidate(h, 'sha-ok', { files: ['src/search.ts'], bench: [101, 101, 101] });
    await h.until(settled(h, id, 5));
    expect(verdicts(h, id)).toEqual({ 'sha-lock': 'blocked', 'sha-npmrc': 'blocked', 'sha-cfg': 'blocked', 'sha-eval': 'blocked', 'sha-ok': 'rejected' });
    const reason = (sha: string) => h.ledger.candidatesWithResults(id).find((c) => c.sha === sha)!.reasons[0];
    expect(reason('sha-lock')).toBe('protected path changed: pnpm-lock.yaml (matches **/pnpm-lock.yaml)');
    expect(reason('sha-eval')).toBe('protected path changed: eval/score.mjs (matches eval/**)');
    expect(h.bench.jobs.filter((j) => j.candidateId).map((j) => j.sha)).toEqual(['sha-ok']); // the blocked ones never cost a bench run
  });

  it("a template's evaluator files are protected, and the user's list still adds to them", async () => {
    const h = makeHarness({ configYaml: `${TEMPLATE_YAML}protected:\n  - tests/contract/**\n` });
    const { id } = await h.begin({ agents: ['claude-code'] });
    candidate(h, 'sha-1', { files: ['bench/http-latency.config.json'] });
    candidate(h, 'sha-2', { files: ['tests/contract/api.test.ts'] });
    await h.until(settled(h, id, 2));
    expect(verdicts(h, id)).toEqual({ 'sha-1': 'blocked', 'sha-2': 'blocked' });
    expect(h.ledger.candidatesWithResults(id).find((c) => c.sha === 'sha-1')!.reasons[0]).toMatch(/matches bench\/\*\*/);
  });

  it('protectedPaths(id) lists the effective set with the reason for each entry', async () => {
    const h = makeHarness({ configYaml: `${TEMPLATE_YAML}protected:\n  - extra/**\n` });
    const { id } = await h.begin({ agents: ['claude-code'] });
    const p = await h.engine.protectedPaths(id);
    expect(p.globs).toEqual(expect.arrayContaining(['ratchet.yaml', 'bench/**', '**/pnpm-lock.yaml', '**/.npmrc', '**/pnpm-workspace.yaml', '**/.pnpmfile.cjs', '**/package-lock.json', '**/npm-shrinkwrap.json', 'extra/**']));
    expect(p.entries.find((e) => e.glob === 'extra/**')).toMatchObject({ source: 'user' });
    expect(p.entries.find((e) => e.glob === 'bench/**')).toMatchObject({ source: 'evaluator' });
  });
});

describe('listTemplates and proposeTemplate', () => {
  const base = { repoUrl: 'https://example.test/acme/repo.git', baseRef: 'main' };
  const emptyRepo = (h: Harness) => {
    h.git.files = { 'README.md': 'hello\n' };
  };

  it('lists the gallery: the seven templates, then the demos', async () => {
    const h = makeHarness();
    const list = await h.engine.listTemplates();
    expect(list.map((t) => t.id)).toEqual([
      'http-latency', 'cli-cold-start', 'test-suite-time', 'build-size', 'memory', 'forecast-backtest', 'llm-eval',
      'demo-search', 'demo-cli', 'demo-forecast',
    ]);
  });

  it('commits the template on a new branch and opens a PR that says what was added and how to run a baseline', async () => {
    const h = makeHarness();
    emptyRepo(h);
    const res = await h.engine.proposeTemplate({ ...base, templateId: 'http-latency' });

    expect(res).toMatchObject({ branch: 'ratchet/template-http-latency', sha: 'files-1', templateId: 'http-latency', prUrl: 'https://example.test/pr/1' });
    expect(res.files).toEqual(['ratchet.yaml', 'RATCHET.md', 'bench/http-latency.config.json', 'bench/http-latency.mjs']);

    const commit = h.git.filesCommitted[0]!.input;
    expect(commit).toMatchObject({ baseRef: 'main', branch: 'ratchet/template-http-latency' });
    expect(commit.files.map((f) => f.path)).toEqual(res.files);
    expect(commit.files[0]!.content).toContain('template: http-latency');

    const pr = h.git.prs[0]!;
    expect(pr).toMatchObject({ base: 'main', head: 'ratchet/template-http-latency', shas: [] });
    expect(pr.title).toContain('HTTP latency');
    expect(pr.body).toContain('What was added');
    expect(pr.body).toContain('`bench/http-latency.mjs`');
    expect(pr.body).toContain('Run a baseline');
    expect(pr.body).toContain('ratchet baseline');
    expect(h.log.filter((l) => l.startsWith('git.commitFiles') || l === 'git.openPullRequest')).toEqual(['git.commitFiles:ratchet/template-http-latency', 'git.openPullRequest']);
  });

  it('passes the objective options through, and accepts a legacy pack name', async () => {
    const h = makeHarness();
    emptyRepo(h);
    const res = await h.engine.proposeTemplate({ ...base, templateId: 'forecasting', metric: 'mape', direction: 'minimize', valueUsd: 250 });
    expect(res.templateId).toBe('forecast-backtest');
    const yaml = h.git.filesCommitted[0]!.input.files[0]!.content;
    expect(yaml).toContain('metric: mape');
    expect(yaml).toContain('usd_per_unit: 250');
  });

  it('refuses when files of the template already exist, unless overwrite is given', async () => {
    const h = makeHarness(); // the repo has ratchet.yaml and RATCHET.md
    const err = await h.engine.proposeTemplate({ ...base, templateId: 'cli-cold-start' }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(EngineError);
    expect((err as EngineError).code).toBe('invalid_state');
    expect((err as EngineError).message).toMatch(/ratchet\.yaml, RATCHET\.md already exist on main/);
    expect(h.git.filesCommitted).toEqual([]);
    expect(h.git.prs).toEqual([]);

    const ok = await h.engine.proposeTemplate({ ...base, templateId: 'cli-cold-start', overwrite: true });
    expect(ok.prUrl).toBeTruthy();
  });

  it('takes another branch name when the first is taken, and gives up with the original error when that fails too', async () => {
    const h = makeHarness();
    emptyRepo(h);
    h.git.existingBranches.add('ratchet/template-memory');
    const res = await h.engine.proposeTemplate({ ...base, templateId: 'memory' });
    expect(res.branch).toMatch(/^ratchet\/template-memory-[a-z0-9]+$/);
    expect(h.git.prs[0]!.head).toBe(res.branch);

    h.git.failCommitFiles = true;
    await expect(h.engine.proposeTemplate({ ...base, templateId: 'memory' })).rejects.toThrow(/already exists/);
    expect(h.git.prs).toHaveLength(1);
  });

  it('rejects demos, unknown templates, bad options and a host that cannot commit files', async () => {
    const h = makeHarness();
    emptyRepo(h);
    const code = async (input: Parameters<typeof h.engine.proposeTemplate>[0]) => ((await h.engine.proposeTemplate(input).catch((e: unknown) => e)) as EngineError).code;
    expect(await code({ ...base, templateId: 'demo-search' })).toBe('invalid_input');
    expect(await code({ ...base, templateId: 'nope' })).toBe('invalid_input');
    expect(await code({ ...base, templateId: 'llm-eval', valueUsd: -1 })).toBe('invalid_input');
    expect(await code({ ...base, templateId: 'llm-eval', direction: 'sideways' as never })).toBe('invalid_input');
    expect(await code({ repoUrl: '', baseRef: 'main', templateId: 'llm-eval' })).toBe('invalid_input');
    expect(await code({ ...base, baseRef: '', templateId: 'llm-eval' })).toBe('invalid_input');

    (h.git as { commitFiles?: unknown }).commitFiles = undefined;
    expect(await code({ ...base, templateId: 'llm-eval' })).toBe('unavailable');
    expect(h.git.prs).toEqual([]);
  });
});
