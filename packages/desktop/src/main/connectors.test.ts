import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { ConnectorError, ConnectorStore, type ConnectionTesterLike, type ConnectorDeps } from './connectors.js';
import { HarnessCache } from './harnesses.js';
import { SettingsStore, type SecretCipher } from './settings.js';
import { ANTHROPIC_KEY, FakeGithub, GH_TOKEN, OPENAI_KEY, claude, codex } from './test-support.js';
import type { AwsProfileList } from './aws-profiles.js';

const cipher = (available = true): SecretCipher & { on: boolean } => {
  const c = { on: available, isAvailable: () => c.on, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };
  return c;
};

const profiles: AwsProfileList = {
  profiles: [
    { name: 'acme-prod', kind: 'sso', source: 'config', ssoSession: 'acme' },
    { name: 'personal-dev', kind: 'keys', source: 'credentials', ssoSession: null },
    { name: 'acme-staging', kind: 'sso', source: 'config', ssoSession: 'acme' },
  ],
  ssoSessions: ['acme'],
  filesFound: true,
};

class Tester implements ConnectionTesterLike {
  fail: string | null = null;
  calls: string[] = [];
  private r(t: string, ok: string) {
    this.calls.push(t);
    return Promise.resolve(this.fail ? { ok: false, detail: this.fail } : { ok: true, detail: ok });
  }
  aws = (i: { profile?: string | undefined }) => this.r(`aws:${i.profile}`, 'signed in to account 123456789012');
  anthropic = () => this.r('anthropic', 'Anthropic accepted the key');
  openai = () => this.r('openai', 'OpenAI accepted the key');
}

const dirs: string[] = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

function make(opts: { cipher?: ReturnType<typeof cipher>; env?: NodeJS.ProcessEnv; file?: string | null; harnesses?: Parameters<typeof claude>[0][] | 'none'; deps?: Partial<ConnectorDeps> } = {}) {
  const c = opts.cipher ?? cipher();
  const settings = new SettingsStore(opts.file ?? null, c, opts.env ?? {});
  const github = new FakeGithub();
  const tester = new Tester();
  const harnesses = opts.harnesses === 'none' ? undefined : new HarnessCache({ loginPath: async () => '/bin', detect: async () => (opts.harnesses ? (opts.harnesses as never) : [claude(), codex()]) });
  const store = new ConnectorStore(settings, { github, tester, harnesses, listAwsProfiles: () => profiles, ...opts.deps });
  return { store, settings, github, tester, cipher: c };
}

const tmpFile = (): string => {
  const d = mkdtempSync(join(tmpdir(), 'ratchet-cn-'));
  dirs.push(d);
  return join(d, 'settings.json');
};

