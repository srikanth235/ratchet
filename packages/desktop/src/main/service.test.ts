import { openLedger, type EngineApi, type Ledger, type SandboxActivity } from '@ratchet/core';
import type { HarnessInfo } from '@ratchet/agents';
import { afterEach, describe, expect, it } from 'vitest';
import { IPC } from '../shared/channels.js';
import type { AppEngine } from './engine.js';
import { createEngineForApp } from './engine.js';
import { HarnessCache } from './harnesses.js';
import { registerIpc, type IpcMainLike } from './ipc.js';
import { createService, type ConnectionTesterLike, type RatchetService, type ServiceDeps } from './service.js';
import type { ConnectorStore } from './connectors.js';
import { SettingsStore } from './settings.js';
import { ANTHROPIC_KEY, DEVICE_TOKEN, FakeGithub, GH_TOKEN, OPENAI_KEY, YAML_OK, claude, codex, receipt } from './test-support.js';

const SECRETS = [GH_TOKEN, ANTHROPIC_KEY, OPENAI_KEY, DEVICE_TOKEN];

const cipher = { isAvailable: () => true, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };

class FakeTester implements ConnectionTesterLike {
  calls: Array<{ target: string; input: unknown }> = [];
  failWith: string | null = null;
  private reply(target: string, input: unknown, ok: string) {
    this.calls.push({ target, input });
    return Promise.resolve(this.failWith ? { ok: false, detail: this.failWith } : { ok: true, detail: ok });
  }
  aws = (i: object) => this.reply('aws', i, 'signed in to account 123456789012');
  anthropic = (i: object) => this.reply('anthropic', i, 'Anthropic accepted the key');
  openai = (i: object) => this.reply('openai', i, 'OpenAI accepted the key');
}

const services: RatchetService[] = [];
afterEach(() => services.splice(0).forEach((s) => s.dispose()));

interface Setup {
  service: RatchetService;
  ledger: Ledger;
  settings: SettingsStore;
  connectors: ConnectorStore;
  gh: FakeGithub;
  tester: FakeTester;
  engineCalls: string[];
  opened: string[];
}

/** The real service over the dev engine, with the engine's optional sandbox / template methods supplied by the test. */
function setup(opts: { engine?: Partial<EngineApi>; deps?: Partial<ServiceDeps>; harnesses?: HarnessInfo[]; env?: NodeJS.ProcessEnv; missingFor?: string[] } = {}): Setup {
  const ledger = openLedger(':memory:');
  const settings = new SettingsStore(null, cipher, opts.env ?? {});
  const gh = new FakeGithub();
  const tester = new FakeTester();
  const engineCalls: string[] = [];
  const opened: string[] = [];
  let connectors!: ConnectorStore;
  const base = (): AppEngine => createEngineForApp(ledger, { engine: 'dev', tickMs: 3_600_000 });
  const service = createService({
    ledger,
    settings,
    github: gh,
    connectionTester: tester,
    openExternal: (u) => opened.push(u),
    sleep: async () => undefined,
    listAwsProfiles: () => ({ profiles: [{ name: 'default', kind: 'keys', source: 'credentials', ssoSession: null }, { name: 'corp-sso', kind: 'sso', source: 'config', ssoSession: 'corp' }], ssoSessions: ['corp'], filesFound: true }),
    harnesses: new HarnessCache({ loginPath: async () => '/bin', detect: async () => opts.harnesses ?? [claude(), codex()] }),
    createApp: (hooks) => {
      connectors = hooks.connectors;
      const app = base();
      const engine = Object.assign(Object.create(app.engine) as EngineApi, opts.engine ?? {});
      return {
        ...app,
        engine,
        invalidate: () => void engineCalls.push('invalidate'),
        ...(opts.missingFor ? { missingFor: async () => opts.missingFor! } : {}),
        dispose: () => app.dispose(),
      };
    },
    ...opts.deps,
  });
  services.push(service);
  return { service, ledger, settings, connectors, gh, tester, engineCalls, opened };
}

const settle = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); await new Promise((r) => setTimeout(r, 5)); };

