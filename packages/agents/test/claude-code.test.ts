import { describe, expect, it } from 'vitest';
import {
  AgentAbortedError,
  AgentError,
  ClaudeCodeAdapter,
  ClaudeStreamParser,
  FakeSpawner,
  type FakeProcessSpec,
} from '../src/index.js';
import { collect, fixture } from './helpers.js';

const SID = '5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11';
const spec = { workdir: '/work/agent-1', prompt: 'make it faster' };

function parse(name: string, chunkSize?: number) {
  const parser = new ClaudeStreamParser(() => 1);
  const text = fixture(name);
  const events =
    chunkSize === undefined
      ? parser.push(text)
      : (text.match(new RegExp(`[\\s\\S]{1,${chunkSize}}`, 'g')) ?? []).flatMap((c) => parser.push(c));
  return { parser, events: [...events, ...parser.end()] };
}

describe('ClaudeStreamParser', () => {
  it('maps a full transcript to status/message/tool/usage events', () => {
    const { parser, events } = parse('claude-success.jsonl');
    expect(parser.sessionId).toBe(SID);
    expect(parser.model).toBe('claude-sonnet-5-5');
    expect(events.map((e) => e.type)).toEqual(['status', 'message', 'tool', 'tool', 'tool', 'tool', 'message', 'usage']);
    expect(events[0]!.data).toMatchObject({ sessionId: SID, apiKeySource: 'ANTHROPIC_API_KEY', tools: 8 });
    expect(events[2]).toMatchObject({ text: 'Bash: node --cpu-prof dist/src/main.js', data: { phase: 'call', name: 'Bash' } });
    expect(events[3]).toMatchObject({ data: { phase: 'result', id: 'toolu_01X', isError: false } });
    expect(events[4]!.text).toContain('Edit: /work/agent-1/src/search.ts'); // thinking block skipped
    expect(events[5]!.text).toContain('has been updated'); // array-form tool_result content
    const usage = events[7]!;
    // input = input + cache creation + cache read, from the result line only (no double counting)
    expect(usage.usage).toEqual({ inputTokens: 16 + 12385 + 36131, outputTokens: 308, costUsd: 0.0841 });
    expect(parser.result).toMatchObject({ subtype: 'success', isError: false, numTurns: 4 });
  });

  it('is identical however the stream is chunked, including 1-byte chunks', () => {
    const whole = parse('claude-success.jsonl').events;
    for (const size of [1, 7, 64, 1000]) expect(parse('claude-success.jsonl', size).events).toEqual(whole);
  });

  it('holds a partial last line until it completes, and parses an unterminated final line on end()', () => {
    const parser = new ClaudeStreamParser(() => 1);
    const line = JSON.stringify({ type: 'assistant', message: { content: [{ type: 'text', text: 'hi' }] } });
    expect(parser.push(line.slice(0, 20))).toEqual([]);
    expect(parser.push(line.slice(20))).toEqual([]); // no newline yet
    expect(parser.end()).toEqual([{ at: 1, type: 'message', text: 'hi' }]);
  });

  it('skips unknown event/block types and garbage lines without throwing', () => {
    const { parser, events } = parse('claude-noisy.jsonl');
    expect(events.map((e) => e.type)).toEqual(['status', 'message', 'tool', 'usage']);
    expect(parser.malformedLines).toBe(3); // stdout noise, the array, the object without type
    expect([...parser.unknownTypes].sort()).toEqual([
      'block:server_tool_use',
      'future_event_kind',
      'rate_limit_event',
      'stream_event',
      'system:hook_started',
    ]);
    expect(parser.sessionId).toBe('sess-noisy');
  });

  it('reports error results as error events and keeps the usage', () => {
    const { parser, events } = parse('claude-error-execution.jsonl');
    expect(events.map((e) => e.type)).toEqual(['status', 'usage', 'error']);
    expect(events[2]!.text).toContain('authentication_error');
    expect(parser.result).toMatchObject({ isError: true, subtype: 'error_during_execution' });
  });

  it('treats error_max_turns as an error-subtype result', () => {
    const { parser } = parse('claude-error-max-turns.jsonl');
    expect(parser.result).toMatchObject({ isError: true, subtype: 'error_max_turns' });
  });
});

