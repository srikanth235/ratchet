import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentEvent, ArtifactStore, CloudCompute, RemoteCommand, RunnerHandle, RunnerMessage } from '@ratchet/core';
import {
  ArtifactEventSink, ArtifactInbox, AwsCliArtifactStore, AcceleratedClock, FakeSpawner, FsArtifactStore, LocalRunnerControl, ScriptedAgent, SsmRunnerControl,
  configKey, eventKey, inboxKey, readEvents, runnerPrefix, seqOf, type AgentScript,
} from '../src/index.js';
import { autoVerdicts, baseConfig, makeSandbox, until, type Sandbox } from './harness.js';

const cleanups: (() => void)[] = [];
afterEach(() => void cleanups.splice(0).forEach((c) => c()));

class MemoryStore implements ArtifactStore {
  readonly objects = new Map<string, string>();
  async put(key: string, body: string | Uint8Array): Promise<void> { this.objects.set(key, typeof body === 'string' ? body : Buffer.from(body).toString()); }
  async get(key: string): Promise<string | null> { return this.objects.get(key) ?? null; }
  async list(prefix: string): Promise<string[]> { return [...this.objects.keys()].filter((k) => k.startsWith(prefix)).sort(); }
}

class FakeCompute {
  readonly commands: { instanceId: string; command: RemoteCommand }[] = [];
  nextResult = { exitCode: 0, stdout: '', stderr: '' };
  fail = false;
  async runCommand(instanceId: string, command: RemoteCommand) {
    this.commands.push({ instanceId, command });
    if (this.fail) throw new Error('instance gone');
    return this.nextResult;
  }
}

const handle: RunnerHandle = { optimizationId: 'opt1', agentId: 'cc', instanceId: 'i-0abc' };
const ev = (text: string, at = 1): AgentEvent => ({ at, type: 'status', text });

describe('artifact layout', () => {
  it('uses the per-agent prefix IAM allows', () => {
    expect(runnerPrefix('opt1', 'cc')).toBe('artifacts/opt1/cc/');
    expect(eventKey('artifacts/opt1/cc/', 12)).toBe('artifacts/opt1/cc/events/00000012.jsonl');
    expect(inboxKey('artifacts/opt1/cc/', 3)).toBe('artifacts/opt1/cc/inbox/00000003.json');
    expect(seqOf('artifacts/opt1/cc/events/00000012.jsonl')).toBe(12);
    expect(seqOf('artifacts/opt1/cc/config.json')).toBeNaN();
  });
});

