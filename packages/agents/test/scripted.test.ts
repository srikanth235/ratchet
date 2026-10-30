import { mkdtempSync, readFileSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { AcceleratedClock, AgentAbortedError, AgentError, ManualClock, ScriptedAgent, execGit, priceUsage, type AgentScript } from '../src/index.js';
import { collect } from './helpers.js';

const dirs: string[] = [];
afterEach(() => void dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));
async function repo(files: Record<string, string> = { 'a.txt': 'one\ntwo\nthree\n' }): Promise<string> {
  const dir = mkdtempSync(join(tmpdir(), 'scripted-'));
  dirs.push(dir);
  for (const [k, v] of Object.entries(files)) {
    mkdirSync(join(dir, k, '..'), { recursive: true });
    writeFileSync(join(dir, k), v);
  }
  await execGit(['init', '-q'], { cwd: dir });
  return dir;
}
const script = (...attempts: AgentScript['attempts'][number]['steps'][]): AgentScript => ({ name: 's', attempts: attempts.map((steps, i) => ({ name: `a${i}`, steps })) });
const PATCH = `diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n`;

describe('ScriptedAgent', () => {
  it('start plays attempt 0, each resume the next, then runs out of ideas', async () => {
    const workdir = await repo();
    const agent = new ScriptedAgent(script([{ type: 'message', text: 'first' }], [{ type: 'message', text: 'second' }]));
    const a = collect();
    const session = await agent.start({ workdir, prompt: 'p' }, a.onEvent);
    expect(session).toMatchObject({ kind: 'mock' });
    const b = collect();
    await agent.resume(session, { workdir, prompt: '' }, 'verdict A', b.onEvent);
    const c = collect();
    await agent.resume(session, { workdir, prompt: '' }, 'verdict B', c.onEvent);
    expect(a.events.filter((e) => e.type === 'message').map((e) => e.text)).toEqual(['first']);
    expect(b.events.filter((e) => e.type === 'message').map((e) => e.text)).toEqual(['second']);
    expect(c.events.find((e) => e.type === 'message')!.text).toContain('run out of ideas');
    expect(agent.received).toEqual(['verdict A', 'verdict B']);
    await expect(agent.resume({ id: 'nope', kind: 'mock' }, { workdir, prompt: '' }, 'x', () => {})).rejects.toBeInstanceOf(AgentError);
  });

  it('applies patches with git apply and reports one that does not apply, without throwing', async () => {
    const workdir = await repo();
    const agent = new ScriptedAgent(script([{ type: 'patch', name: 'p1', diff: PATCH }, { type: 'patch', name: 'p2', diff: PATCH }]));
    const { events, onEvent } = collect();
    await agent.start({ workdir, prompt: '' }, onEvent);
    expect(readFileSync(join(workdir, 'a.txt'), 'utf8')).toBe('one\nTWO\nthree\n');
    expect(events.filter((e) => e.type === 'error')).toHaveLength(1);
    expect(events.find((e) => e.type === 'error')!.text).toContain('p2 does not apply');
  });

  it('skip conditions make patches idempotent', async () => {
    const workdir = await repo({ 'a.txt': 'one\nTWO\nthree\n' });
    const agent = new ScriptedAgent(script([{ type: 'patch', name: 'p', diff: PATCH, skip: { file: 'a.txt', contains: 'TWO', skipWhen: 'present' } }]));
    const { events, onEvent } = collect();
    await agent.start({ workdir, prompt: '' }, onEvent);
    expect(events.some((e) => e.type === 'error')).toBe(false);
    expect(events.some((e) => /already in the tree/.test(e.text ?? ''))).toBe(true);
  });

  it('an attempt whose patches are all in the tree is skipped within the turn; leading git steps still run', async () => {
    const workdir = await repo({ 'a.txt': 'one\nTWO\nthree\n' });
    const skip = { file: 'a.txt', contains: 'TWO', skipWhen: 'present' as const };
    const agent = new ScriptedAgent(script(
      [{ type: 'git', args: ['status'] }, { type: 'patch', diff: PATCH, skip }, { type: 'message', text: 'should not be played' }],
      [{ type: 'write', path: 'b.txt', content: 'b' }],
    ));
    const { events, onEvent } = collect();
    await agent.start({ workdir, prompt: '' }, onEvent);
    expect(events.map((e) => e.text).join('|')).not.toContain('should not be played');
    expect(readFileSync(join(workdir, 'b.txt'), 'utf8')).toBe('b'); // the next idea ran in the same turn
  });

  it('write / edit / delete, with edit requiring exactly one match and paths confined to the workdir', async () => {
    const workdir = await repo({ 'a.txt': 'x x\n', 'gone.txt': 'bye' });
    const agent = new ScriptedAgent(script([
      { type: 'write', path: 'sub/new.txt', content: 'hello' },
      { type: 'edit', path: 'a.txt', find: 'x', replace: 'y' }, // two matches -> error, no change
      { type: 'delete', path: 'gone.txt' },
    ]));
    const { events, onEvent } = collect();
    await agent.start({ workdir, prompt: '' }, onEvent);
    expect(readFileSync(join(workdir, 'sub/new.txt'), 'utf8')).toBe('hello');
    expect(readFileSync(join(workdir, 'a.txt'), 'utf8')).toBe('x x\n');
    expect(events.find((e) => e.type === 'error')!.text).toContain('found 2');
    const escape = new ScriptedAgent(script([{ type: 'write', path: '../evil.txt', content: 'x' }]));
    await expect(escape.start({ workdir, prompt: '' }, () => {})).rejects.toThrow(/escapes the workdir/);
  });

  it('usage events carry the scripted tokens and a believable cost', async () => {
    const workdir = await repo();
    const agent = new ScriptedAgent(script([{ type: 'usage', inputTokens: 80_000, outputTokens: 4_000 }, { type: 'usage', inputTokens: 1, outputTokens: 1, costUsd: 9 }]));
    const { events, onEvent } = collect();
    await agent.start({ workdir, prompt: '' }, onEvent);
    const usage = events.filter((e) => e.type === 'usage').map((e) => e.usage);
    expect(usage[0]).toEqual({ inputTokens: 80_000, outputTokens: 4_000, costUsd: priceUsage(80_000, 4_000) });
    expect(priceUsage(80_000, 4_000)).toBeCloseTo(0.3, 2);
    expect(usage[1]!.costUsd).toBe(9);
  });

  it('sleeps through the injected Clock, and abort interrupts a sleep', async () => {
    const workdir = await repo();
    const clock = new ManualClock(1000);
    const agent = new ScriptedAgent(script([{ type: 'sleep', ms: 5000 }, { type: 'message', text: 'woke' }]), { clock });
    const { events, onEvent } = collect();
    const p = agent.start({ workdir, prompt: '' }, onEvent);
    await new Promise((r) => setTimeout(r, 20));
    expect(events.some((e) => e.text === 'woke')).toBe(false);
    await clock.advance(4999);
    await new Promise((r) => setTimeout(r, 10));
    expect(events.some((e) => e.text === 'woke')).toBe(false);
    await clock.advance(1);
    await p;
    expect(events.find((e) => e.text === 'woke')!.at).toBe(6000);

    const ac = new AbortController();
    const slow = new ScriptedAgent(script([{ type: 'sleep', ms: 3_600_000 }]), { clock: new AcceleratedClock(1) });
    const q = slow.start({ workdir, prompt: '' }, () => {}, ac.signal);
    setTimeout(() => ac.abort(), 20);
    await expect(q).rejects.toBeInstanceOf(AgentAbortedError);
  });

  it('a fail step crashes the turn with an error event', async () => {
    const workdir = await repo();
    const agent = new ScriptedAgent(script([{ type: 'fail', message: 'boom' }]));
    const { events, onEvent } = collect();
    await expect(agent.start({ workdir, prompt: '' }, onEvent)).rejects.toThrow('boom');
    expect(events.at(-1)).toMatchObject({ type: 'error', text: 'boom' });
  });

  it('git steps run in the workdir', async () => {
    const workdir = await repo();
    const agent = new ScriptedAgent(script([{ type: 'git', args: ['no-such-subcommand'] }]));
    const { events, onEvent } = collect();
    await agent.start({ workdir, prompt: '' }, onEvent);
    expect(events.some((e) => e.type === 'error')).toBe(true);
  });
});
