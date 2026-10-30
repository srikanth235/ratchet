import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentAdapter, CloudCompute, RunnerHandle } from '@ratchet/core';
import {
  AcceleratedClock, FakeSpawner, FsArtifactStore, ScriptedAgent, SsmRunnerControl, doneKey, execGit, gitAuthEnv, parseSecretRefs, runnerMain, runnerPrefix, validateConfig,
  type AgentScript, type GitRunner,
} from '../src/index.js';
import { autoVerdicts, baseConfig, makeSandbox, until, type Sandbox } from './harness.js';

const cleanups: (() => void)[] = [];
afterEach(() => void cleanups.splice(0).forEach((c) => c()));

const handle: RunnerHandle = { optimizationId: 'opt1', agentId: 'cc', instanceId: 'i-1' };
const script: AgentScript = { name: 'main', attempts: [
  { name: 'a', steps: [{ type: 'write', path: 'src/app.txt', content: 'faster\n' }, { type: 'usage', inputTokens: 30_000, outputTokens: 1_500 }] },
  { name: 'b', steps: [{ type: 'write', path: 'src/more.txt', content: 'more\n' }] },
] };

function setup() {
  const sb = makeSandbox(undefined, ['ratchet/cc']);
  const work = mkdtempSync(join(tmpdir(), 'runner-main-'));
  const artifacts = join(work, 'artifacts');
  cleanups.push(() => { sb.cleanup(); rmSync(work, { recursive: true, force: true }); });
  const store = new FsArtifactStore(artifacts);
  const scriptFile = join(work, 'script.json');
  writeFileSync(scriptFile, JSON.stringify(script));
  const config = baseConfig(sb, { optimizationId: 'opt1', agentId: 'cc', branch: 'ratchet/cc' });
  const env = {
    RATCHET_OPTIMIZATION_ID: 'opt1', RATCHET_AGENT_ID: 'cc', RATCHET_ARTIFACT_DIR: artifacts, RATCHET_WORKDIR: join(work, 'wd', 'cc'),
    RATCHET_MOCK_SCRIPT: scriptFile, RATCHET_EVENT_FLUSH_MS: '20', RATCHET_INBOX_POLL_MS: '10', RATCHET_MAX_IDLE_TURNS: '1', PATH: process.env['PATH'],
  };
  return { sb, work, artifacts, store, config, env, scriptFile };
}

/** The engine side of the live path: SsmRunnerControl over the same store, answering commits with verdicts. */
async function engineLoop(control: SsmRunnerControl, until_: () => boolean): Promise<void> {
  const answer = autoVerdicts({ push: (m) => void control.send(handle, m) });
  let cursor: string | undefined;
  while (!until_()) {
    const r = await control.poll(handle, cursor);
    cursor = r.cursor;
    r.events.forEach(answer);
    await new Promise((res) => setTimeout(res, 15));
  }
}