describe('GitHub row', () => {
  it('signed out by default; the device flow shows a code, then signs in once approved', async () => {
    const { service, gh, connectors, opened, engineCalls } = setup();
    expect(await service.githubStatus()).toEqual({ signedIn: false, login: null, source: null, clientIdConfigured: true, flow: null });
    gh.polls = [{ status: 'pending' }, { status: 'ok', token: DEVICE_TOKEN }];
    const s = await service.githubSignIn();
    expect(s.flow).toMatchObject({ state: 'pending', userCode: 'ABCD-1234' });
    expect(opened).toEqual(['https://github.com/login/device']);
    await settle();
    expect(await service.githubStatus()).toEqual({ signedIn: true, login: 'octocat', source: 'device', clientIdConfigured: true, flow: null });
    expect(connectors.githubToken()).toBe(DEVICE_TOKEN);
    expect(engineCalls).toContain('invalidate'); // runtimes built before the sign-in are rebuilt
    expect((await service.getSettings()).secrets.githubToken).toBe(true);
  });

  it('a denied sign-in is reported with its reason, and sign out clears the token and the account', async () => {
    const { service, gh, connectors } = setup();
    gh.polls = [{ status: 'denied' }];
    await service.githubSignIn();
    await settle();
    expect((await service.githubStatus()).flow).toMatchObject({ state: 'denied', message: expect.stringMatching(/denied/) });
    await service.setSecret('githubToken', GH_TOKEN);
    expect(await service.githubStatus()).toMatchObject({ signedIn: true, login: 'octocat', source: 'token' });
    const out = await service.githubSignOut();
    expect(out).toMatchObject({ signedIn: false, login: null, flow: null });
    expect(connectors.githubToken()).toBeNull();
    expect(connectors.snapshot()).toEqual([]);
  });

  it('a pasted token (Advanced) is proven against GitHub first, and a rejected one is not kept', async () => {
    const { service, gh, connectors } = setup();
    gh.userFails = true;
    await expect(service.setSecret('githubToken', GH_TOKEN)).rejects.toThrow(/rejected the sign-in/);
    expect(connectors.snapshot()).toEqual([]);
    gh.userFails = false;
    await service.setSecret('githubToken', GH_TOKEN);
    expect(connectors.snapshot()).toMatchObject([{ source: 'token', login: 'octocat' }]);
    await service.clearSecret('githubToken');
    expect(connectors.snapshot()).toEqual([]);
  });

  it('a token from the environment counts as signed in; its login is looked up once', async () => {
    const { service, gh } = setup({ env: { GITHUB_TOKEN: GH_TOKEN } });
    expect(await service.githubStatus()).toMatchObject({ signedIn: true, login: 'octocat', source: 'env' });
    await service.githubStatus();
    expect(gh.tokensSeen.filter((t) => t === GH_TOKEN)).toHaveLength(1);
  });

  it('"Test connection" for GitHub is a read of the account', async () => {
    const { service } = setup();
    expect(await service.testConnection('github')).toMatchObject({ ok: false, detail: 'Sign in with GitHub first.' });
    await service.setSecret('githubToken', GH_TOKEN);
    expect(await service.testConnection('github')).toEqual({ target: 'github', ok: true, detail: 'signed in as octocat' });
  });
});

describe('repo picker and ratchet.yaml', () => {
  it('lists the account repositories (needs the sign-in) and searches them', async () => {
    const { service } = setup();
    await expect(service.listRepos()).rejects.toThrow(/Sign in with GitHub/);
    await service.setSecret('githubToken', GH_TOKEN);
    expect((await service.listRepos()).map((r) => r.fullName)).toEqual(['acme/api', 'acme/web']);
    expect((await service.listRepos('web')).map((r) => r.fullName)).toEqual(['acme/web']);
  });

  it('reads ratchet.yaml from the default branch and summarizes what the repo declares', async () => {
    const { service, gh } = setup();
    await service.setSecret('githubToken', GH_TOKEN);
    gh.files.set('acme/web@trunk:ratchet.yaml', YAML_OK);
    const r = await service.inspectRepo({ kind: 'github', repo: 'acme/web' });
    expect(r).toMatchObject({ repo: 'acme/web', baseRef: 'trunk', problem: null, errors: [], templates: [] });
    expect(r.config).toMatchObject({
      name: 'Checkout latency', metric: 'http_req_duration.p95', direction: 'minimize',
      evaluator: { template: 'http-latency', repeats: null, hardware: 'c7i.4xlarge' },
      environment: { image: '.devcontainer/Dockerfile', machine: 'c7i.4xlarge', region: 'eu-west-2' },
    });
    // the protected set is the automatic one plus the repo's own list, with a reason each
    const globs = r.config!.protectedEntries.map((e) => e.glob);
    expect(globs).toEqual(expect.arrayContaining(['ratchet.yaml', 'bench/**', '.devcontainer/Dockerfile', 'docs/**']));
    expect(r.config!.protectedEntries.every((e) => e.reason.length > 0)).toBe(true);
    expect(r.config!.protectedGlobs).toEqual(['docs/**']);
    expect(r.configYaml).toBe(YAML_OK);
  });

  it('offers the templates when the repo has no ratchet.yaml, and shows why an invalid one is invalid', async () => {
    const { service, gh } = setup();
    await service.setSecret('githubToken', GH_TOKEN);
    const none = await service.inspectRepo({ kind: 'github', repo: 'acme/api' });
    expect(none).toMatchObject({ configYaml: null, config: null, baseRef: 'main' });
    expect(none.templates.map((t) => t.id)).toEqual(expect.arrayContaining(['http-latency', 'cli-cold-start', 'forecast-backtest']));
    expect(none.templates.every((t) => t.kind === 'template')).toBe(true);
    gh.files.set('acme/api@main:ratchet.yaml', 'version: 3\n');
    const bad = await service.inspectRepo({ kind: 'github', repo: 'acme/api' });
    expect(bad.config).toBeNull();
    expect(bad.errors.length).toBeGreaterThan(0);
    expect(bad.templates).toEqual([]);
    await expect(service.inspectRepo({ kind: 'github', repo: 'acme/ghost' })).rejects.toThrow(/not found/);
  });

  it('a local folder: its GitHub remote and ratchet.yaml, or the reason it cannot be used', async () => {
    const { service } = setup({
      deps: {
        pickFolder: async () => '/Users/me/api',
        inspectLocal: async (path) =>
          path === '/Users/me/api'
            ? { path, repo: 'acme/api', baseRef: 'main', configYaml: YAML_OK, problem: null }
            : { path, repo: null, baseRef: 'main', configYaml: null, problem: 'This folder has no GitHub remote (origin).' },
      },
    });
    expect(await service.pickLocalFolder()).toBe('/Users/me/api');
    const ok = await service.inspectRepo({ kind: 'local', path: '/Users/me/api' });
    expect(ok).toMatchObject({ repo: 'acme/api', problem: null, config: { metric: 'http_req_duration.p95' } });
    const bad = await service.inspectRepo({ kind: 'local', path: '/Users/me/scratch' });
    expect(bad).toMatchObject({ repo: null, config: null, templates: [], problem: expect.stringMatching(/no GitHub remote/) });
  });

  it('without a folder dialog, picking returns nothing', async () => {
    expect(await setup().service.pickLocalFolder()).toBeNull();
  });
});