describe('GitHub accounts', () => {
  it('keeps several accounts; the first is the default and the default can move', () => {
    const { store } = make();
    const a = store.upsertGithub({ login: 'srikanth235', token: GH_TOKEN, source: 'device' });
    const b = store.upsertGithub({ login: 'acme-org', token: 'github_pat_ORGTOKENORGTOKENORGTOKEN12', source: 'token' });
    expect(store.snapshot().map((c) => [c.name, c.kind === 'github' && c.source, c.isDefault, c.status])).toEqual([
      ['srikanth235', 'device', true, 'connected'],
      ['acme-org', 'token', false, 'connected'],
    ]);
    store.setDefault(b.id);
    expect(store.snapshot().map((c) => c.isDefault)).toEqual([false, true]);
    expect(store.githubToken()).toBe('github_pat_ORGTOKENORGTOKENORGTOKEN12');
    expect(store.githubToken(a.id)).toBe(GH_TOKEN);
    store.remove(b.id); // removing the default hands it to the one that is left
    expect(store.snapshot().map((c) => [c.name, c.isDefault])).toEqual([['srikanth235', true]]);
    expect(store.githubToken(b.id)).toBeNull();
  });

  it('signing in again as the same account replaces its token; a connector id re-signs-in that connector', () => {
    const { store } = make();
    const a = store.upsertGithub({ login: 'octocat', token: 'gho_OLDTOKENOLDTOKENOLDTOKEN12', source: 'device' });
    store.upsertGithub({ login: 'OctoCat', token: 'gho_NEWTOKENNEWTOKENNEWTOKEN12', source: 'device' });
    expect(store.snapshot()).toHaveLength(1);
    expect(store.githubToken(a.id)).toBe('gho_NEWTOKENNEWTOKENNEWTOKEN12');
    store.upsertGithub({ login: 'octocat-renamed', token: 'gho_THIRDTOKENTHIRDTOKEN123456', source: 'device', connectorId: a.id });
    expect(store.snapshot()).toMatchObject([{ id: a.id, login: 'octocat-renamed', name: 'octocat-renamed' }]);
    expect(() => store.upsertGithub({ login: 'x', token: GH_TOKEN, source: 'device', connectorId: 'nope' })).toThrow(ConnectorError);
  });

  it('a pasted token is proven first (and a rejected one is not kept); a malformed one never reaches GitHub', async () => {
    const { store, github } = make();
    github.userFails = true;
    await expect(store.addGithubToken(GH_TOKEN)).rejects.toThrow(/rejected the sign-in/);
    expect(store.snapshot()).toEqual([]);
    await expect(store.addGithubToken('not-a-token')).rejects.toThrow(/GitHub tokens start with/);
    github.userFails = false;
    github.tokensSeen = [];
    const rec = await store.addGithubToken(GH_TOKEN, 'Work');
    expect(rec).toMatchObject({ name: 'Work', login: 'octocat', source: 'token' });
  });

  it('a GITHUB_TOKEN in the environment is a connector that cannot be removed, and the default when nothing else is stored', () => {
    const { store } = make({ env: { GITHUB_TOKEN: GH_TOKEN } });
    expect(store.snapshot()).toMatchObject([{ id: 'github-env', kind: 'github', source: 'env', isDefault: true, status: 'connected' }]);
    expect(store.githubToken()).toBe(GH_TOKEN);
    expect(() => store.remove('github-env')).toThrow(/environment/);
    const own = store.upsertGithub({ login: 'octocat', token: 'gho_STOREDTOKENSTOREDTOKEN1234', source: 'device' });
    expect(store.defaultOf('github')?.id).toBe(own.id); // a stored account wins over the environment
  });

  it('an unusable keychain refuses to store a token and changes nothing', () => {
    const c = cipher(false);
    const { store } = make({ cipher: c });
    expect(() => store.upsertGithub({ login: 'octocat', token: GH_TOKEN, source: 'device' })).toThrow(/keychain/);
    expect(store.snapshot()).toEqual([]);
  });
});

describe('AWS profiles', () => {
  it('several profiles with a region and an optional monthly cap; detail and defaults', () => {
    const { store } = make();
    const prod = store.addAws({ profile: 'acme-prod', region: 'us-east-1', monthlyCapUsd: 500 });
    store.addAws({ profile: 'personal-dev', region: 'us-west-2', monthlyCapUsd: 100 });
    store.addAws({ profile: 'acme-staging', region: 'eu-west-1' });
    expect(store.snapshot().map((c) => c.kind === 'aws' && [c.name, c.detail, c.isDefault])).toEqual([
      ['acme-prod', 'SSO · us-east-1 · cap $500 / month', true],
      ['personal-dev', '~/.aws profile · us-west-2 · cap $100 / month', false],
      ['acme-staging', 'SSO · eu-west-1', false],
    ]);
    store.update(prod.id, { name: 'Acme production', region: 'eu-west-2', monthlyCapUsd: null });
    expect(store.vmOf(prod.id)).toMatchObject({ name: 'Acme production', region: 'eu-west-2', monthlyCapUsd: null });
  });

  it('validates the profile, region, cap and duplicates', () => {
    const { store } = make();
    store.addAws({ profile: 'acme-prod' });
    expect(() => store.addAws({ profile: 'acme-prod' })).toThrow(/already connected/);
    expect(() => store.addAws({ profile: 'bad profile; rm -rf /' })).toThrow(ConnectorError);
    expect(() => store.addAws({ profile: 'x', region: 'mars-north-9' })).toThrow(/region/i);
    expect(() => store.addAws({ profile: 'y', monthlyCapUsd: -5 })).toThrow(/cap/);
    expect(store.snapshot()).toHaveLength(1);
    expect(store.snapshot()[0]).toMatchObject({ region: 'us-east-1', monthlyCapUsd: null });
  });

  it('shows a profile that left ~/.aws as invalid, and probes: SSO that fails is expired, static keys invalid', async () => {
    const { store, tester } = make({ deps: { listAwsProfiles: () => ({ ...profiles, profiles: profiles.profiles.slice(0, 2) }) } });
    const sso = store.addAws({ profile: 'acme-prod' });
    const keys = store.addAws({ profile: 'personal-dev' });
    const gone = store.addAws({ profile: 'acme-staging' });
    expect(store.vmOf(gone.id)).toMatchObject({ status: 'invalid', detail: expect.stringContaining('not found in ~/.aws') });
    expect(await store.test(sso.id)).toMatchObject({ ok: true });
    expect(store.vmOf(sso.id)?.status).toBe('connected');
    tester.fail = 'The SSO session has expired';
    const r = await store.test(sso.id);
    expect(r).toMatchObject({ ok: false });
    expect(r.detail).toContain('aws sso login --profile acme-prod');
    expect(store.vmOf(sso.id)?.status).toBe('expired');
    await store.test(keys.id);
    expect(store.vmOf(keys.id)?.status).toBe('invalid');
  });
});

