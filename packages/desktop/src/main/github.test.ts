import { describe, expect, it } from 'vitest';
import { createGithubApi, PLACEHOLDER_GITHUB_CLIENT_ID, type DevicePoll, type GithubApi } from './github.js';
import { GithubSignIn } from './github-signin.js';
import { ConnectorStore } from './connectors.js';
import { SettingsStore } from './settings.js';

const cipher = { isAvailable: () => true, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };

interface Call { url: string; method: string; headers: Record<string, string>; body: string | null }

/** A fake `fetch`: answers by URL substring, records every call. No network. */
function fakeFetch(routes: Array<[string, (c: Call) => { status?: number; json?: unknown }]>) {
  const calls: Call[] = [];
  const f = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    const call: Call = { url, method: init?.method ?? 'GET', headers: (init?.headers ?? {}) as Record<string, string>, body: (init?.body as string | undefined) ?? null };
    calls.push(call);
    const route = routes.find(([k]) => url.includes(k));
    if (!route) return new Response('{}', { status: 404 });
    const r = route[1](call);
    return new Response(JSON.stringify(r.json ?? {}), { status: r.status ?? 200, headers: { 'content-type': 'application/json' } });
  }) as typeof fetch;
  return { f, calls };
}

describe('GitHub REST (fetch injected, no network)', () => {
  it('starts the device flow with the client id and the repo scope, and parses the code', async () => {
    const { f, calls } = fakeFetch([['/login/device/code', () => ({ json: { device_code: 'dc', user_code: 'ABCD-1234', verification_uri: 'https://github.com/login/device', expires_in: 900, interval: 5 } })]]);
    const api = createGithubApi({ clientId: 'Iv1.mine', fetch: f });
    expect(api.clientIdConfigured).toBe(true);
    expect(await api.deviceStart()).toEqual({ deviceCode: 'dc', userCode: 'ABCD-1234', verificationUri: 'https://github.com/login/device', expiresIn: 900, interval: 5 });
    expect(new URLSearchParams(calls[0]!.body!).get('client_id')).toBe('Iv1.mine');
    expect(new URLSearchParams(calls[0]!.body!).get('scope')).toBe('repo');
  });

  it('knows the placeholder client id is not a real one', () => {
    expect(createGithubApi({ fetch: fakeFetch([]).f }).clientIdConfigured).toBe(false);
    expect(createGithubApi({ clientId: '  ', fetch: fakeFetch([]).f }).clientIdConfigured).toBe(false);
    expect(PLACEHOLDER_GITHUB_CLIENT_ID).toMatch(/Placeholder/);
  });

  it.each<[Record<string, unknown>, DevicePoll]>([
    [{ access_token: 'gho_TOKENTOKENTOKEN1234567890' }, { status: 'ok', token: 'gho_TOKENTOKENTOKEN1234567890' }],
    [{ error: 'authorization_pending' }, { status: 'pending' }],
    [{ error: 'slow_down', interval: 12 }, { status: 'slow_down', interval: 12 }],
    [{ error: 'expired_token' }, { status: 'expired' }],
    [{ error: 'access_denied' }, { status: 'denied' }],
    [{ error: 'incorrect_client_credentials', error_description: 'bad client' }, { status: 'error', message: 'bad client' }],
  ])('maps the device poll answer %j', async (answer, expected) => {
    const { f, calls } = fakeFetch([['/login/oauth/access_token', () => ({ json: answer })]]);
    expect(await createGithubApi({ clientId: 'x', fetch: f }).devicePoll('dc')).toEqual(expected);
    expect(new URLSearchParams(calls[0]!.body!).get('grant_type')).toBe('urn:ietf:params:oauth:grant-type:device_code');
  });

  it('reads the account, lists repos most recently pushed first across pages, and filters', async () => {
    const page = (n: number, from: number) => Array.from({ length: n }, (_, i) => ({ full_name: `acme/r${from + i}`, default_branch: 'main', private: i % 2 === 0, description: i === 0 ? 'search service' : null, pushed_at: '2026-01-01T00:00:00Z' }));
    const { f, calls } = fakeFetch([
      ['/user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member&page=1', () => ({ json: page(100, 0) })],
      ['&page=2', () => ({ json: page(3, 100) })],
      ['/user', () => ({ json: { login: 'octocat' } })],
    ]);
    const api = createGithubApi({ clientId: 'x', fetch: f });
    expect(await api.user('tok')).toEqual({ login: 'octocat' });
    expect(calls[0]!.headers['Authorization']).toBe('Bearer tok');
    const all = await api.repos('tok');
    expect(all).toHaveLength(103);
    expect(all[0]).toEqual({ fullName: 'acme/r0', defaultBranch: 'main', private: true, description: 'search service', pushedAt: '2026-01-01T00:00:00Z' });
    expect((await api.repos('tok', 'SEARCH')).map((r) => r.fullName)).toEqual(['acme/r0', 'acme/r100']);
    expect((await api.repos('tok', 'acme/r10')).map((r) => r.fullName)).toContain('acme/r101');
  });

  it('reads a file at a ref (base64) and returns null for a missing one', async () => {
    const { f, calls } = fakeFetch([
      ['/contents/ratchet.yaml', () => ({ json: { encoding: 'base64', content: Buffer.from('version: 0\n').toString('base64') } })],
      ['/contents/nope.yaml', () => ({ status: 404 })],
    ]);
    const api = createGithubApi({ clientId: 'x', fetch: f });
    expect(await api.file('tok', 'acme/api', 'main', 'ratchet.yaml')).toBe('version: 0\n');
    expect(calls[0]!.url).toContain('/repos/acme/api/contents/ratchet.yaml?ref=main');
    expect(await api.file('tok', 'acme/api', 'main', 'nope.yaml')).toBeNull();
  });

  it('turns HTTP failures into plain messages that never carry the token', async () => {
    const api = createGithubApi({ clientId: 'x', fetch: fakeFetch([['/user', () => ({ status: 401 })], ['/repos/acme/x', () => ({ status: 404 })]]).f });
    await expect(api.user('gho_SECRETSECRETSECRET')).rejects.toThrow(/sign in again/);
    await expect(api.repo('gho_SECRETSECRETSECRET', 'acme/x')).rejects.toThrow(/not found/);
    const boom = createGithubApi({ clientId: 'x', fetch: (async () => { throw new Error('connect ECONNREFUSED with gho_SECRETSECRETSECRET'); }) as typeof fetch });
    await expect(boom.user('gho_SECRETSECRETSECRET')).rejects.not.toThrow(/SECRETSECRET/);
  });
});