describe('templates and the template pull request', () => {
  it('lists the gallery: runnable templates, then the demos', async () => {
    const t = await setup().service.listTemplates();
    expect(t.filter((x) => x.kind === 'template').map((x) => x.id)).toEqual(expect.arrayContaining(['http-latency', 'cli-cold-start', 'test-suite-time', 'build-size', 'memory', 'forecast-backtest', 'llm-eval']));
    expect(t.filter((x) => x.kind === 'demo').map((x) => x.id)).toEqual(['demo-search', 'demo-cli', 'demo-forecast']);
    expect(t.find((x) => x.id === 'http-latency')).toMatchObject({ holdout: 'hidden request mix', machine: 'c7i.2xlarge', objective: { metric: 'http_req_duration.p95', direction: 'minimize' } });
  });

  it('proposeTemplate opens the pull request through the engine and returns the link', async () => {
    const seen: unknown[] = [];
    const { service } = setup({
      engine: { proposeTemplate: async (i) => (seen.push(i), { prUrl: 'https://github.com/acme/api/pull/12', branch: 'ratchet/template-http-latency', sha: 'abc1234', templateId: i.templateId, files: ['ratchet.yaml', 'bench/http-latency.mjs', 'RATCHET.md'] }) },
    });
    await expect(service.proposeTemplate({ repoUrl: 'acme/api', baseRef: 'main', templateId: 'http-latency' })).rejects.toThrow(/Sign in with GitHub/);
    await service.setSecret('githubToken', GH_TOKEN);
    const r = await service.proposeTemplate({ repoUrl: 'https://github.com/acme/api.git', baseRef: 'main', templateId: 'http-latency' });
    expect(r).toEqual({ prUrl: 'https://github.com/acme/api/pull/12', branch: 'ratchet/template-http-latency', sha: 'abc1234', templateId: 'http-latency', files: ['ratchet.yaml', 'bench/http-latency.mjs', 'RATCHET.md'] });
    expect(seen[0]).toMatchObject({ repoUrl: 'acme/api', baseRef: 'main', templateId: 'http-latency' });
    await expect(service.proposeTemplate({ repoUrl: '/tmp/x', baseRef: 'main', templateId: 'http-latency' })).rejects.toThrow(/not a GitHub repository/);
  });

  it('surfaces the engine refusing (files already exist)', async () => {
    const { service } = setup({ engine: { proposeTemplate: async () => { throw new Error('ratchet.yaml already exists on main'); } } });
    await service.setSecret('githubToken', GH_TOKEN);
    await expect(service.proposeTemplate({ repoUrl: 'acme/api', baseRef: 'main', templateId: 'http-latency' })).rejects.toThrow(/already exists/);
  });
});

describe('creating an optimization', () => {
  const common = { baseRef: 'main', agents: ['claude-code' as const], budgetUsd: 50, timeLimitSeconds: 7200, plateauEvals: 25 };

  it('a real run uses the owner/name and the chosen place for the agents; what is missing stops it before anything is created', async () => {
    const seen: unknown[] = [];
    const { service, ledger } = setup({ engine: { createOptimization: async (i) => (seen.push(i), 'opt_x') } });
    expect(await service.createOptimization({ ...common, repoUrl: 'https://github.com/acme/api.git', agentsWhere: 'cloud' })).toBe('opt_x');
    expect(seen[0]).toMatchObject({ repoUrl: 'acme/api', baseRef: 'main', agentsWhere: 'cloud', budgetUsd: 50, timeLimitSeconds: 7200 });
    expect(JSON.stringify(seen[0])).not.toContain('configYaml');
    expect(ledger.listOptimizations().length).toBeGreaterThan(0);

    const blocked = setup({ missingFor: ['Sign in with GitHub (Settings)'], engine: { createOptimization: async () => 'never' } });
    await expect(blocked.service.createOptimization({ ...common, repoUrl: 'acme/api' })).rejects.toThrow(/Not ready: Sign in with GitHub/);
  });

  it('a demo goes to the simulation runtime and needs no repository', async () => {
    const demos: Array<[string, unknown]> = [];
    const { service } = setup({
      deps: {
        createApp: () => {
          const ledger = openLedger(':memory:');
          const app = createEngineForApp(ledger, { engine: 'dev', tickMs: 3_600_000 });
          return { ...app, createDemo: async (d, i) => (demos.push([d, i]), 'opt_demo') };
        },
      },
    });
    expect(await service.createOptimization({ ...common, demo: 'demo-forecast', repoUrl: '' })).toBe('opt_demo');
    expect(demos[0]![0]).toBe('demo-forecast');
    expect(demos[0]![1]).toMatchObject({ name: 'SKU demand forecast (demo)', baseRef: 'main' });
  });
});