describe('agent credentials', () => {
  it('detected CLI logins become connectors (installed ones only), logins before keys, with version and sign-in state', async () => {
    const { store } = make({ harnesses: [claude({ version: '2.1.314' }), codex({ installed: true, version: '0.166', path: '/bin/codex', auth: 'signed-out', hint: 'x' })] });
    await store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY, name: 'Claude Code · team key' });
    const list = await store.list();
    const agents = list.filter((c) => c.kind === 'agent');
    expect(agents.map((c) => [c.name, c.detail, c.status, c.isDefault])).toEqual([
      ['Claude Code · your login', 'Found on this Mac · v2.1.314', 'connected', true],
      ['Claude Code · team key', `API key ····${ANTHROPIC_KEY.slice(-4)} · also runs in the cloud`, 'valid', false],
      ['Codex · your login', 'Found on this Mac · v0.166 · run `codex login`', 'signed-out', false],
    ]);
    expect(agents.map((c) => c.kind === 'agent' && c.runsOn)).toEqual([['local'], ['local', 'cloud'], ['local']]);
  });

  it('a CLI that is not installed has no login connector; removing one keeps it removed until it is added again', async () => {
    const { store } = make();
    expect((await store.list()).filter((c) => c.kind === 'agent').map((c) => c.name)).toEqual(['Claude Code · your login']);
    store.remove('agent-claude-code-login');
    expect((await store.list()).filter((c) => c.kind === 'agent')).toEqual([]);
    await store.addAgent({ agent: 'claude-code', source: 'login' });
    expect((await store.list()).filter((c) => c.kind === 'agent').map((c) => c.name)).toEqual(['Claude Code · your login']);
    await expect(store.addAgent({ agent: 'codex', source: 'login' })).rejects.toThrow(/not found/);
  });

  it('an API key is format-checked, checked with the provider, and shown only as its last characters; a rejected key is kept as invalid', async () => {
    const { store, tester } = make();
    await expect(store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: OPENAI_KEY })).rejects.toThrow(/Anthropic keys start with sk-ant-/);
    const ok = await store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY });
    expect(store.vmOf(ok.id)).toMatchObject({ status: 'valid', keyHint: `····${ANTHROPIC_KEY.slice(-4)}` });
    tester.fail = 'invalid x-api-key';
    const bad = await store.addAgent({ agent: 'codex', source: 'api-key', apiKey: OPENAI_KEY });
    expect(store.vmOf(bad.id)).toMatchObject({ status: 'invalid', detail: expect.stringContaining('rejected') });
    expect(JSON.stringify(store.snapshot())).not.toContain(ANTHROPIC_KEY);
    expect(store.secretValue('anthropicApiKey')).toBe(ANTHROPIC_KEY);
  });

  it('one default for the kind: the stored one, else the first usable (logins first)', async () => {
    const { store } = make({ harnesses: [claude({ auth: 'signed-out', hint: 'x' }), codex({ installed: true, version: '1', path: '/bin/codex', auth: 'signed-in', hint: 'ok' })] });
    const list = await store.list();
    expect(list.filter((c) => c.isDefault).map((c) => c.name)).toEqual(['Codex · your login']); // the signed-out Claude login is skipped
    const key = await store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY });
    store.setDefault(key.id);
    expect((await store.list()).filter((c) => c.isDefault).map((c) => c.id)).toEqual([key.id]);
  });
});