describe('SsmRunnerControl (in-memory CloudCompute + ArtifactStore)', () => {
  const make = () => {
    const store = new MemoryStore();
    const compute = new FakeCompute();
    const control = new SsmRunnerControl({ compute: compute as unknown as CloudCompute, store, runnerEnv: { RATCHET_ARTIFACTS_BUCKET: 'my-bucket', RATCHET_SECRET_REFS: "ANTHROPIC_API_KEY=/ratchet/opt1/cc/ANTHROPIC_API_KEY" } });
    return { store, compute, control };
  };
  const sb = { remote: 'x' } as Sandbox;

  it('start: writes config.json and launches ratchet-runner detached through SSM', async () => {
    const { store, compute, control } = make();
    const config = baseConfig(sb, { agentId: 'cc' });
    await control.start(handle, config);
    expect(JSON.parse((await store.get(configKey('artifacts/opt1/cc/')))!)).toMatchObject({ agentId: 'cc', branch: 'ratchet/a' });
    expect(compute.commands).toHaveLength(1);
    const { instanceId, command } = compute.commands[0]!;
    expect(instanceId).toBe('i-0abc');
    expect(command.script).toContain("setsid nohup env RATCHET_OPTIMIZATION_ID='opt1' RATCHET_AGENT_ID='cc' RATCHET_ARTIFACTS_BUCKET='my-bucket'");
    expect(command.script).toContain('ratchet-runner');
    expect(command.script).toContain('/var/lib/ratchet/opt1-cc.pid');
    expect(command.script).toContain("RATCHET_SECRET_REFS='ANTHROPIC_API_KEY=/ratchet/opt1/cc/ANTHROPIC_API_KEY'");
  });

  it('start: shell-quotes values and surfaces a failed launch', async () => {
    const { compute, control } = make();
    compute.nextResult = { exitCode: 1, stdout: '', stderr: 'runner already running' };
    await expect(control.start({ ...handle, agentId: "o'brien" }, baseConfig(sb))).rejects.toThrow(/runner already running/);
    expect(compute.commands[0]!.command.script).toContain(`RATCHET_AGENT_ID='o'\\''brien'`);
  });

  it('send: numbered inbox objects, continuing after existing ones', async () => {
    const { store, control } = make();
    await store.put(inboxKey('artifacts/opt1/cc/', 4), '{"type":"pause"}');
    await control.send(handle, { type: 'user', text: 'hello' });
    await control.send(handle, { type: 'stop' });
    expect([...store.objects.keys()].filter((k) => k.includes('/inbox/'))).toEqual([
      'artifacts/opt1/cc/inbox/00000004.json', 'artifacts/opt1/cc/inbox/00000005.json', 'artifacts/opt1/cc/inbox/00000006.json',
    ]);
    expect(JSON.parse(store.objects.get('artifacts/opt1/cc/inbox/00000005.json')!)).toEqual({ type: 'user', text: 'hello' });
  });

  it('poll: events after the cursor from JSONL batches, ignoring torn lines', async () => {
    const { store, control } = make();
    const p = 'artifacts/opt1/cc/';
    await store.put(eventKey(p, 1), `${JSON.stringify(ev('a'))}\n${JSON.stringify(ev('b'))}\n`);
    await store.put(eventKey(p, 2), `${JSON.stringify(ev('c'))}\n{"at":1,"typ`);
    const first = await control.poll(handle);
    expect(first.events.map((e) => e.text)).toEqual(['a', 'b', 'c']);
    expect(first.cursor).toBe('2');
    expect((await control.poll(handle, first.cursor)).events).toEqual([]);
    await store.put(eventKey(p, 3), `${JSON.stringify(ev('d'))}\n`);
    expect((await control.poll(handle, first.cursor)).events.map((e) => e.text)).toEqual(['d']);
    // other agents' events are not mixed in
    await store.put(eventKey('artifacts/opt1/cx/', 1), `${JSON.stringify(ev('other'))}\n`);
    expect((await control.poll(handle)).events).toHaveLength(4);
  });

  it('stop: durable stop message plus SIGTERM to the runner pid; tolerates an instance that is already gone', async () => {
    const { store, compute, control } = make();
    await control.stop(handle);
    expect(JSON.parse(store.objects.get('artifacts/opt1/cc/inbox/00000001.json')!)).toEqual({ type: 'stop' });
    expect(compute.commands[0]!.command.script).toContain('kill -TERM');
    compute.fail = true;
    await expect(control.stop(handle)).resolves.toBeUndefined();
  });

  it('round trip with the runner-side classes: sink -> poll, send -> ArtifactInbox', async () => {
    const { store, control } = make();
    const prefix = 'artifacts/opt1/cc/';
    const sink = new ArtifactEventSink(store, prefix, { flushIntervalMs: 10_000 });
    sink.emit(ev('one'));
    sink.emit(ev('two'));
    await sink.flush();
    sink.emit(ev('three'));
    await sink.close();
    expect([...store.objects.keys()].filter((k) => k.includes('/events/'))).toHaveLength(2);
    expect((await control.poll(handle)).events.map((e) => e.text)).toEqual(['one', 'two', 'three']);

    const inbox = new ArtifactInbox(store, prefix, new AcceleratedClock(1000), 5);
    await control.send(handle, { type: 'user', text: 'a' });
    await control.send(handle, { type: 'pause' });
    expect(await inbox.next()).toEqual({ type: 'user', text: 'a' });
    expect(await inbox.next()).toEqual({ type: 'pause' });
    const ac = new AbortController();
    const waiting = inbox.next(ac.signal);
    setTimeout(() => void control.send(handle, { type: 'resume' }), 30);
    expect(await waiting).toEqual({ type: 'resume' });
    const idle = inbox.next(ac.signal);
    ac.abort();
    expect(await idle).toBeNull();
  });

  it('a restarted event sink continues the numbering; a failed put is retried on the next flush', async () => {
    const store = new MemoryStore();
    const p = 'artifacts/o/a/';
    await store.put(eventKey(p, 7), '{}\n');
    const failOnce = { n: 0 };
    const flaky: ArtifactStore = { ...store, get: (k) => store.get(k), list: (x) => store.list(x), put: async (k, b) => { if (failOnce.n++ === 0) throw new Error('S3 503'); await store.put(k, b); } } as ArtifactStore;
    const sink = new ArtifactEventSink(flaky, p, { flushIntervalMs: 10_000 });
    sink.emit(ev('x'));
    await sink.flush();
    expect(sink.errors).toHaveLength(1);
    await sink.flush();
    await sink.close();
    expect([...store.objects.keys()]).toEqual([eventKey(p, 7), eventKey(p, 8)]);
    expect((await readEvents(store, p, '7')).events.map((e) => e.text)).toEqual(['x']);
  });
});

describe('AwsCliArtifactStore (fake aws CLI)', () => {
  it('put/get/list through `aws s3 cp` / `aws s3api list-objects-v2`', async () => {
    const fake = new FakeSpawner((call) =>
      call.args[0] === 's3api' ? { stdout: '["artifacts/o/a/events/00000001.jsonl","artifacts/o/a/events/00000002.jsonl"]\n' }
      : call.args.join(' ').includes('missing') ? { stderr: 'An error occurred (404) when calling the HeadObject operation: Key "missing" does not exist', exitCode: 1 }
      : call.args[3] === '-' ? { stdout: 'hello\n' }
      : {});
    const store = new AwsCliArtifactStore('bkt', fake.spawner, { PATH: '/bin' }, 'eu-west-1');
    await store.put('k/1', 'body');
    expect(fake.calls[0]!.args).toEqual(['s3', 'cp', '-', 's3://bkt/k/1', '--only-show-errors', '--region', 'eu-west-1']);
    expect(fake.calls[0]!.stdin).toBe('body');
    expect(await store.get('k/1')).toBe('hello\n');
    expect(await store.get('missing')).toBeNull();
    expect(await store.list('artifacts/o/a/events/')).toHaveLength(2);
    expect(fake.calls.at(-1)!.args).toContain('list-objects-v2');
  });
  it('list returns [] when the prefix is empty (aws prints null)', async () => {
    const fake = new FakeSpawner({ stdout: 'null\n' });
    expect(await new AwsCliArtifactStore('b', fake.spawner, {}).list('x/')).toEqual([]);
  });
});

