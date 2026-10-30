import { describe, expect, it } from 'vitest';
import { AgentAbortedError, AgentError, CodexAdapter, CodexStreamParser, FakeSpawner, type FakeProcessSpec } from '../src/index.js';
import { collect, fixture } from './helpers.js';

const TID = '0199a213-81c0-7800-8aa1-bbab2a035a53';
const spec = { workdir: '/work/agent-2', prompt: 'make it faster' };

function parse(name: string, chunkSize?: number, pricing?: ConstructorParameters<typeof CodexStreamParser>[1]) {
  const parser = new CodexStreamParser(() => 1, pricing);
  const text = fixture(name);
  const events =
    chunkSize === undefined
      ? parser.push(text)
      : (text.match(new RegExp(`[\\s\\S]{1,${chunkSize}}`, 'g')) ?? []).flatMap((c) => parser.push(c));
  return { parser, events: [...events, ...parser.end()] };
}

describe('CodexStreamParser', () => {
  it('maps items to message/tool events and turn.completed to usage', () => {
    const { parser, events } = parse('codex-success.jsonl');
    expect(parser.threadId).toBe(TID);
    expect(events.map((e) => `${e.type}${e.data?.['phase'] ? `:${e.data['phase']}` : ''}`)).toEqual([
      'status', 'tool:call', 'tool:result', 'message', 'tool:call', 'tool:call', 'tool:result', 'message', 'usage',
    ]);
    expect(events[1]!.text).toBe("command: bash -lc 'node --cpu-prof dist/src/main.js'");
    expect(events[2]!.data).toMatchObject({ exitCode: 0, isError: false });
    expect(events[4]!.text).toBe('apply_patch: /work/agent-2/src/filter.ts');
    expect(events[6]!.data).toMatchObject({ exitCode: 1, isError: true });
    expect(events[8]!.usage).toEqual({ inputTokens: 24763, outputTokens: 1122 }); // no cost without a price book
    expect(events[8]!.data).toMatchObject({ cachedInputTokens: 24448 });
  });

  it('estimates cost when pricing is given (cached input billed at the cached rate)', () => {
    const { events } = parse('codex-success.jsonl', undefined, { inputPerM: 1.25, cachedInputPerM: 0.125, outputPerM: 10 });
    const cost = events.find((e) => e.type === 'usage')!.usage!.costUsd!;
    expect(cost).toBeCloseTo(((24763 - 24448) * 1.25 + 24448 * 0.125 + 1122 * 10) / 1e6, 10);
  });

  it('is chunking-independent', () => {
    const whole = parse('codex-success.jsonl').events;
    for (const size of [1, 13, 100]) expect(parse('codex-success.jsonl', size).events).toEqual(whole);
  });

  it('tolerates unknown events, items and stdout noise; non-fatal errors do not fail the turn', () => {
    const { parser, events } = parse('codex-noisy.jsonl');
    expect(events.map((e) => e.type)).toEqual(['status', 'error', 'tool', 'message', 'usage']);
    expect(events[1]!.data).toEqual({ fatal: false });
    expect(parser.malformedLines).toBe(1);
    expect([...parser.unknownTypes].sort()).toEqual(['item:hologram', 'something.new']);
    expect(parser.turn).toMatchObject({ completed: true, failed: false });
  });

  it('turn.failed marks the turn failed with the vendor message', () => {
    const { parser } = parse('codex-failed.jsonl');
    expect(parser.turn).toMatchObject({ failed: true, completed: false });
    expect(parser.turn.error).toContain('429');
  });
});