describe('probes', () => {
  it('GitHub: a revoked token is expired, a network failure leaves the status alone, a missing token is signed out', async () => {
    const { store, github } = make();
    const a = store.upsertGithub({ login: 'octocat', token: GH_TOKEN, source: 'device' });
    expect(await store.test(a.id)).toEqual({ ok: true, detail: 'signed in as octocat' });
    github.userFails = true;
    expect(await store.test(a.id)).toMatchObject({ ok: false });
    expect(store.vmOf(a.id)).toMatchObject({ status: 'expired', detail: expect.stringContaining('sign in again') });
    github.user = async () => { throw new Error('Could not reach GitHub: offline'); };
    await store.test(a.id);
    expect(store.vmOf(a.id)?.status).toBe('expired'); // unchanged
    github.user = async () => ({ login: 'octocat' });
    await store.test(a.id);
    expect(store.vmOf(a.id)?.status).toBe('connected');
  });

  it('no secret value ever appears in a probe message', async () => {
    const { store, tester } = make();
    const k = await store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY });
    tester.fail = `401 for key ${ANTHROPIC_KEY}`;
    const r = await store.test(k.id);
    expect(r.detail).not.toContain(ANTHROPIC_KEY);
    expect(r.detail).toContain('***');
  });

  it('refresh checks every connector and never throws', async () => {
    const { store, tester } = make();
    store.upsertGithub({ login: 'octocat', token: GH_TOKEN, source: 'device' });
    store.addAws({ profile: 'acme-prod' });
    tester.fail = 'boom';
    await expect(store.refresh()).resolves.toBeUndefined();
    expect(tester.calls).toEqual(['aws:acme-prod']);
    expect(store.snapshot().find((c) => c.kind === 'aws')?.status).toBe('expired');
  });
});

describe('where secrets live', () => {
  it('tokens and keys are encrypted in the settings file; the metadata is plain; nothing leaks through a view model', async () => {
    const file = tmpFile();
    const { store, settings } = make({ file });
    store.upsertGithub({ login: 'octocat', token: GH_TOKEN, source: 'device' });
    await store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY });
    store.addAws({ profile: 'acme-prod', monthlyCapUsd: 250 });
    const raw = readFileSync(file, 'utf8');
    for (const secret of [GH_TOKEN, ANTHROPIC_KEY]) expect(raw).not.toContain(secret);
    expect(raw).toContain(Buffer.from(`enc:${GH_TOKEN}`).toString('base64'));
    expect(raw).toContain('"login": "octocat"');
    expect(raw).toContain('"monthlyCapUsd": 250');
    expect(JSON.stringify(await store.list())).not.toMatch(/ghp_|sk-ant/);
    expect(settings.encryptionAvailable).toBe(true);
  });

  it('survives a restart: the connectors, the defaults and the remembered selections come back', async () => {
    const file = tmpFile();
    const first = make({ file });
    const a = first.store.upsertGithub({ login: 'octocat', token: GH_TOKEN, source: 'device' });
    const b = first.store.upsertGithub({ login: 'acme-org', token: 'github_pat_ORGTOKENORGTOKENORGTOKEN12', source: 'token' });
    first.store.setDefault(b.id);
    const aws = first.store.addAws({ profile: 'acme-prod' });
    await first.store.list();
    first.store.saveRepoConnections('Acme/Api', { github: a.id, aws: aws.id, agents: [{ connectorId: 'agent-claude-code-login', where: 'local' }] });
    const second = make({ file });
    expect(second.store.snapshot().map((c) => [c.name, c.isDefault])).toEqual([['octocat', false], ['acme-org', true], ['acme-prod', true], ['Claude Code · your login', true]]);
    expect(second.store.githubToken(a.id)).toBe(GH_TOKEN);
    expect(second.store.repoConnections('acme/api').saved).toMatchObject({ github: a.id, aws: aws.id });
  });
});

