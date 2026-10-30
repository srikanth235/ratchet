import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentEvent } from '@ratchet/core';
import {
  MemoryInbox, ScriptedAgent, execGit, hasCandidateTrailer, prepareWorkdir, runRunnerLoop, systemClock, withCandidateTrailer,
  type AgentScript, type GitRunner, type RunnerLoopDeps, type ScriptStep,
} from '../src/index.js';
import { autoVerdicts, baseConfig, makeSandbox, setBest, sh, until, type Sandbox, type VerdictRule } from './harness.js';

const sandboxes: Sandbox[] = [];
afterEach(() => void sandboxes.splice(0).forEach((s) => s.cleanup()));

const write = (path: string, content: string): ScriptStep => ({ type: 'write', path, content });
const script = (...attempts: ScriptStep[][]): AgentScript => ({ name: 't', attempts: attempts.map((steps, i) => ({ name: `a${i}`, steps })) });

interface Run {
  sb: Sandbox;
  inbox: MemoryInbox;
  events: AgentEvent[];
  agent: ScriptedAgent;
  workdir: string;
  result: Promise<Awaited<ReturnType<typeof runRunnerLoop>>>;
}

async function launch(s: AgentScript, opts: { rule?: VerdictRule; auto?: boolean; sb?: Sandbox; config?: Parameters<typeof baseConfig>[1]; deps?: Partial<RunnerLoopDeps>; onVerdict?: (sha: string) => void; agent?: ScriptedAgent } = {}): Promise<Run> {
  const sb = opts.sb ?? makeSandbox();
  if (!opts.sb) sandboxes.push(sb);
  const config = baseConfig(sb, opts.config);
  const workdir = join(sb.dir, `work-${config.agentId}`);
  await prepareWorkdir({ repoUrl: config.repoUrl, branch: config.branch, baseRef: config.baseRef, dir: workdir });
  const inbox = new MemoryInbox();
  const events: AgentEvent[] = [];
  const agent = opts.agent ?? new ScriptedAgent(s);
  const answer = opts.auto === false ? undefined : autoVerdicts({ push: (m) => inbox.push(m), ...(opts.rule ? { rule: opts.rule } : {}), ...(opts.onVerdict ? { onVerdict: opts.onVerdict } : {}) });
  const result = runRunnerLoop({
    config, adapter: agent, workdir, inbox, clock: systemClock, maxIdleTurns: 1, retryDelayMs: 1,
    emit: (e) => { events.push(e); answer?.(e); },
    ...opts.deps,
  });
  return { sb, inbox, events, agent, workdir, result };
}

const types = (events: AgentEvent[]) => events.map((e) => e.type);