describe('connectors', () => {
  const ORG = 'github_pat_ORGTOKENORGTOKENORGTOKEN12';

  it('lists detected logins with the saved accounts, grouped by kind, with one default per kind', async () => {
    const { service, gh } = setup();
    await service.setSecret('githubToken', GH_TOKEN);
    gh.login = 'acme-org';
    await service.addGithubConnector({ token: ORG, name: 'acme-org' });
    await service.addAwsConnector({ profile: 'default', region: 'us-west-2', monthlyCapUsd: 100 });
    const vm = await service.listConnectors();
    expect(vm).toMatchObject({ encryptionAvailable: true, clientIdConfigured: true, githubFlow: null });
    expect(vm.connectors.map((c) => [c.kind, c.name, c.isDefault, c.status])).toEqual([
      ['github', 'octocat', true, 'connected'],
      ['github', 'acme-org', false, 'connected'],
      ['aws', 'default', true, 'connected'],
      ['agent', 'Claude Code · your login', true, 'connected'],
    ]);
    expect(JSON.stringify(vm)).not.toContain(GH_TOKEN);
    expect(JSON.stringify(vm)).not.toContain(ORG);
  });

  it('every change rebuilds what depends on the accounts', async () => {
    const { service, engineCalls } = setup();
    await service.addAwsConnector({ profile: 'default' });
    const [aws] = (await service.listConnectors()).connectors;
    const before = engineCalls.length;
    await service.updateConnector(aws!.id, { name: 'prod' });
    await service.setDefaultConnector(aws!.id);
    await service.removeConnector(aws!.id);
    expect(engineCalls.slice(before)).toEqual(['invalidate', 'invalidate', 'invalidate']);
  });

  it('adding an AWS connector only offers profiles that exist in ~/.aws, and checks the profile', async () => {
    const { service, tester } = setup();
    await expect(service.addAwsConnector({ profile: 'nope' })).rejects.toThrow(/not in ~\/\.aws/);
    const vm = await service.addAwsConnector({ profile: 'corp-sso', region: 'eu-west-1' });
    expect(tester.calls.at(-1)).toMatchObject({ target: 'aws', input: { profile: 'corp-sso', region: 'eu-west-1' } });
    expect(vm.connectors[0]).toMatchObject({ kind: 'aws', status: 'connected', detail: 'SSO · eu-west-1' });
    tester.failWith = 'token expired';
    expect((await service.testConnector(vm.connectors[0]!.id)).connector).toMatchObject({ status: 'expired' });
    await expect(service.addAwsConnector({ profile: 'corp-sso' })).rejects.toThrow(/already connected/);
  });

  it('the device flow adds an account; signing in again with a connector id replaces that account token', async () => {
    const { service, gh, connectors } = setup();
    const started = await service.startGithubConnectorSignIn(); // the fake GitHub keeps answering "pending"
    expect(started.githubFlow).toMatchObject({ state: 'pending', userCode: 'ABCD-1234' });
    expect((await service.cancelGithubConnectorSignIn()).githubFlow).toBeNull();
    gh.polls = [{ status: 'ok', token: DEVICE_TOKEN }];
    await service.startGithubConnectorSignIn();
    await settle();
    const done = await service.listConnectors();
    expect(done.githubFlow).toBeNull();
    const [account] = done.connectors;
    expect(account).toMatchObject({ kind: 'github', login: 'octocat', source: 'device' });
    gh.polls = [{ status: 'ok', token: 'gho_REFRESHEDTOKENREFRESHED123456' }];
    await service.startGithubConnectorSignIn({ connectorId: account!.id });
    await settle();
    expect((await service.listConnectors()).connectors.filter((c) => c.kind === 'github')).toHaveLength(1);
    expect(connectors.githubToken(account!.id)).toBe('gho_REFRESHEDTOKENREFRESHED123456');
    await expect(service.startGithubConnectorSignIn({ connectorId: 'ghost' })).rejects.toThrow(/no longer exists/);
  });

  it('rename, default and remove; a login that is removed can be added back', async () => {
    const { service, gh } = setup();
    const vm = await service.addGithubConnector({ token: GH_TOKEN });
    gh.login = 'acme-org';
    const second = await service.addGithubConnector({ token: ORG, name: 'acme-org' });
    const org = second.connectors.find((c) => c.name === 'acme-org')!;
    await service.updateConnector(org.id, { name: 'Acme organisation' });
    expect((await service.setDefaultConnector(org.id)).connectors.find((c) => c.isDefault && c.kind === 'github')?.name).toBe('Acme organisation');
    await expect(service.updateConnector(vm.connectors[0]!.id, { name: 'Acme organisation' })).rejects.toThrow(/already a connector named/);
    const left = await service.removeConnector(org.id);
    expect(left.connectors.filter((c) => c.kind === 'github').map((c) => [c.name, c.isDefault])).toEqual([['octocat', true]]);
    const loginId = left.connectors.find((c) => c.kind === 'agent')!.id;
    expect((await service.removeConnector(loginId)).connectors.some((c) => c.kind === 'agent')).toBe(false);
    expect((await service.addAgentConnector({ agent: 'claude-code', source: 'login' })).connectors.some((c) => c.kind === 'agent')).toBe(true);
  });

  it('remembers the choice per repo and rebuilds that repo runtime', async () => {
    const { service, engineCalls, gh } = setup();
    await service.addGithubConnector({ token: GH_TOKEN });
    gh.login = 'acme-org';
    await service.addGithubConnector({ token: ORG, name: 'acme-org' });
    await service.addAwsConnector({ profile: 'default' });
    await service.addAwsConnector({ profile: 'corp-sso' });
    const all = (await service.listConnectors()).connectors;
    const id = (name: string) => all.find((c) => c.name === name)!.id;
    const empty = await service.getRepoConnections('acme/api');
    expect(empty).toMatchObject({ repo: 'acme/api', saved: null, effective: { github: id('octocat'), aws: id('default') } });
    const sel = { github: id('acme-org'), aws: id('corp-sso'), agents: [{ connectorId: 'agent-claude-code-login', where: 'local' as const }] };
    const before = engineCalls.length;
    expect((await service.saveRepoConnections('acme/api', sel)).saved).toEqual(sel);
    expect(engineCalls.slice(before)).toEqual(['invalidate']);
    expect((await service.getRepoConnections('ACME/API')).effective).toEqual({ github: sel.github, aws: sel.aws, agents: sel.agents });
    await expect(service.saveRepoConnections('acme/api', { ...sel, github: 'ghost' })).rejects.toThrow(/GitHub account/);
  });

  it('createOptimization with connections remembers them and runs the agents where they were chosen', async () => {
    const seen: Array<Record<string, unknown>> = [];
    const { service, connectors } = setup({ engine: { createOptimization: async (i) => (seen.push(i as never), 'opt_x') } });
    await service.addGithubConnector({ token: GH_TOKEN });
    await service.addAwsConnector({ profile: 'default' });
    const key = await connectors.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY });
    await service.listConnectors();
    const github = connectors.defaultOf('github')!.id;
    const aws = connectors.defaultOf('aws')!.id;
    const common = { baseRef: 'main', agents: ['codex' as const], budgetUsd: 50, timeLimitSeconds: 7200, plateauEvals: 25, repoUrl: 'https://github.com/acme/api.git' };
    await service.createOptimization({ ...common, connections: { github, aws, agents: [{ connectorId: key.id, where: 'cloud' }] } });
    expect(seen[0]).toMatchObject({ repoUrl: 'acme/api', agents: ['claude-code'], agentsWhere: 'cloud' }); // the connections win over `agents`
    expect(connectors.repoConnections('acme/api').saved).toMatchObject({ github, aws, agents: [{ connectorId: key.id, where: 'cloud' }] });
    await expect(
      service.createOptimization({ ...common, connections: { github, aws, agents: [{ connectorId: key.id, where: 'cloud' }, { connectorId: 'agent-codex-login', where: 'local' }] } }),
    ).rejects.toThrow();
    // without connections: the remembered choice stays, the request decides the rest
    await service.createOptimization({ ...common, agents: ['claude-code'], agentsWhere: 'local' });
    expect(seen[1]).toMatchObject({ agents: ['claude-code'], agentsWhere: 'local' });
  });

  it('preflight checks the connectors remembered for the repo', async () => {
    const { service, gh } = setup();
    await service.addGithubConnector({ token: GH_TOKEN });
    gh.login = 'acme-org';
    await service.addGithubConnector({ token: ORG, name: 'acme-org' });
    await service.addAwsConnector({ profile: 'default' });
    await service.addAwsConnector({ profile: 'corp-sso', region: 'eu-west-1' });
    const all = (await service.listConnectors()).connectors;
    await service.saveRepoConnections('acme/api', {
      github: all.find((c) => c.name === 'acme-org')!.id,
      aws: all.find((c) => c.name === 'corp-sso')!.id,
      agents: [{ connectorId: 'agent-claude-code-login', where: 'local' }],
    });
    const r = await service.preflight({ agents: ['claude-code'], agentsWhere: 'local', source: 'github', repo: 'acme/api' });
    expect(r.find((i) => i.id === 'aws')!.detail).toBe('corp-sso · eu-west-1');
    expect(r.find((i) => i.id === 'github')!.detail).toBe('signed in as acme-org');
    expect((await service.preflight({ agents: ['claude-code'], agentsWhere: 'local', source: 'github' })).find((i) => i.id === 'aws')!.detail).toBe('default · us-east-1');
  });

  it('the old single-account calls still work on top of the connectors', async () => {
    const { service, connectors } = setup();
    await service.setSecret('githubToken', GH_TOKEN);
    await service.setAwsProfile('corp-sso');
    await service.saveLiveSettings({ awsRegion: 'eu-west-2' });
    expect(await service.getSettings()).toMatchObject({ awsProfile: 'corp-sso', awsRegion: 'eu-west-2', secrets: { githubToken: true } });
    expect(connectors.snapshot().find((c) => c.kind === 'aws')).toMatchObject({ profile: 'corp-sso', region: 'eu-west-2' });
    await service.setAwsProfile('default'); // another profile: added, and the default moves to it
    expect(connectors.snapshot().filter((c) => c.kind === 'aws')).toHaveLength(2);
    expect((await service.getSettings()).awsProfile).toBe('default');
    await service.setAwsProfile('');
    expect((await service.getSettings()).awsProfile).toBe('corp-sso');
  });
});