describe('ClaudeCodeAdapter', () => {
  const adapterFor = (plan: FakeProcessSpec | ((c: never, i: number) => FakeProcessSpec), env = { ANTHROPIC_API_KEY: 'sk-test', PATH: '/bin' }) => {
    const fake = new FakeSpawner(plan as never);
    return { fake, adapter: new ClaudeCodeAdapter({ spawner: fake.spawner, env, killGraceMs: 20 }) };
  };

  it('start: builds the command line, pipes the prompt on stdin and returns the session', async () => {
    const { fake, adapter } = adapterFor({ stdout: fixture('claude-success.jsonl'), chunkSize: 50 });
    const { events, onEvent } = collect();
    const session = await adapter.start({ ...spec, model: 'claude-opus-5-5', maxTurns: 40, env: { FOO: 'bar' } }, onEvent);
    expect(session).toEqual({ id: SID, kind: 'claude-code' });
    const call = fake.calls[0]!;
    expect(call.command).toBe('claude');
    expect(call.args).toEqual([
      '-p', '--output-format', 'stream-json', '--verbose',
      '--permission-mode', 'acceptEdits',
      '--allowedTools', 'Bash,Read,Edit,Write,Glob,Grep',
      '--model', 'claude-opus-5-5',
      '--max-turns', '40',
    ]);
    expect(call.stdin).toBe('make it faster');
    expect(call.options.cwd).toBe('/work/agent-1');
    expect(call.options.env).toMatchObject({ ANTHROPIC_API_KEY: 'sk-test', FOO: 'bar' });
    expect(events.some((e) => e.type === 'usage')).toBe(true);
  });

  it('honours binary path, permission mode, tool lists and prompt-as-argument', async () => {
    const fake = new FakeSpawner({ stdout: fixture('claude-success.jsonl') });
    const adapter = new ClaudeCodeAdapter({
      spawner: fake.spawner, env: {}, apiKey: 'sk-from-option', binary: '/opt/claude/bin/claude',
      permissionMode: 'bypassPermissions', allowedTools: ['Bash(git:*)', 'Edit'], disallowedTools: ['WebFetch'],
      promptVia: 'arg', extraArgs: ['--add-dir', '/data'],
    });
    await adapter.start(spec, () => {});
    const call = fake.calls[0]!;
    expect(call.command).toBe('/opt/claude/bin/claude');
    expect(call.args).toEqual([
      '-p', '--output-format', 'stream-json', '--verbose', '--permission-mode', 'bypassPermissions',
      '--allowedTools', 'Bash(git:*),Edit', '--disallowedTools', 'WebFetch', '--add-dir', '/data', '--', 'make it faster',
    ]);
    expect(call.stdin).toBe('');
    expect(call.options.env['ANTHROPIC_API_KEY']).toBe('sk-from-option');
  });

  it('resume: passes --resume <id>, sends the message and keeps the session id', async () => {
    const { fake, adapter } = adapterFor({ stdout: fixture('claude-resume.jsonl') });
    const session = { id: SID, kind: 'claude-code' as const };
    const { events, onEvent } = collect();
    await adapter.resume(session, spec, 'verdict: verified', onEvent);
    expect(fake.calls[0]!.args).toContain('--resume');
    expect(fake.calls[0]!.args[fake.calls[0]!.args.indexOf('--resume') + 1]).toBe(SID);
    expect(fake.calls[0]!.stdin).toBe('verdict: verified');
    expect(session.id).toBe(SID);
    expect(events.find((e) => e.type === 'usage')!.usage).toEqual({ inputTokens: 12606, outputTokens: 120, costUsd: 0.0312 });
  });

  it('resume: follows a new session id if the CLI forks one', async () => {
    const { adapter } = adapterFor({ stdout: fixture('claude-resume-new-id.jsonl') });
    const session = { id: SID, kind: 'claude-code' as const };
    await adapter.resume(session, spec, 'go on', () => {});
    expect(session.id).toBe('0b9d4c20-aaaa-bbbb-cccc-111122223333');
  });

  it('resume of an unknown session: non-zero exit with stderr becomes an AgentError and an error event', async () => {
    const { adapter } = adapterFor({ stdout: '', stderr: 'No conversation found with session ID: nope\n', exitCode: 1 });
    const { events, onEvent } = collect();
    const err = await adapter.resume({ id: 'nope', kind: 'claude-code' }, spec, 'x', onEvent).catch((e) => e);
    expect(err).toBeInstanceOf(AgentError);
    expect(err.message).toContain('No conversation found');
    expect(err.detail).toMatchObject({ exitCode: 1 });
    expect(events.at(-1)).toMatchObject({ type: 'error' });
  });

  it('an is_error result fails the turn (error event emitted once)', async () => {
    const { adapter } = adapterFor({ stdout: fixture('claude-error-execution.jsonl'), exitCode: 1 });
    const { events, onEvent } = collect();
    await expect(adapter.start(spec, onEvent)).rejects.toThrow(/authentication_error/);
    expect(events.filter((e) => e.type === 'error')).toHaveLength(1);
  });

  it('error_max_turns ends the turn normally so the runner can carry on', async () => {
    const { adapter } = adapterFor({ stdout: fixture('claude-error-max-turns.jsonl') });
    const session = await adapter.start(spec, () => {});
    expect(session.id).toBe('sess-turns');
  });

  it('a stream that ends without a result line is an error, even with exit 0', async () => {
    const { adapter } = adapterFor({ stdout: fixture('claude-success.jsonl').split('\n').slice(0, 3).join('\n') + '\n{"type":"assis' });
    await expect(adapter.start(spec, () => {})).rejects.toThrow(/without a result/);
  });

  it('a missing binary is reported, not thrown raw', async () => {
    const { adapter } = adapterFor({ spawnError: 'spawn claude ENOENT' });
    await expect(adapter.start(spec, () => {})).rejects.toThrow(/cannot run 'claude'.*ENOENT/);
  });

  it('abort: signals the process group (SIGTERM) and rejects with AgentAbortedError', async () => {
    const { fake, adapter } = adapterFor({ stdout: fixture('claude-success.jsonl').split('\n').slice(0, 2).join('\n') + '\n', hang: true });
    const ac = new AbortController();
    const { events, onEvent } = collect();
    const p = adapter.start(spec, (e) => {
      onEvent(e);
      if (e.type === 'status') ac.abort(); // abort once the session has started
    }, ac.signal);
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(AgentAbortedError);
    expect(err.session).toMatchObject({ id: SID });
    expect(fake.calls[0]!.signals[0]).toBe('SIGTERM');
    expect(events.at(-1)).toMatchObject({ type: 'status', text: 'claude-code turn aborted' });
  });

  it('an already-aborted signal never lets the turn proceed', async () => {
    const { fake, adapter } = adapterFor({ hang: true });
    const ac = new AbortController();
    ac.abort();
    await expect(adapter.start(spec, () => {}, ac.signal)).rejects.toBeInstanceOf(AgentAbortedError);
    expect(fake.calls[0]!.signals).toContain('SIGTERM');
  });

  describe('preflight', () => {
    it('ok with the binary present and a key', async () => {
      const { adapter } = adapterFor({ stdout: '2.1.0 (Claude Code)\n' });
      expect(await adapter.preflight()).toEqual({ ok: true, problems: [] });
    });
    it('reports a missing key and a missing binary', async () => {
      const { adapter } = adapterFor({ spawnError: 'ENOENT' }, {} as never);
      const r = await adapter.preflight();
      expect(r.ok).toBe(false);
      expect(r.problems).toEqual([expect.stringContaining("cannot run 'claude'"), 'ANTHROPIC_API_KEY is not set']);
    });
    it('a subscription login can be allowed explicitly', async () => {
      const fake = new FakeSpawner({ stdout: '2.1.0\n' });
      const adapter = new ClaudeCodeAdapter({ spawner: fake.spawner, env: {}, allowSubscriptionAuth: true });
      expect((await adapter.preflight()).ok).toBe(true);
    });
  });
});
