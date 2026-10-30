import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openLedger, type Ledger } from '@ratchet/core';
import { createLiveRuntime, type AwsSandboxOutputs, type LiveOptions } from '@ratchet/runtime';
import { afterEach, describe, expect, it } from 'vitest';
import type { ConnectionResultVM, ConnectionTarget } from '../shared/types.js';
import { IPC } from '../shared/channels.js';
import { createEngineForApp } from './engine.js';
import { registerIpc, type IpcMainLike } from './ipc.js';
import { effectivePaths, liveReadiness, defaultLivePaths } from './live-config.js';
import { createService, type ConnectionTesterLike, type RatchetService } from './service.js';
import { SettingsStore } from './settings.js';

const cipher = { isAvailable: () => true, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };

const GH_TOKEN = 'ghp_TOPSECRETTOKENVALUE1234567890abcd';
const ANTHROPIC_KEY = 'sk-ant-api03-TOPSECRETANTHROPICKEY';
const OPENAI_KEY = 'sk-proj-TOPSECRETOPENAIKEYVALUE';
const SECRETS = [GH_TOKEN, ANTHROPIC_KEY, OPENAI_KEY];

const ACCT = '123456789012';
const identity = (n: string) => ({
  role_arn: `arn:aws:iam::${ACCT}:role/ratchet/bench/${n}`,
  role_name: n,
  instance_profile_arn: `arn:aws:iam::${ACCT}:instance-profile/ratchet/bench/${n}`,
  instance_profile_name: n,
});
const OUTPUTS_JSON = JSON.stringify({
  artifacts_bucket: { value: 'ratchet-sbx-artifacts' },
  sealed_bucket: { value: 'ratchet-sbx-sealed' },
  agent_boundary_ref: { value: `arn:aws:iam::${ACCT}:policy/ratchet/boundary` },
  eval_identity_ref: { value: identity('bench-runner') },
  confirm_identity_ref: { value: identity('confirm-runner') },
  sealed_confirm_prefix: { value: 'holdout/' },
  network_ref: { value: { vpc_id: 'vpc-1', subnet_id: 'subnet-1', bench_subnet_id: 'subnet-bench', security_group_ids: { agent_runner: 'sg-r', bench: 'sg-b' }, egress_proxy_url: null } },
  agent_role_path: { value: '/ratchet/agent/' },
  limits: { value: { max_instances_per_agent: 2, max_instance_lifetime_hours: 4 } },
});
const PLAN_JSON = JSON.stringify({
  resource_changes: [
    { address: 'aws_s3_bucket.artifacts', mode: 'managed', type: 'aws_s3_bucket', change: { actions: ['create'] } },
    { address: 'aws_vpc.sandbox', mode: 'managed', type: 'aws_vpc', change: { actions: ['create'] } },
  ],
});

interface Spec { args: string[]; env: Record<string, string> }
class FakeTofu {
  calls: Spec[] = [];
  async run(spec: Spec) {
    this.calls.push(spec);
    const sub = spec.args[0]!;
    if (sub === 'plan') return { exitCode: 2, stdout: '', stderr: '' };
    if (sub === 'show') return { exitCode: 0, stdout: PLAN_JSON, stderr: '' };
    if (sub === 'output') return { exitCode: 0, stdout: OUTPUTS_JSON, stderr: '' };
    return { exitCode: 0, stdout: '', stderr: '' };
  }
}

class FakeTester implements ConnectionTesterLike {
  calls: Array<{ target: ConnectionTarget; input: unknown }> = [];
  failWith: string | null = null;
  private reply(target: ConnectionTarget, input: unknown, ok: string): Promise<ConnectionResultVM> {
    this.calls.push({ target, input });
    return Promise.resolve(this.failWith ? { target, ok: false, detail: this.failWith } : { target, ok: true, detail: ok });
  }
  aws = (i: object) => this.reply('aws', i, 'signed in to account 123456789012');
  github = (i: object) => this.reply('github', i, 'acme/api; the token can push');
  anthropic = (i: object) => this.reply('anthropic', i, 'Anthropic accepted the key');
  openai = (i: object) => this.reply('openai', i, 'OpenAI accepted the key');
  tofu = (i: object) => this.reply('tofu', i, 'OpenTofu 1.8.2');
}