describe('preflight: what is missing', () => {
  it('a demo needs nothing', async () => {
    const r = await setup().service.preflight({ agents: ['claude-code'], agentsWhere: 'local', source: 'demo' });
    expect(r).toEqual([expect.objectContaining({ id: 'demo', state: 'ok' })]);
  });

  it('a real run names each missing thing, then turns green as it is fixed', async () => {
    const { service } = setup(); // claude signed in, codex not installed
    const before = await service.preflight({ agents: ['claude-code', 'codex'], agentsWhere: 'local', source: 'github' });
    expect(before.filter((i) => i.state === 'fail').map((i) => i.id)).toEqual(['github', 'aws', 'agent-codex']);
    expect(before.find((i) => i.id === 'agent-codex')!.detail).toContain('npm i -g @openai/codex');
    expect(before.find((i) => i.id === 'agent-claude-code')).toMatchObject({ state: 'ok', detail: expect.stringContaining('2.1.4') });
    await service.setSecret('githubToken', GH_TOKEN);
    await service.setAwsProfile('default');
    const after = await service.preflight({ agents: ['claude-code'], agentsWhere: 'local', source: 'local' });
    expect(after.every((i) => i.state === 'ok')).toBe(true);
    expect(after.find((i) => i.id === 'github')!.detail).toBe('signed in as octocat');
    expect(after.find((i) => i.id === 'aws')!.detail).toBe('default · us-east-1');
  });

  it('a signed-out harness is missing unless an API key is in the environment (which detection already reports)', async () => {
    const { service } = setup({ harnesses: [claude({ auth: 'signed-out', hint: 'Run `claude` and sign in' })] });
    const r = await service.preflight({ agents: ['claude-code'], agentsWhere: 'local', source: 'github' });
    expect(r.find((i) => i.id === 'agent-claude-code')).toMatchObject({ state: 'fail', detail: 'Run `claude` and sign in' });
    const keyed = setup({ harnesses: [claude({ auth: 'api-key' })] });
    expect((await keyed.service.preflight({ agents: ['claude-code'], agentsWhere: 'local', source: 'github' })).find((i) => i.id === 'agent-claude-code')).toMatchObject({ state: 'ok', detail: expect.stringContaining('API key') });
  });

  it('cloud runners need API keys, not a local login', async () => {
    const { service } = setup();
    const r = await service.preflight({ agents: ['claude-code', 'codex'], agentsWhere: 'cloud', source: 'github' });
    expect(r.filter((i) => i.state === 'fail').map((i) => i.id)).toEqual(['github', 'aws', 'key-claude-code', 'key-codex']);
    await service.setSecret('anthropicApiKey', ANTHROPIC_KEY);
    expect((await service.preflight({ agents: ['claude-code'], agentsWhere: 'cloud', source: 'github' })).find((i) => i.id === 'key-claude-code')!.state).toBe('ok');
  });
});