describe('FsArtifactStore', () => {
  it('behaves like S3 for put/get/list-by-prefix', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'fsstore-'));
    cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
    const store = new FsArtifactStore(dir);
    await store.put('artifacts/o/a/events/00000001.jsonl', 'x');
    await store.put('artifacts/o/a/inbox/00000001.json', 'y');
    expect(await store.get('artifacts/o/a/events/00000001.jsonl')).toBe('x');
    expect(await store.get('nope')).toBeNull();
    expect(await store.list('artifacts/o/a/events/')).toEqual(['artifacts/o/a/events/00000001.jsonl']);
    expect(await store.list('artifacts/')).toHaveLength(2);
  });
});

describe('LocalRunnerControl', () => {
  const twoAttempts: AgentScript = { name: 'l', attempts: [
    { name: 'a', steps: [{ type: 'write', path: 'one.txt', content: '1' }, { type: 'usage', inputTokens: 50_000, outputTokens: 2_000 }] },
    { name: 'b', steps: [{ type: 'write', path: 'two.txt', content: '2' }] },
  ] };

  it('runs agents in separate clones with an in-memory inbox and cursor-based polling', async () => {
    const sb = makeSandbox(undefined, ['ratchet/a', 'ratchet/b']);
    cleanups.push(sb.cleanup);
    const control = new LocalRunnerControl({ rootDir: join(sb.dir, 'runners'), adapterFor: () => new ScriptedAgent(twoAttempts), loop: { maxIdleTurns: 1, retryDelayMs: 1 } });
    const ha: RunnerHandle = { optimizationId: 'opt1', agentId: 'a', instanceId: 'local-a' };
    const hb: RunnerHandle = { optimizationId: 'opt1', agentId: 'b', instanceId: 'local-b' };
    await control.start(ha, baseConfig(sb, { agentId: 'a', branch: 'ratchet/a' }));
    await control.start(hb, baseConfig(sb, { agentId: 'b', branch: 'ratchet/b' }));
    expect(control.workdirOf(ha)).not.toBe(control.workdirOf(hb));
    await expect(control.start(ha, baseConfig(sb))).rejects.toThrow(/already started/);

    const answer = (h: RunnerHandle) => autoVerdicts({ push: (m: RunnerMessage) => void control.send(h, m) });
    const seen: AgentEvent[] = [];
    let cursor: string | undefined;
    const answerA = answer(ha);
    const drain = async () => {
      const r = await control.poll(ha, cursor);
      cursor = r.cursor;
      r.events.forEach((e) => { seen.push(e); answerA(e); });
    };
    const answerB = answer(hb);
    let cursorB: string | undefined;
    const drainB = async () => {
      const r = await control.poll(hb, cursorB);
      cursorB = r.cursor;
      r.events.forEach(answerB);
    };
    const done = Promise.all([control.result(ha), control.result(hb)]);
    let finished = false;
    void done.then(() => (finished = true));
    while (!finished) { await drain(); await drainB(); await new Promise((r) => setTimeout(r, 10)); }
    await drain();
    const [ra, rb] = await done;
    expect(ra).toMatchObject({ reason: 'idle', candidates: 2 });
    expect(rb).toMatchObject({ reason: 'idle', candidates: 2 });
    // cursors never re-deliver
    expect(seen.filter((e) => e.type === 'commit')).toHaveLength(2);
    expect((await control.poll(ha, cursor)).events).toEqual([]);
    expect((await control.poll(ha)).events.length).toBe(seen.length); // no cursor = from the start
  });

  it('stop ends a running agent promptly', async () => {
    const sb = makeSandbox();
    cleanups.push(sb.cleanup);
    const slow: AgentScript = { name: 's', attempts: [{ name: 'a', steps: [{ type: 'sleep', ms: 60_000 }] }] };
    const control = new LocalRunnerControl({ rootDir: join(sb.dir, 'runners'), adapterFor: () => new ScriptedAgent(slow) });
    const h: RunnerHandle = { optimizationId: 'opt1', agentId: 'a', instanceId: 'local-a' };
    await control.start(h, baseConfig(sb));
    await until(async () => (await control.poll(h)).events.some((e) => e.text === 'agent turn started'));
    const t0 = Date.now();
    await control.stop(h);
    expect(Date.now() - t0).toBeLessThan(3000);
    expect((await control.result(h)).reason).toBe('stopped');
    await expect(control.send({ ...h, agentId: 'zzz' }, { type: 'pause' })).rejects.toThrow(/unknown runner/);
  });
});