// ------------------------------------------------------------------ the sign-in loop

class FakeApi implements GithubApi {
  clientIdConfigured = true;
  polls: DevicePoll[] = [];
  started = 0;
  async deviceStart() {
    this.started++;
    return { deviceCode: 'dc', userCode: 'WXYZ-9999', verificationUri: 'https://github.com/login/device', expiresIn: 900, interval: 5 };
  }
  async devicePoll(): Promise<DevicePoll> {
    return this.polls.shift() ?? { status: 'pending' };
  }
  async user() {
    return { login: 'octocat' };
  }
  async repos() { return []; }
  async repo(): Promise<never> { throw new Error('unused'); }
  async file() { return null; }
}

/** A sleep that never waits, so the loop runs as fast as the test awaits. */
const instant = async () => undefined;
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); await new Promise((r) => setTimeout(r, 5)); };

const saver = (connectors: ConnectorStore) => (token: string, login: string, connectorId: string | undefined) => {
  connectors.upsertGithub({ login, token, source: 'device', ...(connectorId ? { connectorId } : {}) });
};

describe('GithubSignIn (device flow)', () => {
  it('shows the code, opens the browser, polls until approved, then keeps the token in the keychain-backed store', async () => {
    const api = new FakeApi();
    api.polls = [{ status: 'pending' }, { status: 'slow_down', interval: 10 }, { status: 'ok', token: 'gho_DEVICEFLOWTOKEN1234567890' }];
    const settings = new SettingsStore(null, cipher, {});
    const opened: string[] = [];
    let signedIn = 0;
    const sleeps: number[] = [];
    const connectors = new ConnectorStore(settings);
    const flow = new GithubSignIn({ api, save: saver(connectors), openExternal: (u) => opened.push(u), sleep: async (ms) => { sleeps.push(ms); }, onSignedIn: () => signedIn++ });
    const f = await flow.start();
    expect(f).toMatchObject({ state: 'pending', userCode: 'WXYZ-9999', verificationUri: 'https://github.com/login/device' });
    expect(opened).toEqual(['https://github.com/login/device']);
    await settle();
    expect(connectors.githubToken()).toBe('gho_DEVICEFLOWTOKEN1234567890');
    expect(connectors.snapshot()).toMatchObject([{ kind: 'github', login: 'octocat', source: 'device', isDefault: true, status: 'connected' }]);
    expect(JSON.stringify(connectors.snapshot())).not.toContain('gho_');
    expect(flow.current()).toBeNull();
    expect(signedIn).toBe(1);
    // the interval GitHub asked for (5 s), then slowed down (+5 s at least)
    expect(sleeps.slice(0, 3)).toEqual([5000, 5000, 10_000]);
    expect(JSON.stringify(f)).not.toContain('gho_');
  });

  it.each<[DevicePoll, string, RegExp]>([
    [{ status: 'denied' }, 'denied', /denied/],
    [{ status: 'expired' }, 'expired', /expired/],
    [{ status: 'error', message: 'bad client id' }, 'error', /bad client id/],
  ])('ends in %j with a message and stores nothing', async (poll, state, msg) => {
    const api = new FakeApi();
    api.polls = [poll];
    const settings = new SettingsStore(null, cipher, {});
    const connectors = new ConnectorStore(settings);
    const flow = new GithubSignIn({ api, save: saver(connectors), sleep: instant });
    await flow.start();
    await settle();
    expect(flow.current()).toMatchObject({ state, message: expect.stringMatching(msg) });
    expect(connectors.snapshot()).toEqual([]);
  });

  it('expires on its own when the code outlives its time', async () => {
    const api = new FakeApi();
    let t = 0;
    const settings = new SettingsStore(null, cipher, {});
    const flow = new GithubSignIn({ api, save: saver(new ConnectorStore(settings)), now: () => t, sleep: async () => { t += 1_000_000; } });
    await flow.start();
    await settle();
    expect(flow.current()?.state).toBe('expired');
  });

  it('cancel stops the loop; a second start replaces the first flow', async () => {
    const api = new FakeApi();
    const settings = new SettingsStore(null, cipher, {});
    const connectors = new ConnectorStore(settings);
    const flow = new GithubSignIn({ api, save: saver(connectors), sleep: (ms, signal) => new Promise((r) => signal.addEventListener('abort', () => r(), { once: true })) });
    await flow.start();
    flow.cancel();
    expect(flow.current()).toBeNull();
    await flow.start();
    await flow.start();
    expect(api.started).toBe(3);
    flow.cancel();
    await settle();
    expect(connectors.snapshot()).toEqual([]);
  });

  it('signing in again as the same account replaces its token; a given connector id is re-signed-in', async () => {
    const api = new FakeApi();
    const settings = new SettingsStore(null, cipher, {});
    const connectors = new ConnectorStore(settings);
    api.polls = [{ status: 'ok', token: 'gho_FIRSTTOKENFIRSTTOKEN1234' }];
    await new GithubSignIn({ api, save: saver(connectors), sleep: instant }).start();
    await settle();
    const [first] = connectors.snapshot();
    api.polls = [{ status: 'ok', token: 'gho_SECONDTOKENSECONDTOKEN12' }];
    await new GithubSignIn({ api, save: saver(connectors), sleep: instant }).start({ connectorId: first!.id });
    await settle();
    expect(connectors.snapshot()).toHaveLength(1);
    expect(connectors.githubToken(first!.id)).toBe('gho_SECONDTOKENSECONDTOKEN12');
  });
});