describe('runRunnerLoop', () => {
  it('prompt → turn → commit with trailer → push → verdict → resume → idle stop', async () => {
    const run = await launch(script(
      [{ type: 'message', text: 'Trying idea one' }, write('src/app.txt', 'line 1\nfast 2\nline 3\n'), { type: 'usage', inputTokens: 40_000, outputTokens: 2_000, costUsd: 0.25 }],
      [{ type: 'message', text: 'Idea two' }, write('src/extra.txt', 'x\n')],
    ), { deps: { maxIdleTurns: 2 } });
    const res = await run.result;
    expect(res).toMatchObject({ reason: 'idle', candidates: 2, bestValue: 16 });
    expect(res.spendUsd).toBeCloseTo(0.25);

    // remote branch has both candidate commits, each with the trailer exactly once
    sh(run.sb.engine, 'fetch', '--quiet', 'origin');
    const msgs = sh(run.sb.engine, 'log', '--format=%B%x01', 'main..origin/ratchet/a').split('\x01').map((m) => m.trim()).filter(Boolean);
    expect(msgs).toHaveLength(2);
    for (const m of msgs) expect(m.match(/Ratchet-Candidate: true/g)).toHaveLength(1);
    expect(msgs[1]).toContain('ratchet: Trying idea one');
    expect(sh(run.sb.engine, 'show', 'origin/ratchet/a:src/app.txt')).toContain('fast 2');

    // events: status, agent events, commit, verdict
    const commit = run.events.filter((e) => e.type === 'commit');
    expect(commit).toHaveLength(2);
    expect(commit[0]!.data).toMatchObject({ branch: 'ratchet/a', files: ['src/app.txt'] });
    expect(run.events.filter((e) => e.type === 'verdict').map((e) => e.data?.['verdict'])).toEqual(['verified', 'verified']);
    expect(types(run.events).indexOf('commit')).toBeLessThan(types(run.events).indexOf('verdict'));
    expect(run.events.at(-1)).toMatchObject({ type: 'status', text: 'runner finished: idle' });

    // the second turn got a resume message with the verdict, best and limits
    expect(run.agent.received[0]).toContain('Verdict for candidate 1');
    expect(run.agent.received[0]).toContain('VERIFIED');
    expect(run.agent.received[0]).toContain('Current ratchet best: 18');
    expect(run.agent.received[0]).toMatch(/Budget remaining: \$49\.75/);
    expect(run.agent.received[1]).toContain('Verdict for candidate 2');
    expect(run.agent.received[2]).toContain('No change to the repository was detected');

    // context.md exists in the workdir, untracked and unstaged
    const ctx = readFileSync(join(run.workdir, '.ratchet', 'context.md'), 'utf8');
    expect(ctx).toContain('http_req_duration.p95');
    expect(ctx).toContain('| 1 |');
    expect(sh(run.workdir, 'status', '--porcelain')).toBe('');
    expect(sh(run.workdir, 'ls-files', '.ratchet')).toBe('');
  });

  it('the first prompt carries the RATCHET.md brief and the generated context', async () => {
    const prompts: string[] = [];
    const agent = new ScriptedAgent(script([]));
    const orig = agent.start.bind(agent);
    agent.start = (spec, onEvent, signal) => (prompts.push(spec.prompt), orig(spec, onEvent, signal));
    const run = await launch(script([]), { agent });
    await run.result;
    expect(prompts[0]).toContain('# Brief');
    expect(prompts[0]).toContain('Make it faster');
    expect(prompts[0]).toContain('## Objective');
    expect(prompts[0]).toContain('Baseline: 20');
    expect(prompts[0]).toContain('Protected paths');
    expect(prompts[0]).toContain('`bench/**`');
    expect(prompts[0]).toContain('`pnpm test` must pass');
    expect(prompts[0]).toContain('`error_rate` < 0.001');
    expect(prompts[0]).toContain('Budget remaining: $50.00');
  });

  it('a commit the agent made itself gets the trailer added (message kept)', async () => {
    const run = await launch(script([write('src/app.txt', 'changed\n'), { type: 'git', args: ['add', '-A'] }, { type: 'git', args: ['commit', '-q', '-m', 'agent: memoize parser\n\nmeasured -12%'] }]));
    await run.result;
    sh(run.sb.engine, 'fetch', '--quiet', 'origin');
    const msg = sh(run.sb.engine, 'log', '-1', '--format=%B', 'origin/ratchet/a');
    expect(msg).toBe('agent: memoize parser\n\nmeasured -12%\n\nRatchet-Candidate: true');
  });

  it('a dirty tree on top of an agent commit is committed too (one candidate, not two)', async () => {
    const run = await launch(script([write('a.txt', '1\n'), { type: 'git', args: ['add', '-A'] }, { type: 'git', args: ['commit', '-q', '-m', 'first'] }, write('b.txt', '2\n')]));
    await run.result;
    expect(run.events.filter((e) => e.type === 'commit')).toHaveLength(1);
    sh(run.sb.engine, 'fetch', '--quiet', 'origin');
    expect(sh(run.sb.engine, 'show', '--name-only', '--format=', 'origin/ratchet/a')).toBe('b.txt');
  });

  it('records rejected / blocked verdicts, tells the agent, and `refs/ratchet/accepted` lets it drop the work', async () => {
    const rule: VerdictRule = ({ n, files }) =>
      n === 1 ? { verdict: 'blocked', reasons: [`touches protected path ${files[0]}`] } : n === 2 ? { verdict: 'rejected', reasons: ['gain 0.4% below noise'] } : undefined;
    const run = await launch(script(
      [write('bench/run.sh', 'echo cheat\n')],
      [{ type: 'git', args: ['reset', '--hard', 'refs/ratchet/accepted'] }, write('src/app.txt', 'noise\n')],
      [{ type: 'git', args: ['reset', '--hard', 'refs/ratchet/accepted'] }, write('src/app.txt', 'real\n')],
    ), { rule, deps: { maxIdleTurns: 1 } });
    const res = await run.result;
    expect(res.history.map((h) => h.verdict)).toEqual(['blocked', 'rejected', 'verified']);
    expect(run.agent.received[0]).toContain('BLOCKED');
    expect(run.agent.received[0]).toContain('touches protected path bench/run.sh');
    expect(run.agent.received[0]).toContain('git reset --hard refs/ratchet/accepted');
    // the cheat was dropped: the final tree has the original bench script
    expect(readFileSync(join(run.workdir, 'bench/run.sh'), 'utf8')).toBe('echo bench\n');
    expect(readFileSync(join(run.workdir, 'src/app.txt'), 'utf8')).toBe('real\n');
    expect(res.bestValue).toBe(18); // only the verified one moved it
    // a later candidate's `files` is relative to the accepted tree, not the whole branch history
    const files = run.events.filter((e) => e.type === 'commit').map((e) => e.data?.['files']);
    expect(files).toEqual([['bench/run.sh'], ['src/app.txt'], ['src/app.txt']]);
  });

  it('rebases onto the ratchet best when it moved, preferring the best on conflict, and tells the agent', async () => {
    const sb = makeSandbox();
    sandboxes.push(sb);
    // another agent's verified commit becomes best: edits line 2 and adds a file
    sh(sb.engine, 'checkout', '-q', '-b', 'other', 'origin/main');
    writeFileSyncIn(sb.engine, 'src/app.txt', 'line 1\nBEST 2\nline 3\n');
    writeFileSyncIn(sb.engine, 'src/best.txt', 'best\n');
    sh(sb.engine, 'add', '-A');
    sh(sb.engine, 'commit', '-q', '-m', 'best');
    const bestSha = sh(sb.engine, 'rev-parse', 'HEAD');
    // our agent: conflicting edit of line 2 plus a separate file, rejected
    const run = await launch(script(
      [write('src/app.txt', 'line 1\nMINE 2\nline 3\n'), write('src/mine.txt', 'mine\n')],
      [],
    ), {
      sb,
      rule: () => ({ verdict: 'rejected', reasons: ['below noise'] }),
      onVerdict: () => setBest(sb, bestSha),
    });
    await run.result;
    expect(readFileSync(join(run.workdir, 'src/app.txt'), 'utf8')).toBe('line 1\nBEST 2\nline 3\n'); // best wins the conflict
    expect(readFileSync(join(run.workdir, 'src/best.txt'), 'utf8')).toBe('best\n');
    expect(readFileSync(join(run.workdir, 'src/mine.txt'), 'utf8')).toBe('mine\n'); // non-conflicting work survives
    expect(sh(run.workdir, 'merge-base', '--is-ancestor', bestSha, 'HEAD') ?? '').toBe('');
    expect(sh(run.workdir, 'rev-parse', 'refs/ratchet/accepted')).toBe(bestSha);
    expect(run.agent.received[0]).toMatch(/ratchet best moved .* rebased onto it; where they conflicted, the best's version was kept/s);
    expect(run.events.some((e) => e.type === 'status' && /rebased onto ratchet best/.test(e.text ?? ''))).toBe(true);
    // the next candidate is pushed with the rewritten history (force-with-lease on the agent's own branch)
    expect(existsSync(join(run.workdir, '.git', 'rebase-merge'))).toBe(false);
  });

  it('when the rebase cannot be resolved (modify/delete), resets to the best and says so', async () => {
    const sb = makeSandbox();
    sandboxes.push(sb);
    sh(sb.engine, 'checkout', '-q', '-b', 'other', 'origin/main');
    writeFileSyncIn(sb.engine, 'src/app.txt', 'line 1\nBEST 2\nline 3\n');
    sh(sb.engine, 'commit', '-q', '-am', 'best');
    const bestSha = sh(sb.engine, 'rev-parse', 'HEAD');
    const run = await launch(script([{ type: 'delete', path: 'src/app.txt' }], []), { sb, rule: () => ({ verdict: 'rejected' }), onVerdict: () => setBest(sb, bestSha) });
    await run.result;
    expect(sh(run.workdir, 'rev-parse', 'HEAD')).toBe(bestSha);
    expect(readFileSync(join(run.workdir, 'src/app.txt'), 'utf8')).toContain('BEST 2');
    expect(run.agent.received[0]).toMatch(/conflicted, so your branch was reset to the best/);
    expect(existsSync(join(run.workdir, '.git', 'rebase-merge'))).toBe(false);
  });

  it('pause: finishes the turn, waits, and only resumes the agent after `resume`', async () => {
    const run = await launch(script([write('a.txt', '1\n')], [write('b.txt', '2\n')]), { auto: false });
    await until(() => run.events.some((e) => e.type === 'commit'));
    const sha = String(run.events.find((e) => e.type === 'commit')!.data?.['sha']);
    run.inbox.push({ type: 'pause' });
    run.inbox.push({ type: 'verdict', sha, verdict: 'verified', reasons: [], currentBest: 17 });
    await until(() => run.events.some((e) => e.text === 'paused'));
    await new Promise((r) => setTimeout(r, 100));
    expect(run.agent.received).toEqual([]); // verdict recorded, agent not resumed
    run.inbox.push({ type: 'resume' });
    await until(() => run.agent.received.length === 1);
    expect(run.agent.received[0]).toContain('VERIFIED');
    expect(run.events.some((e) => e.text === 'resumed')).toBe(true);
    run.inbox.push({ type: 'stop' });
    expect((await run.result).reason).toBe('stopped');
  });

  it('user messages are delivered with the next resume', async () => {
    const run = await launch(script([write('a.txt', '1\n')], [write('b.txt', '2\n')]), { auto: false });
    await until(() => run.events.some((e) => e.type === 'commit'));
    const sha = String(run.events.find((e) => e.type === 'commit')!.data?.['sha']);
    run.inbox.push({ type: 'user', text: 'focus on the filter parser please' });
    run.inbox.push({ type: 'verdict', sha, verdict: 'rejected', reasons: ['x'], currentBest: 20 });
    await until(() => run.agent.received.length === 1);
    expect(run.agent.received[0]).toContain('Message from the user: focus on the filter parser please');
    run.inbox.push({ type: 'stop' });
    await run.result;
  });

  it('stop mid-turn aborts the agent promptly and commits nothing', async () => {
    const run = await launch(script([write('a.txt', '1\n'), { type: 'sleep', ms: 60_000 }]), { auto: false });
    await until(() => run.events.some((e) => e.text === 'agent turn started'));
    await new Promise((r) => setTimeout(r, 50));
    const t0 = Date.now();
    run.inbox.push({ type: 'stop' });
    const res = await run.result;
    expect(Date.now() - t0).toBeLessThan(2000);
    expect(res).toMatchObject({ reason: 'stopped', candidates: 0 });
    expect(run.events.some((e) => e.type === 'commit')).toBe(false);
  });

  it('stop while waiting for a verdict', async () => {
    const run = await launch(script([write('a.txt', '1\n')]), { auto: false });
    await until(() => run.events.some((e) => e.text === 'waiting for verdict'));
    run.inbox.push({ type: 'stop' });
    expect((await run.result).reason).toBe('stopped');
  });

  it('external AbortSignal ends the loop as aborted', async () => {
    const ac = new AbortController();
    const run = await launch(script([{ type: 'sleep', ms: 60_000 }]), { auto: false, deps: { signal: ac.signal } });
    await until(() => run.events.some((e) => e.text === 'agent turn started'));
    ac.abort();
    expect((await run.result).reason).toBe('aborted');
  });

  it('deadline: stops mid-turn, and while waiting for a verdict', async () => {
    const midTurn = await launch(script([{ type: 'sleep', ms: 60_000 }]), { auto: false, config: { deadlineAt: Date.now() + 300 } });
    expect((await midTurn.result).reason).toBe('deadline');
    const waiting = await launch(script([write('a.txt', '1\n')]), { auto: false, config: { deadlineAt: Date.now() + 2500 } });
    const res = await waiting.result;
    expect(res.reason).toBe('deadline');
    expect(waiting.events.some((e) => e.type === 'commit')).toBe(true); // it did submit before the deadline
  });

  it('budget: the turn that crosses it is submitted and judged, then the loop stops without resuming', async () => {
    const run = await launch(script(
      [write('a.txt', '1\n'), { type: 'usage', inputTokens: 90_000, outputTokens: 5_000, costUsd: 0.6 }],
      [write('b.txt', '2\n')],
    ), { config: { budgetUsd: 0.5 } });
    const res = await run.result;
    expect(res).toMatchObject({ reason: 'budget', candidates: 1 });
    expect(res.spendUsd).toBeCloseTo(0.6);
    expect(run.agent.received).toEqual([]);
  });

  it('budget can be estimated from tokens when the adapter reports no cost', async () => {
    const agent = new ScriptedAgent(script([write('a.txt', '1\n'), { type: 'usage', inputTokens: 1_000_000, outputTokens: 0 }]));
    const start = agent.start.bind(agent);
    agent.start = (spec, onEvent, signal) => start(spec, (e) => onEvent(e.usage ? { ...e, usage: { inputTokens: e.usage.inputTokens, outputTokens: e.usage.outputTokens } } : e), signal); // adapter reports tokens only
    const run = await launch(script(), { agent, config: { budgetUsd: 1 }, deps: { estimateCost: (u) => (u.inputTokens / 1e6) * 3 } });
    const res = await run.result;
    expect(res).toMatchObject({ reason: 'budget' });
    expect(res.spendUsd).toBeCloseTo(3);
  });

  it('retries a crashed turn (telling the agent), gives up after maxConsecutiveFailures', async () => {
    const flaky = await launch(script(
      [{ type: 'fail', message: 'segfault' }],
      [write('a.txt', '1\n')],
    ), { deps: { maxConsecutiveFailures: 3 } });
    // start crashed -> retried as a fresh start (attempt index advanced by the scripted agent)
    const res = await flaky.result;
    expect(flaky.events.filter((e) => e.type === 'error' && /agent turn failed: segfault/.test(e.text ?? ''))).toHaveLength(1);
    expect(res.candidates).toBe(1);

    const doomed = await launch(script([{ type: 'fail', message: 'a' }], [{ type: 'fail', message: 'b' }], [{ type: 'fail', message: 'c' }]), { deps: { maxConsecutiveFailures: 3 } });
    expect((await doomed.result).reason).toBe('failed');
  });

  it('agent preflight failure ends the run as agent-unavailable', async () => {
    const agent = new ScriptedAgent(script());
    const run = await launch(script(), { agent });
    const res = await run.result;
    expect(res.reason).toBe('agent-unavailable');
    expect(run.events.find((e) => e.type === 'error')!.text).toContain('script has no attempts');
  });

  it('push is retried, git is injectable and audited', async () => {
    const calls: string[][] = [];
    let pushFailures = 1;
    const git: GitRunner = async (args, opts) => {
      calls.push(args);
      if (args[0] === 'push' && pushFailures-- > 0) return { code: 1, stdout: '', stderr: 'remote: temporary failure' };
      return execGit(args, opts);
    };
    const run = await launch(script([write('a.txt', '1\n')]), { deps: { git, pushRetries: 2, pushRetryDelayMs: 1 } });
    const res = await run.result;
    expect(res.candidates).toBe(1);
    expect(calls.filter((c) => c[0] === 'push')).toHaveLength(2);
    expect(calls.find((c) => c[0] === 'push')).toEqual(['push', '--quiet', '--force-with-lease', 'origin', 'HEAD:refs/heads/ratchet/a']);
  });

  it('gives up with failed when push never succeeds', async () => {
    const git: GitRunner = async (args, opts) => (args[0] === 'push' ? { code: 1, stdout: '', stderr: 'denied' } : execGit(args, opts));
    const run = await launch(script([write('a.txt', '1\n')]), { deps: { git, pushRetries: 1, pushRetryDelayMs: 1 }, auto: false });
    const res = await run.result;
    expect(res.reason).toBe('failed');
    expect(run.events.some((e) => e.type === 'error' && /push failed: denied/.test(e.text ?? ''))).toBe(true);
  });
});

describe('candidate trailer', () => {
  it('is added once, after a blank line', () => {
    expect(withCandidateTrailer('subject')).toBe('subject\n\nRatchet-Candidate: true\n');
    expect(withCandidateTrailer('subject\n\nbody\n')).toBe('subject\n\nbody\n\nRatchet-Candidate: true\n');
    expect(withCandidateTrailer('s\n\nRatchet-Candidate: true\n')).toBe('s\n\nRatchet-Candidate: true\n');
  });
  it('joins an existing trailer block', () => {
    expect(withCandidateTrailer('s\n\nbody\n\nSigned-off-by: A <a@b>\n')).toBe('s\n\nbody\n\nSigned-off-by: A <a@b>\nRatchet-Candidate: true\n');
  });
  it('detects it', () => {
    expect(hasCandidateTrailer('s\n\nRatchet-Candidate: true')).toBe(true);
    expect(hasCandidateTrailer('s\n\nRatchet-Candidate: false')).toBe(false);
  });
});

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
function writeFileSyncIn(dir: string, path: string, content: string): void {
  mkdirSync(dirname(join(dir, path)), { recursive: true });
  writeFileSync(join(dir, path), content);
}