describe('migration of the single-account Settings', () => {
  it('the GitHub token, the AWS profile and the API keys become connectors, once; the old fields are emptied', async () => {
    const file = tmpFile();
    const old = new SettingsStore(file, cipher(), {});
    old.setSecret('githubToken', GH_TOKEN);
    old.setGithubAccount('octocat', 'device');
    old.setAwsProfile('acme-prod');
    old.setLive({ awsRegion: 'eu-west-2' });
    old.setSecret('anthropicApiKey', ANTHROPIC_KEY);
    old.setSecret('openaiApiKey', OPENAI_KEY);

    const { store, settings } = make({ file });
    const list = await store.list();
    expect(list.map((c) => [c.kind, c.name, c.isDefault])).toEqual([
      ['github', 'octocat', true],
      ['aws', 'acme-prod', true],
      ['agent', 'Claude Code · your login', true],
      ['agent', 'Claude Code · API key', false],
      ['agent', 'Codex · API key', false],
    ]);
    expect(list.find((c) => c.kind === 'aws')).toMatchObject({ region: 'eu-west-2', profile: 'acme-prod' });
    expect(list.find((c) => c.kind === 'github')).toMatchObject({ source: 'device', login: 'octocat' });
    expect(store.githubToken()).toBe(GH_TOKEN);
    expect(store.secretValue('anthropicApiKey')).toBe(ANTHROPIC_KEY);
    expect(store.secretValue('openaiApiKey')).toBe(OPENAI_KEY);
    expect(settings.legacy()).toMatchObject({ awsProfile: '', githubLogin: '', secrets: {} });
    expect(readFileSync(file, 'utf8')).not.toContain(Buffer.from(`enc:${GH_TOKEN}`).toString('base64').repeat(2));

    // a second launch does not migrate again
    const again = make({ file });
    expect((await again.store.list()).filter((c) => c.kind !== 'agent')).toHaveLength(2);
    expect((await again.store.list()).filter((c) => c.kind === 'agent')).toHaveLength(3);
  });

  it('waits for the keychain instead of losing a secret it cannot read yet', async () => {
    const file = tmpFile();
    const old = new SettingsStore(file, cipher(), {});
    old.setSecret('githubToken', GH_TOKEN);
    old.setAwsProfile('acme-prod');
    const c = cipher(false);
    const locked = make({ file, cipher: c });
    expect(locked.store.snapshot()).toEqual([]);
    c.on = true;
    const later = make({ file, cipher: c });
    expect(later.store.snapshot().map((x) => x.kind)).toEqual(['github', 'aws']);
    expect(later.store.githubToken()).toBe(GH_TOKEN);
  });

  it('a fresh install has nothing to migrate', () => {
    const { store, settings } = make();
    expect(store.snapshot()).toEqual([]);
    expect(settings.connectorData.migrated).toBe(true);
  });
});