const dirs: string[] = [];
const services: RatchetService[] = [];
afterEach(() => {
  for (const s of services.splice(0)) s.dispose();
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

function setup(opts: { live?: boolean } = { live: true }) {
  const stateRoot = mkdtempSync(join(tmpdir(), 'ratchet-desktop-live-'));
  dirs.push(stateRoot);
  const ledger: Ledger = openLedger(':memory:');
  const settings = new SettingsStore(null, cipher, {});
  if (opts.live) settings.setMode('live');
  const tofu = new FakeTofu();
  const tester = new FakeTester();
  const created: LiveOptions[] = [];
  const defaults = { tofuBinary: '/opt/tofu', moduleDir: '/repo/infra/aws' };
  const live = {
    settings,
    stateRoot,
    defaults,
    createRuntime: (o: LiveOptions) => {
      created.push(o);
      return createLiveRuntime({ ...o, tofuRunner: tofu });
    },
  };
  const service = createService({ ledger, settings, liveDefaults: defaults, connectionTester: tester, createApp: (m) => createEngineForApp(ledger, m, { engine: 'real', live }) });
  services.push(service);
  return { ledger, settings, service, tofu, tester, created, stateRoot };
}

const everything = (...parts: unknown[]) => JSON.stringify(parts);

describe('live mode wiring', () => {
  it('starts unconfigured: explains what is missing, refuses to plan, and builds a runtime from the settings', async () => {
    const { service, created } = setup();
    expect(created).toHaveLength(1);
    expect(created[0]).toMatchObject({ sandbox: null, region: 'us-east-1', tofuBinary: '/opt/tofu', moduleDir: '/repo/infra/aws', sandboxId: 'ratchet-sandbox' });
    const s = await service.getSettings();
    expect(s.mode).toBe('live');
    expect(s.live).toMatchObject({ ready: false, awsRegion: 'us-east-1', awsAuth: 'profile', defaults: { tofuBinary: '/opt/tofu' } });
    expect(s.live.readiness.filter((r) => r.state === 'fail').map((r) => r.id)).toEqual(['aws', 'github', 'models', 'images', 'sandbox']);

    const info = await service.sandboxInfo();
    expect(info).toMatchObject({ status: 'absent', plan: null, canApply: false, account: { sandboxId: 'ratchet-sandbox', simulated: false } });
    expect(info.setupProblems).toEqual(['Choose an AWS profile in Settings.']);
    await expect(service.planSandbox()).rejects.toThrow(/AWS profile/);
    await expect(service.applySandbox()).rejects.toThrow(/AWS profile/);
  });

  it('settings -> plan on demand -> apply -> runtime rebuilt from the ledger outputs', async () => {
    const { service, settings, ledger, tofu, created } = setup();
    await service.saveLiveSettings({ awsProfile: 'sandbox', awsRegion: 'eu-west-1', awsAuth: 'sso' });
    expect(created).toHaveLength(2); // rebuilt with the new settings
    expect(created[1]).toMatchObject({ region: 'eu-west-1', awsProfile: 'sandbox', sandbox: null });
    expect(settings.live).toMatchObject({ awsProfile: 'sandbox', awsRegion: 'eu-west-1', awsAuth: 'sso' });

    // no plan until asked for; a real plan then shows the diff
    expect((await service.sandboxInfo()).plan).toBeNull();
    expect(tofu.calls).toHaveLength(0);
    const planned = await service.planSandbox();
    expect(planned).toMatchObject({ status: 'planned', canApply: true, setupProblems: [] });
    expect(planned.plan?.changes.map((c) => c.address)).toEqual(['aws_s3_bucket.artifacts', 'aws_vpc.sandbox']);
    expect(planned.plan?.summary).toBe('Plan: 2 to add, 0 to change, 0 to destroy.');
    expect(tofu.calls.every((c) => c.env.AWS_PROFILE === 'sandbox')).toBe(true);
    // polling the screen does not run tofu again
    const before = tofu.calls.length;
    await service.sandboxInfo();
    expect(tofu.calls).toHaveLength(before);

    const applied = await service.applySandbox();
    expect(applied).toMatchObject({ status: 'applied', canDestroy: true, account: { accountId: ACCT, region: 'eu-west-1', profile: 'sandbox', simulated: false }, plan: null });
    expect(applied.outputs?.artifactsBucket).toBe('ratchet-sbx-artifacts');
    const row = ledger.getSandbox('ratchet-sandbox')!;
    expect(row).toMatchObject({ status: 'applied', provider: 'aws', accountId: ACCT, region: 'eu-west-1' });

    // the live engine was rebuilt with the outputs from the ledger
    expect(created).toHaveLength(3);
    const last = created[2]!;
    expect((last.sandbox as AwsSandboxOutputs).network.benchSubnetId).toBe('subnet-bench');
    expect(last.sandbox).toEqual(row.outputs);
    expect((await service.getSettings()).live.readiness.find((r) => r.id === 'sandbox')?.state).toBe('ok');
  });

  it('feeds GitHub, images and secrets to the runtime in the main process only', async () => {
    const { service, created } = setup();
    await service.setSecret('githubToken', GH_TOKEN);
    await service.setSecret('anthropicApiKey', ANTHROPIC_KEY);
    await service.saveLiveSettings({ awsProfile: 'sandbox', githubRepo: 'acme/api', runnerImageRef: 'ami-0123456789abcdef0', benchImageRef: 'ami-0fedcba9876543210' });
    const last = created.at(-1)!;
    expect(last).toMatchObject({ githubRepo: 'acme/api', githubToken: GH_TOKEN, runnerImageRef: 'ami-0123456789abcdef0', benchImageRef: 'ami-0fedcba9876543210', awsProfile: 'sandbox' });
    expect(last.secrets?.('claude-code', 'ANTHROPIC_API_KEY')).toBe(ANTHROPIC_KEY);
    expect(last.secrets?.('codex', 'OPENAI_API_KEY')).toBeUndefined();
    expect(last.secrets?.('claude-code', 'SOMETHING_ELSE')).toBeUndefined();
    const s = await service.getSettings();
    expect(s.live.readiness.filter((r) => r.state === 'fail').map((r) => r.id)).toEqual(['sandbox']);
    // a form that names another repository would silently run on the Settings one: refuse
    await expect(service.createOptimization({ repoUrl: 'acme/other', baseRef: 'main', agents: ['claude-code'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 })).rejects.toThrow(/differs from the one in Settings/);
    // same repository, but the sandbox is not applied: the engine refuses
    await expect(service.createOptimization({ repoUrl: 'https://github.com/acme/api.git', baseRef: 'main', agents: ['claude-code'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 })).rejects.toThrow(/apply it first/);
  });

  it('refuses to destroy while an optimization in the sandbox is running', async () => {
    const { service, ledger } = setup();
    await service.saveLiveSettings({ awsProfile: 'sandbox' });
    await service.applySandbox();
    ledger.createOptimization({ id: 'opt-1', repo: 'acme/api', baseRef: 'main', metric: 'm', direction: 'minimize', minGain: 0.1, budgetUsd: 5, timeLimitS: 60, configYaml: 'x', status: 'running', sandboxId: 'ratchet-sandbox' });
    const info = await service.sandboxInfo();
    expect(info).toMatchObject({ status: 'applied', canDestroy: false, destroyBlockedReason: 'Stop optimization opt-1 first.' });
    await expect(service.destroySandbox()).rejects.toThrow(/still running.*opt-1/);
    expect(ledger.getSandbox('ratchet-sandbox')?.status).toBe('applied');
  });

  it('waits to rebuild the live engine while an optimization is running', async () => {
    const { service, ledger, created } = setup();
    ledger.createOptimization({ id: 'opt-1', repo: 'acme/api', baseRef: 'main', metric: 'm', direction: 'minimize', minGain: 0.1, budgetUsd: 5, timeLimitS: 60, configYaml: 'x', status: 'running' });
    const n = created.length;
    const s = await service.saveLiveSettings({ awsRegion: 'us-west-2' });
    expect(s.live.restartRequired).toBe(true);
    expect(created).toHaveLength(n);
    ledger.setOptimizationStatus('opt-1', 'stopped');
    const after = await service.saveLiveSettings({ awsRegion: 'us-west-2', githubRepo: 'acme/api' });
    expect(after.live.restartRequired).toBe(false);
    expect(created).toHaveLength(n + 1);
    expect(created.at(-1)).toMatchObject({ region: 'us-west-2' });
  });

  it('never mixes the simulation sandbox into live mode', async () => {
    const { service, ledger, created } = setup();
    // a mock sandbox from simulation, even with the live id, must not hand its fake outputs to the AWS clients
    ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'mock', region: 'us-east-1' });
    ledger.setSandboxStatus('ratchet-sandbox', 'applied', { outputs: { artifactsBucket: 'fake' } });
    await service.saveLiveSettings({ awsRegion: 'us-east-2' });
    expect(created.at(-1)?.sandbox).toBeNull();
  });
});

describe('Test connection', () => {
  it('reads the secrets in the main process, passes them to the tester, and returns only a message', async () => {
    const { service, tester } = setup();
    await service.saveLiveSettings({ awsProfile: 'sandbox', awsRegion: 'eu-west-1', githubRepo: 'acme/api' });
    await service.setSecret('githubToken', GH_TOKEN);
    await service.setSecret('anthropicApiKey', ANTHROPIC_KEY);
    await service.setSecret('openaiApiKey', OPENAI_KEY);

    const results = [await service.testConnection('aws'), await service.testConnection('github'), await service.testConnection('anthropic'), await service.testConnection('openai'), await service.testConnection('tofu')];
    expect(results.map((r) => [r.target, r.ok])).toEqual([['aws', true], ['github', true], ['anthropic', true], ['openai', true], ['tofu', true]]);
    expect(tester.calls).toEqual([
      { target: 'aws', input: { region: 'eu-west-1', profile: 'sandbox', expectedAccountId: undefined } },
      { target: 'github', input: { repo: 'acme/api', token: GH_TOKEN } },
      { target: 'anthropic', input: { apiKey: ANTHROPIC_KEY } },
      { target: 'openai', input: { apiKey: OPENAI_KEY } },
      { target: 'tofu', input: { binary: '/opt/tofu', moduleDir: '/repo/infra/aws' } },
    ]);
    for (const s of SECRETS) expect(JSON.stringify(results)).not.toContain(s);
  });

  it('says what to store first instead of calling out, and adds an SSO hint when sign-in fails', async () => {
    const { service, tester } = setup();
    expect(await service.testConnection('github')).toMatchObject({ ok: false, detail: expect.stringContaining('Set the GitHub repository') });
    await service.saveLiveSettings({ githubRepo: 'acme/api', awsProfile: 'corp-sso', awsAuth: 'sso' });
    expect(await service.testConnection('github')).toMatchObject({ ok: false, detail: 'Store a GitHub token first.' });
    expect(await service.testConnection('anthropic')).toMatchObject({ ok: false, detail: 'Store an Anthropic API key first.' });
    expect(await service.testConnection('openai')).toMatchObject({ ok: false, detail: 'Store an OpenAI API key first.' });
    expect(tester.calls).toHaveLength(0);
    tester.failWith = 'The SSO session associated with this profile has expired';
    const r = await service.testConnection('aws');
    expect(r.ok).toBe(false);
    expect(r.detail).toContain('aws sso login --profile corp-sso');
  });

  it('scrubs a stored secret out of whatever the tester returns', async () => {
    const { service, tester } = setup();
    await service.setSecret('anthropicApiKey', ANTHROPIC_KEY);
    tester.failWith = `request failed for key ${ANTHROPIC_KEY}`;
    const r = await service.testConnection('anthropic');
    expect(r.detail).not.toContain(ANTHROPIC_KEY);
    expect(r.detail).toContain('***');
  });

  it('is unavailable without a tester', async () => {
    const ledger = openLedger(':memory:');
    const service = createService({ ledger, settings: new SettingsStore(null, cipher, {}), createApp: (m) => createEngineForApp(ledger, m, { engine: 'dev', tickMs: 600_000 }) });
    services.push(service);
    expect(await service.testConnection('aws')).toMatchObject({ ok: false, detail: expect.stringContaining('not available') });
  });
});

describe('live settings validation', () => {
  it('rejects a bad value without saving anything from the batch', async () => {
    const { service, settings } = setup();
    await expect(service.saveLiveSettings({ awsRegion: 'eu-west-1', runnerImageRef: 'not-an-ami' })).rejects.toThrow(/Runner AMI: Use an AMI id/);
    expect(settings.live.awsRegion).toBe('us-east-1');
    await expect(service.saveLiveSettings({ githubRepo: 'nope' })).rejects.toThrow(/owner\/name/);
    await expect(service.saveLiveSettings({ tofuBinary: 'tofu; rm -rf /' })).rejects.toThrow(/OpenTofu binary/);
    await expect(service.saveLiveSettings({ moduleDir: 'relative/dir' })).rejects.toThrow(/absolute path/);
    await expect(service.setSecret('anthropicApiKey', 'sk-proj-wrongboxwrongbox')).rejects.toThrow(/Anthropic keys start with sk-ant-/);
    await expect(service.setSecret('githubToken', 'password123456')).rejects.toThrow(/GitHub tokens start with/);
  });

  it('normalizes and persists accepted values, including a trailing .git and blanks that mean "default"', async () => {
    const { service, settings } = setup();
    await service.saveLiveSettings({ githubRepo: ' acme/api.git ', tofuBinary: '/usr/local/bin/tofu', moduleDir: '/srv/infra/aws' });
    expect(settings.live).toMatchObject({ githubRepo: 'acme/api', tofuBinary: '/usr/local/bin/tofu', moduleDir: '/srv/infra/aws' });
    const s = await service.saveLiveSettings({ tofuBinary: '', moduleDir: '' });
    expect(effectivePaths(s.live, s.live.defaults)).toEqual({ tofuBinary: '/opt/tofu', moduleDir: '/repo/infra/aws' });
  });
});

describe('live helpers', () => {
  it('defaultLivePaths: bundled module and pinned tofu when packaged, checkout paths otherwise', () => {
    expect(defaultLivePaths({ isPackaged: true, resourcesPath: '/App/Resources', repoRoot: '/r', exists: (p) => p === '/App/Resources/bin/tofu' })).toEqual({ tofuBinary: '/App/Resources/bin/tofu', moduleDir: '/App/Resources/infra/aws' });
    expect(defaultLivePaths({ isPackaged: true, resourcesPath: '/App/Resources', repoRoot: '/r', exists: () => false }).tofuBinary).toBe('tofu');
    expect(defaultLivePaths({ isPackaged: false, resourcesPath: '/x', repoRoot: '/r' })).toEqual({ tofuBinary: 'tofu', moduleDir: '/r/infra/aws' });
  });

  it('liveReadiness lists what blocks a run and turns green when everything is set', () => {
    const live = { awsProfile: 'p', awsRegion: 'us-east-1', awsAuth: 'sso' as const, githubRepo: 'a/b', runnerImageRef: 'ami-0123456789abcdef0', benchImageRef: 'ami-0123456789abcdef1', tofuBinary: '', moduleDir: '' };
    const defaults = { tofuBinary: 'tofu', moduleDir: '/m' };
    const ready = liveReadiness({ live, hasSecret: () => true, sandboxApplied: true, defaults });
    expect(ready.every((r) => r.state === 'ok')).toBe(true);
    expect(ready[0]!.detail).toContain('SSO');
    const none = liveReadiness({ live: { ...live, githubRepo: '', runnerImageRef: '' }, hasSecret: (n) => n === 'githubToken', sandboxApplied: false, defaults });
    expect(none.filter((r) => r.state === 'fail').map((r) => r.id)).toEqual(['github', 'models', 'images', 'sandbox']);
  });
});

// ------------------------------------------------------------------ nothing secret crosses the IPC boundary

class FakeIpc implements IpcMainLike {
  handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
  handle(channel: string, fn: (event: unknown, ...args: unknown[]) => unknown): void {
    this.handlers.set(channel, fn);
  }
  invoke(channel: string, ...args: unknown[]): Promise<unknown> {
    return Promise.resolve(this.handlers.get(channel)!({}, ...args));
  }
}

describe('secrets never reach the renderer', () => {
  it('no IPC response, event or error carries a stored key or token', async () => {
    const { service, tofu } = setup();
    const ipc = new FakeIpc();
    registerIpc(ipc, service);
    const seen: unknown[] = [];
    service.onEvent((e) => seen.push(e));

    seen.push(await ipc.invoke(IPC.setSecret, 'githubToken', GH_TOKEN));
    seen.push(await ipc.invoke(IPC.setSecret, 'anthropicApiKey', ANTHROPIC_KEY));
    seen.push(await ipc.invoke(IPC.setSecret, 'openaiApiKey', OPENAI_KEY));
    seen.push(await ipc.invoke(IPC.saveLiveSettings, { awsProfile: 'sandbox', githubRepo: 'acme/api', runnerImageRef: 'ami-0123456789abcdef0', benchImageRef: 'ami-0fedcba9876543210' }));
    seen.push(await ipc.invoke(IPC.getSettings));
    seen.push(await ipc.invoke(IPC.planSandbox));
    seen.push(await ipc.invoke(IPC.applySandbox));
    seen.push(await ipc.invoke(IPC.sandboxInfo));
    seen.push(await ipc.invoke(IPC.listOptimizations));
    for (const t of ['aws', 'github', 'anthropic', 'openai', 'tofu']) seen.push(await ipc.invoke(IPC.testConnection, t));
    seen.push(await ipc.invoke(IPC.preflight, { agents: ['claude-code', 'codex'], configYaml: '' }));
    // error paths too
    const failing = [
      () => ipc.invoke(IPC.createOptimization, { repoUrl: 'acme/api', baseRef: 'main', agents: ['claude-code'], budgetUsd: 5, timeLimitSeconds: 60, plateauEvals: 3 }),
      () => ipc.invoke(IPC.setSecret, 'githubToken', `bad ${GH_TOKEN}`),
      () => ipc.invoke(IPC.setSecret, 'anthropicApiKey', OPENAI_KEY),
    ];
    for (const call of failing) seen.push(await call().catch((e: Error) => e.message));

    const dump = everything(seen);
    for (const secret of SECRETS) expect(dump).not.toContain(secret);
    // ...and tofu never saw them either (contract invariant: no secrets reach OpenTofu)
    for (const secret of SECRETS) expect(JSON.stringify(tofu.calls)).not.toContain(secret);
    // the only way a value goes out is the main-process engine settings, which the renderer cannot call
    expect((await service.getSettings()).secrets).toEqual({ anthropicApiKey: true, openaiApiKey: true, githubToken: true });
  });

  it('exposes no method that returns a secret: the API surface is the documented one', () => {
    const ipc = new FakeIpc();
    const { service } = setup();
    registerIpc(ipc, service);
    const channels = [...ipc.handlers.keys()];
    expect(channels.filter((c) => /getSecret|readSecret|revealSecret/i.test(c))).toEqual([]);
    expect(channels).toEqual(expect.arrayContaining([IPC.planSandbox, IPC.applySandbox, IPC.destroySandbox, IPC.saveLiveSettings, IPC.testConnection]));
  });
});