describe('runnerMain (what the `ratchet-runner` bin runs)', () => {
  it('reads config from the artifact store, runs the loop, streams events, answers inbox messages, writes done.json', async () => {
    const { store, config, env, sb } = setup();
    const control = new SsmRunnerControl({ compute: { runCommand: async () => ({ exitCode: 0, stdout: '', stderr: '' }) } as unknown as CloudCompute, store });
    await control.start(handle, config); // writes config.json (launch itself is faked)
    let finished = false;
    const logs: string[] = [];
    const run = runnerMain({ env, log: (l) => logs.push(l) }).then((code) => ((finished = true), code));
    await engineLoop(control, () => finished);
    expect(await run).toBe(0);

    const { events } = await control.poll(handle);
    expect(events.filter((e) => e.type === 'commit')).toHaveLength(2);
    expect(events.filter((e) => e.type === 'verdict')).toHaveLength(2);
    expect(events.at(-1)).toMatchObject({ type: 'status', text: 'runner finished: idle' });
    const done = JSON.parse((await store.get(doneKey(runnerPrefix('opt1', 'cc'))))!);
    expect(done).toMatchObject({ reason: 'idle', candidates: 2 });
    expect(logs.at(-1)).toMatch(/finished: idle \(2 candidates/);
    // candidates really landed on the agent branch
    await execGit(['fetch', '--quiet', 'origin'], { cwd: sb.engine });
    const out = await execGit(['log', '--format=%B', 'main..origin/ratchet/cc'], { cwd: sb.engine });
    expect(out.stdout.match(/Ratchet-Candidate: true/g)).toHaveLength(2);
  });

  it('fetches SSM secrets for the agent (never the git token) and authenticates git with the token', async () => {
    const { store, config, env } = setup();
    const httpsUrl = 'https://github.com/acme/repo.git';
    await store.put(`${runnerPrefix('opt1', 'cc')}config.json`, JSON.stringify({ ...config, repoUrl: httpsUrl }));
    const aws = new FakeSpawner((call) => ({ stdout: call.args.includes('/ratchet/opt1/cc/ANTHROPIC_API_KEY') ? 'sk-ant-xyz\n' : 'ghs_token123\n' }));
    let agentEnv: Record<string, string> = {};
    const gitEnvs: (Record<string, string> | undefined)[] = [];
    // pretend github.com is the local sandbox remote, but record the env the clone ran with
    const git: GitRunner = (args, opts) => (gitEnvs.push(opts.env), execGit(args.map((a) => (a === httpsUrl ? config.repoUrl : a)), opts));
    const adapterFactory = (_c: unknown, e: Record<string, string>): AgentAdapter => ((agentEnv = e), new ScriptedAgent({ name: 'x', attempts: [] }));
    const code = await runnerMain({
      env: { ...env, RATCHET_SECRET_REFS: 'ANTHROPIC_API_KEY=/ratchet/opt1/cc/ANTHROPIC_API_KEY,GITHUB_TOKEN=/ratchet/opt1/cc/GITHUB_TOKEN', AWS_REGION: 'us-east-1', SECRET_OTHER: 'leak-me-not', HTTPS_PROXY: 'http://10.42.3.10:3128', NO_PROXY: '169.254.169.254', NODE_USE_ENV_PROXY: '1' },
      spawner: aws.spawner, git, adapterFactory, log: () => {},
    });
    expect(code).toBe(1); // empty script -> agent-unavailable
    expect(aws.calls.map((c) => c.args.slice(0, 2).join(' '))).toEqual(['ssm get-parameter', 'ssm get-parameter']);
    expect(aws.calls[0]!.args).toContain('--with-decryption');
    expect(agentEnv['ANTHROPIC_API_KEY']).toBe('sk-ant-xyz');
    expect(agentEnv['GITHUB_TOKEN']).toBeUndefined();
    expect(agentEnv['SECRET_OTHER']).toBeUndefined(); // only a small passthrough list reaches the agent
    expect(agentEnv['AWS_REGION']).toBe('us-east-1');
    // With the sandbox egress proxy on, the agent CLIs reach the model APIs only through it.
    expect(agentEnv['HTTPS_PROXY']).toBe('http://10.42.3.10:3128');
    expect(agentEnv['NO_PROXY']).toBe('169.254.169.254');
    expect(agentEnv['NODE_USE_ENV_PROXY']).toBe('1');
    const header = gitEnvs.find((e) => e?.['GIT_CONFIG_KEY_0'])!;
    expect(header['GIT_CONFIG_KEY_0']).toBe('http.https://github.com/.extraheader');
    expect(header['GIT_CONFIG_VALUE_0']).toBe(`AUTHORIZATION: basic ${Buffer.from('x-access-token:ghs_token123').toString('base64')}`);
  });

  it('validates its inputs with actionable errors', async () => {
    const { env } = setup();
    await expect(runnerMain({ env: { ...env, RATCHET_OPTIMIZATION_ID: undefined } })).rejects.toThrow('RATCHET_OPTIMIZATION_ID is required');
    await expect(runnerMain({ env: { ...env, RATCHET_ARTIFACT_DIR: undefined } })).rejects.toThrow(/RATCHET_ARTIFACTS_BUCKET/);
    await expect(runnerMain({ env })).rejects.toThrow(/no runner config/);
    await expect(runnerMain({ env: { ...env, RATCHET_RUNNER_CONFIG: '{"agentId":"cc"}' } })).rejects.toThrow(/missing: optimizationId/);
    await expect(runnerMain({ env: { ...env, RATCHET_RUNNER_CONFIG: JSON.stringify({ ...baseConfig({ remote: 'x' } as Sandbox), agentId: 'other' }) } })).rejects.toThrow(/does not match/);
    expect(() => parseSecretRefs('NOEQUALS')).toThrow(/bad RATCHET_SECRET_REFS/);
    expect(parseSecretRefs(' A=/x/a , B=/y/b=c ')).toEqual({ A: '/x/a', B: '/y/b=c' });
    expect(() => validateConfig(null)).toThrow(/missing/);
  });

  it('gitAuthEnv scopes an http extraheader to the repo host, via env (not argv or disk)', () => {
    const env = gitAuthEnv('https://github.com/acme/repo.git', 'tok');
    expect(env['GIT_CONFIG_KEY_0']).toBe('http.https://github.com/.extraheader');
    expect(env['GIT_CONFIG_VALUE_0']).toBe(`AUTHORIZATION: basic ${Buffer.from('x-access-token:tok').toString('base64')}`);
    expect(gitAuthEnv('/local/path.git', 'tok')).toEqual({});
  });
});

const BIN = fileURLToPath(new URL('../dist/bin/ratchet-runner.js', import.meta.url));

describe.skipIf(!existsSync(BIN))('ratchet-runner binary (needs `pnpm build`)', () => {
  const run = (env: Record<string, string | undefined>) =>
    spawn(process.execPath, [BIN], { env: env as NodeJS.ProcessEnv, stdio: ['ignore', 'pipe', 'pipe'] });

  it('exits 0 and writes done.json after a full run', async () => {
    const { store, config, env } = setup();
    const control = new SsmRunnerControl({ compute: { runCommand: async () => ({ exitCode: 0, stdout: '', stderr: '' }) } as unknown as CloudCompute, store });
    await control.start(handle, config);
    const child = run(env);
    let stderr = '';
    child.stderr.on('data', (d) => (stderr += d));
    const exited = new Promise<number | null>((r) => child.on('exit', r));
    let finished = false;
    void exited.then(() => (finished = true));
    await engineLoop(control, () => finished);
    expect(await exited, stderr).toBe(0);
    expect(JSON.parse((await store.get(doneKey(runnerPrefix('opt1', 'cc'))))!).candidates).toBe(2);
  }, 60_000);

  it('SIGTERM aborts the agent and exits cleanly, recording why', async () => {
    const { store, config, env, work } = setup();
    const slow = join(work, 'slow.json');
    writeFileSync(slow, JSON.stringify({ name: 'slow', attempts: [{ name: 'a', steps: [{ type: 'sleep', ms: 600_000 }] }] }));
    const control = new SsmRunnerControl({ compute: { runCommand: async () => ({ exitCode: 0, stdout: '', stderr: '' }) } as unknown as CloudCompute, store });
    await control.start(handle, config);
    const child = run({ ...env, RATCHET_MOCK_SCRIPT: slow });
    const exited = new Promise<number | null>((r) => child.on('exit', r));
    await until(async () => (await control.poll(handle)).events.some((e) => e.text === 'agent turn started'), 15_000);
    child.kill('SIGTERM');
    expect(await exited).toBe(0);
    expect(JSON.parse((await store.get(doneKey(runnerPrefix('opt1', 'cc'))))!).reason).toBe('aborted');
    void AcceleratedClock;
  }, 60_000);

  it('exits 2 with a message when misconfigured', async () => {
    const child = run({ PATH: process.env['PATH'] });
    let stderr = '';
    child.stderr.on('data', (d) => (stderr += d));
    expect(await new Promise((r) => child.on('exit', r))).toBe(2);
    expect(stderr).toContain('RATCHET_OPTIMIZATION_ID is required');
  });
});