describe('AWS and agents rows', () => {
  it('lists profiles by name, remembers the choice and the kind of sign-in, defaults the region', async () => {
    const { service, settings, engineCalls } = setup();
    const before = await service.awsProfiles();
    expect(before).toMatchObject({ selected: '', region: 'us-east-1', defaultRegion: 'us-east-1', filesFound: true, ssoSessions: ['corp'] });
    expect(before.profiles.map((p) => p.name)).toEqual(['default', 'corp-sso']);
    const s = await service.setAwsProfile('corp-sso');
    expect(s.awsProfile).toBe('corp-sso');
    expect(settings.live.awsAuth).toBe('sso');
    expect(engineCalls).toContain('invalidate');
    expect((await service.awsProfiles()).selected).toBe('corp-sso');
    expect((await service.saveLiveSettings({ awsRegion: 'eu-west-1' })).awsRegion).toBe('eu-west-1');
    await expect(service.saveLiveSettings({ awsRegion: 'mars' })).rejects.toThrow(/region/i);
  });

  it('"Test connection" passes the profile and region, adds the SSO hint, and never invents a profile', async () => {
    const { service, tester } = setup();
    expect(await service.testConnection('aws')).toMatchObject({ ok: false, detail: 'Choose an AWS profile first.' });
    await service.setAwsProfile('corp-sso');
    await service.saveLiveSettings({ awsRegion: 'eu-west-1' });
    expect(await service.testConnection('aws')).toMatchObject({ ok: true });
    expect(tester.calls.at(-1)).toEqual({ target: 'aws', input: { region: 'eu-west-1', profile: 'corp-sso' } });
    tester.failWith = 'The SSO session associated with this profile has expired';
    expect((await service.testConnection('aws')).detail).toContain('aws sso login --profile corp-sso');
  });

  it('detects the harnesses once and again on demand', async () => {
    let probes = 0;
    const { service } = setup({ deps: { harnesses: new HarnessCache({ loginPath: async () => '/bin', detect: async () => (probes++, probes === 1 ? [claude(), codex()] : [claude(), codex({ installed: true, version: '0.9.0', path: '/bin/codex', auth: 'signed-in', hint: 'signed in' })]) }) } });
    expect((await service.detectHarnesses()).map((h) => [h.kind, h.installed])).toEqual([['claude-code', true], ['codex', false]]);
    await service.detectHarnesses();
    expect(probes).toBe(1);
    expect((await service.redetectHarnesses()).find((h) => h.kind === 'codex')).toMatchObject({ installed: true, version: '0.9.0' });
    expect(probes).toBe(2);
    expect(await setup({ deps: { harnesses: undefined as never } }).service.detectHarnesses()).toBeDefined();
  });

  it('the developer overrides are validated and kept apart', async () => {
    const { service, settings } = setup();
    await expect(service.saveLiveSettings({ benchImageRef: 'not-an-ami' })).rejects.toThrow(/Bench AMI/);
    await expect(service.saveLiveSettings({ tofuBinary: 'tofu; rm -rf /' })).rejects.toThrow(/OpenTofu binary/);
    const s = await service.saveLiveSettings({ moduleDir: '/srv/infra/aws', benchImageRef: 'ami-0123456789abcdef0' });
    expect(s.dev).toMatchObject({ moduleDir: '/srv/infra/aws', benchImageRef: 'ami-0123456789abcdef0' });
    expect(settings.live.benchImageRef).toBe('ami-0123456789abcdef0');
  });
});