describe('CodexAdapter', () => {
  const adapterFor = (plan: FakeProcessSpec, env: Record<string, string> = { OPENAI_API_KEY: 'sk-oai', PATH: '/bin' }) => {
    const fake = new FakeSpawner(plan);
    return { fake, adapter: new CodexAdapter({ spawner: fake.spawner, env, killGraceMs: 20 }) };
  };

  it('start: `codex exec --json`, prompt on stdin via "-", returns the thread id', async () => {
    const { fake, adapter } = adapterFor({ stdout: fixture('codex-success.jsonl'), chunkSize: 40 });
    const { events, onEvent } = collect();
    const session = await adapter.start({ ...spec, model: 'gpt-5-codex' }, onEvent);
    expect(session).toEqual({ id: TID, kind: 'codex' });
    const call = fake.calls[0]!;
    expect(call.command).toBe('codex');
    expect(call.args).toEqual([
      'exec', '--json', '--skip-git-repo-check', '--model', 'gpt-5-codex',
      '-c', 'sandbox_mode="workspace-write"', '-c', 'sandbox_workspace_write.network_access=true', '-',
    ]);
    expect(call.stdin).toBe('make it faster');
    expect(call.options.cwd).toBe('/work/agent-2');
    expect(call.options.env).toMatchObject({ OPENAI_API_KEY: 'sk-oai', CODEX_API_KEY: 'sk-oai' });
    expect(events.at(-1)!.type).toBe('usage');
  });

  it('resume: `codex exec resume <thread> --json ...`', async () => {
    const { fake, adapter } = adapterFor({ stdout: fixture('codex-resume.jsonl') });
    const session = { id: TID, kind: 'codex' as const };
    const { events, onEvent } = collect();
    await adapter.resume(session, spec, 'verdict: verified', onEvent);
    expect(fake.calls[0]!.args.slice(0, 4)).toEqual(['exec', 'resume', TID, '--json']);
    expect(fake.calls[0]!.args.at(-1)).toBe('-');
    expect(fake.calls[0]!.stdin).toBe('verdict: verified');
    expect(events.find((e) => e.type === 'message')!.text).toContain('Verdict verified');
  });

  it('sandbox / prompt-as-argument / extra overrides are configurable', async () => {
    const fake = new FakeSpawner({ stdout: fixture('codex-success.jsonl') });
    const adapter = new CodexAdapter({
      spawner: fake.spawner, env: {}, apiKey: 'k', sandbox: 'danger-full-access', promptVia: 'arg',
      configOverrides: ['model_reasoning_effort="high"'], binary: '/usr/local/bin/codex',
    });
    await adapter.start(spec, () => {});
    expect(fake.calls[0]!.command).toBe('/usr/local/bin/codex');
    expect(fake.calls[0]!.args).toEqual([
      'exec', '--json', '--skip-git-repo-check', '-c', 'sandbox_mode="danger-full-access"',
      '-c', 'model_reasoning_effort="high"', '--', 'make it faster',
    ]);
    expect(fake.calls[0]!.options.env['OPENAI_API_KEY']).toBe('k');
  });

  it('turn.failed becomes an AgentError carrying the vendor message', async () => {
    const { adapter } = adapterFor({ stdout: fixture('codex-failed.jsonl'), exitCode: 1 });
    const { events, onEvent } = collect();
    await expect(adapter.start(spec, onEvent)).rejects.toThrow(/429/);
    expect(events.filter((e) => e.type === 'error').length).toBeGreaterThan(0);
  });

  it('non-zero exit without turn.completed carries stderr', async () => {
    const { adapter } = adapterFor({ stdout: '', stderr: 'error: unknown session\n', exitCode: 2 });
    const err = await adapter.resume({ id: 'x', kind: 'codex' }, spec, 'm', () => {}).catch((e) => e);
    expect(err).toBeInstanceOf(AgentError);
    expect(err.message).toContain('unknown session');
  });

  it('abort kills the process group and rejects with AgentAbortedError', async () => {
    const { fake, adapter } = adapterFor({ stdout: fixture('codex-success.jsonl').split('\n').slice(0, 1).join('\n') + '\n', hang: true });
    const ac = new AbortController();
    const err = await adapter.start(spec, (e) => e.type === 'status' && ac.abort(), ac.signal).catch((e) => e);
    expect(err).toBeInstanceOf(AgentAbortedError);
    expect(fake.calls[0]!.signals).toEqual(['SIGTERM']);
  });

  it('preflight: key and binary', async () => {
    expect((await adapterFor({ stdout: 'codex-cli 0.50.0\n' }).adapter.preflight()).ok).toBe(true);
    const bad = await adapterFor({ spawnError: 'ENOENT' }, {}).adapter.preflight();
    expect(bad.problems).toHaveLength(2);
  });
});