describe('the choice remembered per repo', () => {
  async function three() {
    const m = make({ env: {} });
    const g1 = m.store.upsertGithub({ login: 'octocat', token: GH_TOKEN, source: 'device' });
    const g2 = m.store.upsertGithub({ login: 'acme-org', token: 'github_pat_ORGTOKENORGTOKENORGTOKEN12', source: 'token' });
    const a1 = m.store.addAws({ profile: 'acme-prod', region: 'us-east-1' });
    const a2 = m.store.addAws({ profile: 'personal-dev', region: 'us-west-2', monthlyCapUsd: 100 });
    await m.store.list();
    const key = await m.store.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY });
    return { ...m, g1, g2, a1, a2, key, login: 'agent-claude-code-login' };
  }

  it('with nothing saved a repo uses the defaults', async () => {
    const t = await three();
    expect(t.store.repoConnections('acme/api')).toEqual({ repo: 'acme/api', saved: null, effective: { github: t.g1.id, aws: t.a1.id, agents: [{ connectorId: t.login, where: 'local' }] } });
    const r = await t.store.resolve('acme/api');
    expect(r.github).toMatchObject({ id: t.g1.id, token: GH_TOKEN });
    expect(r.aws).toMatchObject({ profile: 'acme-prod', region: 'us-east-1' });
    expect(r.agents.map((a) => [a.agent, a.source, a.where, a.apiKey])).toEqual([['claude-code', 'login', 'local', null]]);
    expect(r.cloudKeys).toEqual({ 'claude-code': ANTHROPIC_KEY });
  });

  it('save then get, keyed without regard to case; resolve follows it', async () => {
    const t = await three();
    const saved = t.store.saveRepoConnections('Acme/Shop-API', { github: t.g2.id, aws: t.a2.id, agents: [{ connectorId: t.key.id, where: 'cloud' }] });
    expect(saved).toMatchObject({ repo: 'acme/shop-api', saved: { github: t.g2.id, aws: t.a2.id, agents: [{ connectorId: t.key.id, where: 'cloud' }] } });
    expect(t.store.repoConnections('acme/SHOP-api').saved).toEqual(saved.saved);
    expect(t.store.repoConnections('acme/other').saved).toBeNull();
    const r = await t.store.resolve('acme/shop-api');
    expect(r.github).toMatchObject({ login: 'acme-org', token: 'github_pat_ORGTOKENORGTOKENORGTOKEN12' });
    expect(r.aws).toMatchObject({ profile: 'personal-dev', region: 'us-west-2', monthlyCapUsd: 100 });
    expect(r.agents).toMatchObject([{ agent: 'claude-code', source: 'api-key', where: 'cloud', apiKey: ANTHROPIC_KEY }]);
    expect(t.store.whereOf(saved.saved!.agents)).toBe('cloud');
    expect((await t.store.resolve('acme/other')).github?.id).toBe(t.g1.id); // another repo is unaffected
  });

  it('a chosen login never gets a model key, even when one is stored', async () => {
    const t = await three();
    t.store.saveRepoConnections('acme/api', { github: t.g1.id, aws: t.a1.id, agents: [{ connectorId: t.login, where: 'local' }] });
    expect((await t.store.resolve('acme/api')).cloudKeys).toEqual({});
  });

  it('rejects choices that cannot run: unknown ids, a login in the cloud, two connectors for one agent, nothing chosen', async () => {
    const t = await three();
    const ok = { github: t.g1.id, aws: t.a1.id };
    expect(() => t.store.saveRepoConnections('a/b', { ...ok, github: 'nope', agents: [{ connectorId: t.login, where: 'local' }] })).toThrow(/GitHub account/);
    expect(() => t.store.saveRepoConnections('a/b', { ...ok, aws: t.g1.id, agents: [{ connectorId: t.login, where: 'local' }] })).toThrow(/AWS/);
    expect(() => t.store.saveRepoConnections('a/b', { ...ok, agents: [] })).toThrow(/at least one agent/);
    expect(() => t.store.saveRepoConnections('a/b', { ...ok, agents: [{ connectorId: t.login, where: 'cloud' }] })).toThrow(/cannot run in the cloud/);
    expect(() => t.store.saveRepoConnections('a/b', { ...ok, agents: [{ connectorId: t.login, where: 'local' }, { connectorId: t.key.id, where: 'local' }] })).toThrow(/one Claude Code connector/);
    expect(t.store.repoConnections('a/b').saved).toBeNull();
    expect(() => t.store.whereOf([{ connectorId: t.login, where: 'local' }, { connectorId: t.key.id, where: 'cloud' }])).toThrow(/Run all agents/);
  });

  it('removing a chosen connector falls back to the defaults for that part', async () => {
    const t = await three();
    t.store.saveRepoConnections('acme/api', { github: t.g2.id, aws: t.a2.id, agents: [{ connectorId: t.login, where: 'local' }] });
    t.store.remove(t.a2.id);
    const v = t.store.repoConnections('acme/api');
    expect(v.saved).toBeNull();
    expect(v.effective).toMatchObject({ github: t.g2.id, aws: t.a1.id });
    expect((await t.store.resolve('acme/api')).aws?.profile).toBe('acme-prod');
  });
});
