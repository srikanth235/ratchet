import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentEvent } from '@ratchet/core';
import { AcceleratedClock, DEMO_SEARCH_SCRIPT_DIR, MemoryInbox, ScriptedAgent, loadDemoSearchScript, prepareWorkdir, runRunnerLoop, LocalRunnerControl, type AgentScript } from '../src/index.js';
import { DEMO_DIR, DemoEngine, demoTestsPass, REPO_ROOT, demoConfig, linkNodeModules, makeDemoSandbox } from './demo-engine.js';
import { sh, type Sandbox } from './harness.js';

const sandboxes: Sandbox[] = [];
afterEach(() => void sandboxes.splice(0).forEach((s) => s.cleanup()));

const patchFiles = readdirSync(DEMO_SEARCH_SCRIPT_DIR).filter((f) => f.endsWith('.patch')).sort();
const patchOrder = (s: AgentScript) =>
  s.attempts.flatMap((a) => a.steps.flatMap((st) => (st.type === 'patch' ? [st.name!] : [])));

describe('demo-search patches', () => {
  it('ships the six real fixes', () => {
    expect(patchFiles).toEqual([
      '01-search-no-deep-clone.patch',
      '02-search-topk-collation-rank.patch',
      '03-filter-memoize-parse.patch',
      '04-search-filter-indexes.patch',
      '05-search-prefix-binary-search.patch',
      '06-filter-tokenizer-no-regex.patch',
    ]);
  });

  it.each(patchFiles.filter((f) => !/^0[24]-/.test(f)))('%s applies to the current examples/demo-search tree (git apply --check)', (file) => {
    execFileSync('git', ['apply', '--check', '--directory=examples/demo-search', join(DEMO_SEARCH_SCRIPT_DIR, file)], { cwd: REPO_ROOT });
  });

  it('02 and 04 build on 01 (and 02), so they are checked cumulatively in a scratch tree', () => {
    // apply 01 -> check 02 -> apply 02 -> check 04 on a throwaway copy of the tree
    const sb = makeDemoSandbox([]);
    sandboxes.push(sb);
    const apply = (f: string, check = false) =>
      execFileSync('git', ['apply', ...(check ? ['--check'] : []), join(DEMO_SEARCH_SCRIPT_DIR, f)], { cwd: sb.engine });
    expect(() => apply('02-search-topk-collation-rank.patch', true)).toThrow(); // needs 01 first
    apply('01-search-no-deep-clone.patch');
    apply('02-search-topk-collation-rank.patch', true);
    apply('02-search-topk-collation-rank.patch');
    apply('04-search-filter-indexes.patch', true);
  });

  it('cc and cx scripts use the same fixes in different orders and include all three kinds of bad attempt', () => {
    const cc = loadDemoSearchScript('cc');
    const cx = loadDemoSearchScript('cx');
    for (const s of [cc, cx]) {
      const names = patchOrder(s);
      expect(new Set(names).size).toBeGreaterThanOrEqual(6);
      const edits = s.attempts.flatMap((a) => a.steps.filter((st) => st.type === 'edit')) as { path: string; find: string }[];
      expect(edits.some((e) => e.path.startsWith('bench/'))).toBe(true); // protected path
      expect(edits.some((e) => e.find === 'const TAG_WEIGHT = 1;')).toBe(true); // contract break
      expect(edits.some((e) => e.find === '.filter(Boolean)')).toBe(true); // noise-level no-op
      expect(s.attempts.at(-1)!.name).toBe('wrap-up');
    }
    expect(patchOrder(cc)).not.toEqual(patchOrder(cx));
    // every patch step carries realistic usage in its attempt
    for (const a of [...cc.attempts, ...cx.attempts]) {
      const u = a.steps.find((st) => st.type === 'usage') as { inputTokens: number; outputTokens: number } | undefined;
      expect(u && u.inputTokens > 10_000 && u.outputTokens > 100).toBe(true);
    }
  });
});

async function playVariant(variant: 'cc' | 'cx') {
  const sb = makeDemoSandbox([variant]);
  sandboxes.push(sb);
  const config = demoConfig(sb, variant);
  const workdir = join(sb.dir, 'work', variant);
  await prepareWorkdir({ repoUrl: config.repoUrl, branch: config.branch, baseRef: config.baseRef, dir: workdir });
  linkNodeModules(workdir);
  const inbox = new MemoryInbox();
  const engine = new DemoEngine(sb, { realTests: true, workdirOf: () => workdir });
  const answer = engine.handler(variant, (m) => inbox.push(m));
  const events: AgentEvent[] = [];
  const agent = new ScriptedAgent(loadDemoSearchScript(variant), { clock: new AcceleratedClock(5000) });
  const res = await runRunnerLoop({
    config, adapter: agent, workdir, inbox, clock: new AcceleratedClock(5000), maxIdleTurns: 1,
    emit: (e) => { events.push(e); answer(e); },
  });
  return { sb, res, engine, events, workdir };
}