describe('environments (read-only) and the teardown receipt', () => {
  const activity = (over: Partial<SandboxActivity> = {}): SandboxActivity => ({
    sandboxId: 'ratchet-sandbox', status: 'applied', region: 'eu-west-2', auto: true, activeRuns: [], instancesRunning: 2, spendUsd: 1.5, idleSince: 5_000, destroyAt: 1_805_000, lastReceipt: null, ...over,
  });

  it('shows the running environment with phase, region, cost and when it removes itself', async () => {
    let ended = 0;
    const { service, ledger } = setup({
      engine: {
        sandboxActivity: async () => activity({ activeRuns: ['opt_demo_search'], destroyAt: null }),
        endSandbox: async () => (ended++, receipt()),
      },
    });
    ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    const e = await service.environments();
    expect(e.idleMinutes).toBe(30);
    expect(e.environments).toHaveLength(1);
    expect(e.environments[0]).toMatchObject({ sandboxId: 'ratchet-sandbox', region: 'eu-west-2', phase: 'ready', activeRuns: ['opt_demo_search'], instancesRunning: 2, spendUsd: 1.5, canEnd: false, simulated: false });
    expect(ended).toBe(0);
  });

  it('idle shows the destroy time; ending it now asks the engine and returns the new picture with the receipt', async () => {
    let live = true;
    const r = receipt({ trigger: 'manual' });
    const { service, ledger } = setup({
      engine: {
        sandboxActivity: async () => (live ? activity() : activity({ status: 'destroyed', lastReceipt: r })),
        endSandbox: async () => ((live = false), (ledger.setSandboxStatus('ratchet-sandbox', 'destroyed'), r)),
      },
    });
    ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    const idle = (await service.environments()).environments[0]!;
    expect(idle).toMatchObject({ phase: 'idle', destroyAt: 1_805_000, canEnd: true });
    const after = await service.endSandbox('ratchet-sandbox');
    expect(after.environments).toEqual([]);
    expect(after.lastReceipt).toMatchObject({ sandboxId: 'ratchet-sandbox', trigger: 'manual', nothingRunning: true, spendUsd: 4.2, removed: { instances: 2, agentIdentities: 1, secrets: 1 } });
    expect(after.lastReceipt!.spendByCategory).toEqual([{ label: 'Model tokens', usd: 3 }, { label: 'Bench runner', usd: 1.2 }]);
  });

  it('follows the sandbox events for the phase it shows', async () => {
    const dev = { listeners: new Set<(e: never) => void>() };
    const { service, ledger } = setup({
      engine: {
        sandboxActivity: async () => activity(),
        subscribe: (l) => (dev.listeners.add(l as never), () => dev.listeners.delete(l as never)),
      },
    });
    ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    for (const l of dev.listeners) l({ type: 'sandbox', sandboxId: 'ratchet-sandbox', phase: 'destroying', region: 'eu-west-2', text: 'removing the environment' } as never);
    expect((await service.environments()).environments[0]).toMatchObject({ phase: 'destroying', text: 'removing the environment' });
  });

  it('Result gets the receipt of the run and the calibration in plain words', async () => {
    const { service } = setup({
      engine: {
        teardownReceipt: async (id) => (id === 'opt_demo_ingest' ? receipt() : null),
        calibration: async () => ({ explanation: 'this metric is noisy (±9%); wins must beat 9%; each candidate is measured 7 times' }) as never,
      },
    });
    const d = await service.getOptimization('opt_demo_ingest');
    expect(d.receipt).toMatchObject({ summary: 'Removed 2 instances; nothing is left running.', nothingRunning: true, destroyedAt: Date.UTC(2026, 9, 30, 12, 0, 0) });
    expect(d.calibration).toContain('wins must beat 9%');
    expect((await service.getOptimization('opt_demo_search')).receipt).toBeNull();
  });

  it('an engine that cannot say (no sandbox support) yields an empty, honest view', async () => {
    const { service } = setup();
    expect(await service.environments()).toMatchObject({ environments: [], lastReceipt: null });
    await expect(service.endSandbox('ratchet-sandbox')).rejects.toThrow(/cannot end the sandbox/);
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
    const { service, gh, tester } = setup();
    const ipc = new FakeIpc();
    registerIpc(ipc, service);
    const seen: unknown[] = [];
    service.onEvent((e) => seen.push(e));
    gh.polls = [{ status: 'ok', token: DEVICE_TOKEN }];
    seen.push(await ipc.invoke(IPC.githubSignIn));
    await settle();
    seen.push(await ipc.invoke(IPC.githubStatus));
    seen.push(await ipc.invoke(IPC.setSecret, 'githubToken', GH_TOKEN));
    seen.push(await ipc.invoke(IPC.setSecret, 'anthropicApiKey', ANTHROPIC_KEY));
    seen.push(await ipc.invoke(IPC.setSecret, 'openaiApiKey', OPENAI_KEY));
    seen.push(await ipc.invoke(IPC.saveLiveSettings, { awsRegion: 'eu-west-1', benchImageRef: 'ami-0fedcba9876543210' }));
    seen.push(await ipc.invoke(IPC.setAwsProfile, 'default'));
    seen.push(await ipc.invoke(IPC.addGithubConnector, { token: GH_TOKEN, name: 'second' }));
    seen.push(await ipc.invoke(IPC.addAgentConnector, { agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY }));
    for (const m of [IPC.listConnectors, IPC.refreshConnectors]) seen.push(await ipc.invoke(m));
    for (const c of (await service.listConnectors()).connectors) seen.push(await ipc.invoke(IPC.testConnector, c.id));
    for (const m of [IPC.getSettings, IPC.environments, IPC.listRepos, IPC.listTemplates, IPC.awsProfiles, IPC.detectHarnesses, IPC.redetectHarnesses, IPC.listOptimizations]) seen.push(await ipc.invoke(m));
    for (const t of ['aws', 'github', 'anthropic', 'openai']) seen.push(await ipc.invoke(IPC.testConnection, t));
    seen.push(await ipc.invoke(IPC.preflight, { agents: ['claude-code', 'codex'], agentsWhere: 'local', source: 'github' }));
    seen.push(await ipc.invoke(IPC.inspectRepo, { kind: 'github', repo: 'acme/api' }));
    tester.failWith = `rejected ${ANTHROPIC_KEY} and ${OPENAI_KEY} and ${GH_TOKEN}`;
    for (const t of ['aws', 'anthropic', 'openai']) seen.push(await ipc.invoke(IPC.testConnection, t));
    const failing = [
      () => ipc.invoke(IPC.setSecret, 'githubToken', `bad ${GH_TOKEN}`),
      () => ipc.invoke(IPC.setSecret, 'anthropicApiKey', OPENAI_KEY),
      () => ipc.invoke(IPC.inspectRepo, { kind: 'github', repo: 'acme/ghost' }),
    ];
    for (const call of failing) seen.push(await call().catch((e: Error) => e.message));
    const dump = JSON.stringify(seen);
    for (const secret of SECRETS) expect(dump).not.toContain(secret);
    expect((await service.getSettings()).secrets).toEqual({ anthropicApiKey: true, openaiApiKey: true, githubToken: true });
  });

  it('exposes no method that returns a secret, and no way to apply or destroy infrastructure', () => {
    const ipc = new FakeIpc();
    const { service } = setup();
    registerIpc(ipc, service);
    const channels = [...ipc.handlers.keys()];
    expect(channels.filter((c) => /getSecret|readSecret|revealSecret|token/i.test(c))).toEqual([]);
    expect(channels.filter((c) => /plan|apply|destroySandbox|setMode|teardown$/i.test(c))).toEqual([]);
    expect(channels).toEqual(expect.arrayContaining([IPC.endSandbox, IPC.environments, IPC.githubSignIn, IPC.proposeTemplate]));
  });
});