describe('demo-search scripted agents, one at a time, against the real demo tests', () => {
  it('cc: every good attempt keeps `pnpm test` green; the three bad attempts are blocked / rejected', async () => {
    const { res, engine, workdir } = await playVariant('cc');
    expect(engine.decisions.map((d) => d.verdict)).toEqual([
      'verified', 'blocked', 'verified', 'rejected', 'verified', 'blocked', 'verified', 'verified', 'verified',
    ]);
    expect(engine.decisions[1]!.reasons[0]).toContain('bench/local.ts');
    expect(engine.decisions[5]!.reasons[0]).toContain('pnpm test');
    expect(res.reason).toBe('idle');
    // the agent's final tree: all six fixes, bad attempts gone, tests green
    const search = readFileSync(join(workdir, 'src/search.ts'), 'utf8');
    const filter = readFileSync(join(workdir, 'src/filter.ts'), 'utf8');
    expect(search).toMatch(/class TopK/);
    expect(search).toMatch(/candidatePositions/);
    expect(search).toMatch(/sortedWordsCache/);
    expect(search).not.toMatch(/JSON\.parse\(JSON\.stringify/);
    expect(search).toMatch(/TAG_WEIGHT = 1;/);
    expect(filter).toMatch(/parseCache/);
    expect(filter).toMatch(/const CONDITION/);
    expect(sh(workdir, 'diff', '--stat', 'main', '--', 'bench')).toBe('');
    expect(res.spendUsd).toBeGreaterThan(0.5);
  }, 240_000);

  it('cx: different order, same outcome', async () => {
    const { res, engine, workdir } = await playVariant('cx');
    expect(engine.decisions.map((d) => d.verdict)).toEqual([
      'verified', 'verified', 'blocked', 'verified', 'rejected', 'verified', 'blocked', 'verified', 'verified',
    ]);
    expect(engine.decisions[6]!.reasons[0]).toContain('bench/k6/search.js');
    expect(res.reason).toBe('idle');
    expect(sh(workdir, 'diff', '--stat', 'main', '--', 'bench')).toBe('');
    expect(readFileSync(join(workdir, 'src/search.ts'), 'utf8')).toMatch(/class TopK/);
  }, 240_000);
});

describe('demo-search: both scripted agents in parallel through LocalRunnerControl', () => {
  it('share one ratchet best; the final best contains every fix and passes the demo tests', async () => {
    const sb = makeDemoSandbox(['cc', 'cx']);
    sandboxes.push(sb);
    const engine = new DemoEngine(sb); // structural guardrail only: fast
    const clock = new AcceleratedClock(5000);
    const control = new LocalRunnerControl({
      rootDir: join(sb.dir, 'runners'),
      clock,
      adapterFor: (config) => new ScriptedAgent(loadDemoSearchScript(config.agentId === 'cc' ? 'cc' : 'cx'), { clock }),
      loop: { maxIdleTurns: 2, retryDelayMs: 1 },
    });
    const handles = ['cc', 'cx'].map((a) => ({ optimizationId: 'opt1', agentId: a, instanceId: `i-${a}` }));
    await Promise.all(handles.map((h) => control.start(h, demoConfig(sb, h.agentId))));
    // engine loop: poll events, answer commits
    const cursors = new Map<string, string>();
    const results = handles.map((h) => control.result(h));
    let done = false;
    void Promise.all(results).then(() => (done = true));
    while (!done) {
      for (const h of handles) {
        const { events, cursor } = await control.poll(h, cursors.get(h.agentId));
        cursors.set(h.agentId, cursor);
        const answer = engine.handler(h.agentId, (m) => void control.send(h, m));
        events.forEach(answer);
      }
      await new Promise((r) => setTimeout(r, 20));
    }
    const final = await Promise.all(results);
    expect(final.map((r) => r.reason)).toEqual(['idle', 'idle']);

    const verdicts = engine.decisions.map((d) => d.verdict);
    expect(verdicts.filter((v) => v === 'blocked').length).toBe(4); // 2 protected-path edits + 2 contract breaks
    expect(verdicts.filter((v) => v === 'verified').length).toBeGreaterThanOrEqual(6);

    // the final ratchet best: every fix present, bad attempts absent, real tests green
    const finalDir = join(sb.dir, 'final', 'best');
    execFileSync('git', ['clone', '--quiet', '--branch', 'ratchet/best', sb.remote, finalDir]);
    const search = readFileSync(join(finalDir, 'src/search.ts'), 'utf8');
    const filter = readFileSync(join(finalDir, 'src/filter.ts'), 'utf8');
    for (const marker of [/class TopK/, /candidatePositions/, /sortedWordsCache/]) expect(search).toMatch(marker);
    expect(search).not.toMatch(/JSON\.parse\(JSON\.stringify/);
    expect(search).not.toMatch(/TAG_WEIGHT = 0/);
    expect(filter).toMatch(/parseCache/);
    expect(filter).toMatch(/const CONDITION/);
    linkNodeModules(finalDir);
    expect(demoTestsPass(finalDir)).toBe(true);
    expect(sh(finalDir, 'diff', '--stat', 'origin/main', '--', 'bench', 'tests', 'ratchet.yaml')).toBe('');
    void DEMO_DIR;
  }, 300_000);
});
