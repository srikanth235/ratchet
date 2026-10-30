import { describe, expect, it } from 'vitest';
import {
  FakeSpawner,
  detectHarnesses,
  fallbackPathDirs,
  localAgentEnv,
  parseVersion,
  resolveLoginPath,
  type DetectFs,
  type DetectOptions,
  type FakeCall,
  type FakeProcessSpec,
} from '../src/index.js';

const HOME = '/home/u';

class FakeFs implements DetectFs {
  constructor(
    readonly executables: string[] = [],
    readonly existing: string[] = [],
    readonly dirs: Record<string, string[]> = {},
  ) {}
  isExecutable = (p: string) => this.executables.includes(p);
  exists = (p: string) => this.executables.includes(p) || this.existing.includes(p) || p in this.dirs;
  listDir = (p: string) => this.dirs[p] ?? [];
}

type Rule = (call: FakeCall) => FakeProcessSpec | undefined;
const argsIs = (call: FakeCall, ...args: string[]) => args.length === call.args.length && args.every((a, i) => call.args[i] === a);

/** Spawner whose behaviour comes from the first rule that answers; anything unmatched exits 127 (command not found). */
function spawnerOf(...rules: Rule[]) {
  const fake = new FakeSpawner((call) => {
    for (const r of rules) {
      const spec = r(call);
      if (spec) return spec;
    }
    return { exitCode: 127, stderr: 'not found' };
  });
  return fake;
}

const shellRule = (path: string, noise = ''): Rule => (c) => (c.command.endsWith('sh') && c.args[0] === '-lc' ? { stdout: `${noise}__RATCHET_PATH__${path}__RATCHET_PATH__` } : undefined);

const claudeBin = '/usr/local/bin/claude';
const codexBin = '/usr/local/bin/codex';

function opts(over: DetectOptions & { fake: FakeSpawner }): DetectOptions {
  const { fake, ...rest } = over;
  return { spawner: fake.spawner, platform: 'linux', home: HOME, shell: '/bin/zsh', timeoutMs: 200, shellTimeoutMs: 200, env: { PATH: '/usr/bin' }, fs: new FakeFs(), ...rest };
}

describe('parseVersion', () => {
  it('finds a semver in the first line', () => {
    expect(parseVersion('2.1.4 (Claude Code)\n')).toBe('2.1.4');
    expect(parseVersion('codex-cli 0.46.0\n')).toBe('0.46.0');
    expect(parseVersion('v1.2.3-beta.1 extra')).toBe('1.2.3-beta.1');
  });
  it('falls back to the first line, and to null when empty', () => {
    expect(parseVersion('weird build\n')).toBe('weird build');
    expect(parseVersion('')).toBeNull();
  });
});

describe('resolveLoginPath', () => {
  it('asks a login shell for PATH, ignores rc-file noise, and merges the process PATH and existing fallback dirs', async () => {
    const fake = spawnerOf(shellRule('/opt/nvm/bin:/usr/bin:/opt/homebrew/bin', 'Welcome to zsh!\nnvm loaded\n'));
    const fs = new FakeFs([], ['/opt/homebrew/bin', '/usr/local/bin']);
    const r = await resolveLoginPath(opts({ fake, fs, env: { PATH: '/usr/bin:/proc/path', SHELL: '/bin/zsh' } }));
    expect(r.fromLoginShell).toBe(true);
    expect(r.path.split(':')).toEqual(['/opt/nvm/bin', '/usr/bin', '/opt/homebrew/bin', '/proc/path', '/usr/local/bin']);
    expect(fake.calls[0]).toMatchObject({ command: '/bin/zsh', args: ['-lc', expect.stringContaining('$PATH')] });
  });

  it('falls back to the process PATH and the common dirs when the shell times out (and kills it)', async () => {
    const fake = spawnerOf((c) => (c.args[0] === '-lc' ? { hang: true } : undefined));
    const fs = new FakeFs([], [`${HOME}/.local/bin`, '/opt/homebrew/bin']);
    const r = await resolveLoginPath(opts({ fake, fs, shellTimeoutMs: 30, env: { PATH: '/usr/bin' } }));
    expect(r.fromLoginShell).toBe(false);
    expect(r.path).toBe(['/usr/bin', '/opt/homebrew/bin', `${HOME}/.local/bin`].join(':'));
    expect(fake.calls[0]!.signals).toContain('SIGTERM');
  });

  it('falls back when the shell cannot be started or prints no markers', async () => {
    const broken = spawnerOf((c) => (c.args[0] === '-lc' ? { spawnError: 'ENOENT' } : undefined));
    expect((await resolveLoginPath(opts({ fake: broken }))).fromLoginShell).toBe(false);
    const silent = spawnerOf((c) => (c.args[0] === '-lc' ? { stdout: 'no markers here\n' } : undefined));
    const r = await resolveLoginPath(opts({ fake: silent, env: { PATH: '/x' } }));
    expect(r).toEqual({ path: '/x', fromLoginShell: false });
  });

  it('uses `string join` for fish, whose $PATH is a list', async () => {
    const fake = spawnerOf(shellRule('/a:/b'));
    const r = await resolveLoginPath(opts({ fake, shell: '/opt/homebrew/bin/fish' }));
    expect(fake.calls[0]!.args[1]).toContain('string join : $PATH');
    expect(r.path.startsWith('/a:/b')).toBe(true);
  });

  it('does not start a shell on Windows, and defaults the shell by platform', async () => {
    const fake = spawnerOf();
    await resolveLoginPath(opts({ fake, platform: 'win32', shell: undefined, env: { PATH: 'C:\\x' } }));
    expect(fake.calls).toHaveLength(0);
    const mac = spawnerOf(shellRule('/m'));
    await resolveLoginPath({ spawner: mac.spawner, platform: 'darwin', home: HOME, env: {}, fs: new FakeFs() });
    expect(mac.calls[0]!.command).toBe('/bin/zsh');
  });

  it('lists nvm / fnm versions newest first and honours NVM_BIN, VOLTA_HOME, PNPM_HOME', () => {
    const fs = new FakeFs([], ['/n/bin', '/v/bin', '/p'], {
      [`${HOME}/.nvm/versions/node`]: ['v18.9.0', 'v22.1.0', 'v20.10.0'],
      [`${HOME}/.nvm/versions/node/v22.1.0/bin`]: [],
      [`${HOME}/.nvm/versions/node/v20.10.0/bin`]: [],
      [`${HOME}/.fnm/node-versions`]: ['v22.0.0'],
      [`${HOME}/.fnm/node-versions/v22.0.0/installation/bin`]: [],
    });
    const dirs = fallbackPathDirs({ fs, home: HOME, platform: 'linux', env: { NVM_BIN: '/n/bin', VOLTA_HOME: '/v', PNPM_HOME: '/p' } });
    expect(dirs).toEqual(['/n/bin', '/v/bin', '/p', `${HOME}/.nvm/versions/node/v22.1.0/bin`, `${HOME}/.nvm/versions/node/v20.10.0/bin`, `${HOME}/.fnm/node-versions/v22.0.0/installation/bin`]);
  });
});

describe('detectHarnesses: install and version', () => {
  const claudeOk: Rule = (c) => (c.command === claudeBin && argsIs(c, '--version') ? { stdout: '2.1.4 (Claude Code)\n' } : undefined);
  const codexOk: Rule = (c) => (c.command === codexBin && argsIs(c, '--version') ? { stdout: 'codex-cli 0.46.0\n' } : undefined);

  it('reports not installed with an install hint', async () => {
    const fake = spawnerOf(shellRule('/usr/bin'));
    const [cc, cx] = await detectHarnesses(opts({ fake }));
    expect(cc).toMatchObject({ kind: 'claude-code', installed: false, version: null, path: null, auth: 'signed-out' });
    expect(cc!.hint).toMatch(/npm install -g @anthropic-ai\/claude-code/);
    expect(cx).toMatchObject({ kind: 'codex', installed: false, path: null });
    expect(cx!.hint).toMatch(/@openai\/codex/);
  });

  it('finds the binaries on the login-shell PATH and runs them with that PATH (npm shims need node on it)', async () => {
    const fs = new FakeFs([claudeBin, codexBin]);
    const fake = spawnerOf(shellRule('/usr/local/bin:/usr/bin'), claudeOk, codexOk, (c) => (c.args[0] === 'auth' || c.args[0] === 'login' ? { exitCode: 1, stderr: 'unknown' } : undefined));
    const [cc, cx] = await detectHarnesses(opts({ fake, fs, env: { PATH: '/bin' } }));
    expect(cc).toMatchObject({ installed: true, version: '2.1.4', path: claudeBin });
    expect(cx).toMatchObject({ installed: true, version: '0.46.0', path: codexBin });
    const versionCall = fake.calls.find((c) => c.command === claudeBin && c.args[0] === '--version')!;
    expect(versionCall.options.env['PATH']!.split(':')[0]).toBe('/usr/local/bin');
  });

  it('finds a binary in a fallback dir when the login shell is unavailable', async () => {
    const bin = `${HOME}/.npm-global/bin/codex`;
    const fs = new FakeFs([bin], [`${HOME}/.npm-global/bin`]);
    const fake = spawnerOf((c) => (c.args[0] === '-lc' ? { spawnError: 'ENOENT' } : undefined), (c) => (c.command === bin && argsIs(c, '--version') ? { stdout: 'codex-cli 1.0.0' } : undefined));
    const cx = (await detectHarnesses(opts({ fake, fs, kinds: ['codex'] })))[0]!;
    expect(cx).toMatchObject({ installed: true, path: bin, version: '1.0.0' });
  });

  it('a binary whose --version fails is not installed, with the reason in the hint', async () => {
    const fs = new FakeFs([claudeBin]);
    const exit1 = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { exitCode: 1, stderr: 'env: node: No such file or directory' } : undefined));
    const cc = (await detectHarnesses(opts({ fake: exit1, fs, kinds: ['claude-code'] })))[0]!;
    expect(cc).toMatchObject({ installed: false, path: claudeBin, version: null });
    expect(cc.hint).toMatch(/exited with 1.*node/);

    const slow = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { hang: true } : undefined));
    const timedOut = (await detectHarnesses(opts({ fake: slow, fs, kinds: ['claude-code'], timeoutMs: 30 })))[0]!;
    expect(timedOut).toMatchObject({ installed: false, path: claudeBin });
    expect(timedOut.hint).toMatch(/timed out/);

    const cannot = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { spawnError: 'EACCES' } : undefined));
    expect((await detectHarnesses(opts({ fake: cannot, fs, kinds: ['claude-code'] })))[0]!.hint).toMatch(/cannot run it/);
  });

  it('respects the kinds filter and a precomputed loginPath (no shell probe)', async () => {
    const fs = new FakeFs([codexBin]);
    const fake = spawnerOf(codexOk, (c) => (c.args[0] === 'login' ? { exitCode: 1, stderr: 'Not logged in' } : undefined));
    const res = await detectHarnesses(opts({ fake, fs, kinds: ['codex'], loginPath: '/usr/local/bin' }));
    expect(res.map((r) => r.kind)).toEqual(['codex']);
    expect(fake.calls.some((c) => c.args[0] === '-lc')).toBe(false);
  });
});

describe('detectHarnesses: Claude Code auth', () => {
  const claude = (rules: Rule[], over: Partial<DetectOptions> = {}, fs = new FakeFs([claudeBin])) =>
    detectHarnesses(
      opts({
        fake: spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: '2.1.4 (Claude Code)' } : undefined), ...rules),
        fs,
        kinds: ['claude-code'],
        ...over,
      }),
    ).then((r) => r[0]!);
  const status = (json: unknown, exitCode = 0): Rule => (c) => (c.command === claudeBin && argsIs(c, 'auth', 'status') ? { stdout: typeof json === 'string' ? json : JSON.stringify(json), exitCode } : undefined);

  it('signed-in when `claude auth status` says loggedIn', async () => {
    const r = await claude([status({ loggedIn: true, authMethod: 'claude.ai', apiProvider: 'firstParty' })]);
    expect(r.auth).toBe('signed-in');
    expect(r.hint).toMatch(/your own login/);
  });

  it('api-key when the status reports an API key login', async () => {
    expect((await claude([status({ loggedIn: true, authMethod: 'api_key' })])).auth).toBe('api-key');
  });

  it('signed-out when the CLI says so, unless a key is in the environment', async () => {
    const out = await claude([status({ loggedIn: false }, 1)]);
    expect(out.auth).toBe('signed-out');
    expect(out.hint).toMatch(/sign in.*ANTHROPIC_API_KEY/);
    const withKey = await claude([status({ loggedIn: false }, 1)], { env: { PATH: '/usr/bin', ANTHROPIC_API_KEY: 'sk-ant-secret-value' } });
    expect(withKey.auth).toBe('api-key');
  });

  it('asks the CLI about the stored login only: key variables are removed from the status probe', async () => {
    const fake = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: '2.1.4' } : undefined), status({ loggedIn: true }));
    await detectHarnesses(opts({ fake, fs: new FakeFs([claudeBin]), kinds: ['claude-code'], env: { PATH: '/usr/bin', ANTHROPIC_API_KEY: 'sk-ant-secret-value', HOME } }));
    const call = fake.calls.find((c) => argsIs(c, 'auth', 'status'))!;
    expect(call.options.env['ANTHROPIC_API_KEY']).toBeUndefined();
    expect(call.options.env['HOME']).toBe(HOME);
  });

  it('falls back to the credentials file when the status command is unavailable (old CLI, timeout, garbage)', async () => {
    const creds = new FakeFs([claudeBin], [`${HOME}/.claude/.credentials.json`]);
    const unsupported: Rule = (c) => (argsIs(c, 'auth', 'status') ? { exitCode: 1, stderr: "error: unknown command 'auth'" } : undefined);
    expect((await claude([unsupported], {}, creds)).auth).toBe('signed-in');
    const garbage: Rule = (c) => (argsIs(c, 'auth', 'status') ? { stdout: 'Logged in as someone' } : undefined);
    expect((await claude([garbage], {}, creds)).auth).toBe('signed-in');
    const hang: Rule = (c) => (argsIs(c, 'auth', 'status') ? { hang: true } : undefined);
    expect((await claude([hang], { timeoutMs: 40 }, creds)).auth).toBe('signed-in');
  });

  it('honours CLAUDE_CONFIG_DIR for the credentials file', async () => {
    const fs = new FakeFs([claudeBin], ['/cfg/.credentials.json']);
    const unsupported: Rule = (c) => (argsIs(c, 'auth', 'status') ? { exitCode: 1 } : undefined);
    expect((await claude([unsupported], { env: { PATH: '/usr/bin', CLAUDE_CONFIG_DIR: '/cfg' } }, fs)).auth).toBe('signed-in');
    expect((await claude([unsupported], {}, fs)).auth).toBe('signed-out');
  });

  describe('macOS keychain', () => {
    const unsupported: Rule = (c) => (argsIs(c, 'auth', 'status') ? { exitCode: 1 } : undefined);
    const keychain = (exitCode: number): Rule => (c) => (c.command === '/usr/bin/security' ? { exitCode, stdout: 'keychain: "/Users/u/Library/Keychains/login.keychain-db"\nattributes:\n' } : undefined);

    it('item present -> signed-in; and the secret is never requested (no -w / -g)', async () => {
      const fake = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: '2.1.4' } : undefined), unsupported, keychain(0));
      const [r] = await detectHarnesses(opts({ fake, platform: 'darwin', fs: new FakeFs([claudeBin]), kinds: ['claude-code'] }));
      expect(r!.auth).toBe('signed-in');
      const sec = fake.calls.find((c) => c.command === '/usr/bin/security')!;
      expect(sec.args).toEqual(['find-generic-password', '-s', 'Claude Code-credentials']);
      expect(sec.args).not.toContain('-w');
      expect(sec.args).not.toContain('-g');
    });

    it('item missing -> falls through to the API key, then signed-out', async () => {
      const mk = (env: Record<string, string>) =>
        detectHarnesses(opts({ fake: spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: '2.1.4' } : undefined), unsupported, keychain(44)), platform: 'darwin', fs: new FakeFs([claudeBin]), kinds: ['claude-code'], env: { PATH: '/usr/bin', ...env } })).then((r) => r[0]!);
      expect((await mk({ ANTHROPIC_API_KEY: 'sk-ant-x' })).auth).toBe('api-key');
      expect((await mk({})).auth).toBe('signed-out');
    });

    it('is not probed on Linux', async () => {
      const fake = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: '2.1.4' } : undefined), unsupported, keychain(0));
      const [r] = await detectHarnesses(opts({ fake, platform: 'linux', fs: new FakeFs([claudeBin]), kinds: ['claude-code'] }));
      expect(r!.auth).toBe('signed-out');
      expect(fake.calls.some((c) => c.command === '/usr/bin/security')).toBe(false);
    });
  });

  it('never puts a key value in the result', async () => {
    const r = await claude([status({ loggedIn: false }, 1)], { env: { PATH: '/usr/bin', ANTHROPIC_API_KEY: 'sk-ant-TOPSECRET' } });
    expect(JSON.stringify(r)).not.toContain('TOPSECRET');
  });
});

describe('detectHarnesses: Codex auth', () => {
  const codex = (rules: Rule[], over: Partial<DetectOptions> = {}, fs = new FakeFs([codexBin])) =>
    detectHarnesses(
      opts({
        fake: spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: 'codex-cli 0.46.0' } : undefined), ...rules),
        fs,
        kinds: ['codex'],
        ...over,
      }),
    ).then((r) => r[0]!);
  const status = (spec: FakeProcessSpec): Rule => (c) => (argsIs(c, 'login', 'status') ? spec : undefined);

  it('reads `codex login status` from stderr: ChatGPT -> signed-in', async () => {
    expect((await codex([status({ stderr: 'Logged in using ChatGPT\n' })])).auth).toBe('signed-in');
  });

  it('a stored API key is reported as api-key, and its masked form never reaches the result', async () => {
    const r = await codex([status({ stderr: 'API key configured - sk-testt***t1234\nAuthentication is checked when you send a request.\n' })]);
    expect(r.auth).toBe('api-key');
    expect(JSON.stringify(r)).not.toMatch(/sk-|t1234/);
  });

  it('"Not logged in" (exit 1) is signed-out, or api-key when OPENAI_API_KEY / CODEX_API_KEY is set', async () => {
    const out = status({ stderr: 'Not logged in\n', exitCode: 1 });
    const r = await codex([out]);
    expect(r.auth).toBe('signed-out');
    expect(r.hint).toMatch(/codex login/);
    expect((await codex([out], { env: { PATH: '/usr/bin', OPENAI_API_KEY: 'sk-x' } })).auth).toBe('api-key');
    expect((await codex([out], { env: { PATH: '/usr/bin', CODEX_API_KEY: 'sk-x' } })).auth).toBe('api-key');
  });

  it('exit 0 with unknown wording counts as signed-in', async () => {
    expect((await codex([status({ stdout: 'ok' })])).auth).toBe('signed-in');
  });

  it('removes key variables from the status probe', async () => {
    const fake = spawnerOf(shellRule('/usr/local/bin'), (c) => (argsIs(c, '--version') ? { stdout: 'codex-cli 1.0.0' } : undefined), status({ stderr: 'Not logged in', exitCode: 1 }));
    await detectHarnesses(opts({ fake, fs: new FakeFs([codexBin]), kinds: ['codex'], env: { PATH: '/usr/bin', OPENAI_API_KEY: 'sk-x', CODEX_API_KEY: 'sk-y' } }));
    const env = fake.calls.find((c) => argsIs(c, 'login', 'status'))!.options.env;
    expect(env['OPENAI_API_KEY']).toBeUndefined();
    expect(env['CODEX_API_KEY']).toBeUndefined();
  });

  it('falls back to auth.json (existence only) and CODEX_HOME when the status command fails or times out', async () => {
    const broken = status({ exitCode: 2, stderr: 'unrecognized subcommand' });
    const fs = new FakeFs([codexBin], [`${HOME}/.codex/auth.json`]);
    expect((await codex([broken], {}, fs)).auth).toBe('signed-in');
    expect((await codex([status({ hang: true })], { timeoutMs: 40 }, fs)).auth).toBe('signed-in');
    const custom = new FakeFs([codexBin], ['/elsewhere/auth.json']);
    expect((await codex([broken], { env: { PATH: '/usr/bin', CODEX_HOME: '/elsewhere' } }, custom)).auth).toBe('signed-in');
    expect((await codex([broken], {}, custom)).auth).toBe('signed-out');
  });

  it('no status, no file: an API key variable -> api-key, nothing -> signed-out', async () => {
    const broken = status({ spawnError: 'EACCES' });
    expect((await codex([broken], { env: { PATH: '/usr/bin', OPENAI_API_KEY: 'sk-z' } })).auth).toBe('api-key');
    expect((await codex([broken])).auth).toBe('signed-out');
  });
});

describe('localAgentEnv', () => {
  const base = {
    HOME: '/home/u',
    USER: 'u',
    SHELL: '/bin/zsh',
    PATH: '/old',
    ANTHROPIC_API_KEY: 'sk-ant',
    OPENAI_API_KEY: 'sk-oai',
    CODEX_API_KEY: 'sk-cx',
    GITHUB_TOKEN: 'ghp_x',
    GH_TOKEN: 'ghp_y',
    AWS_ACCESS_KEY_ID: 'AKIA',
    AWS_SECRET_ACCESS_KEY: 's',
    AWS_PROFILE: 'work',
    NPM_TOKEN: 'n',
    SSH_AUTH_SOCK: '/tmp/agent.sock',
    DB_PASSWORD: 'p',
    LANG: 'en_US.UTF-8',
    HTTPS_PROXY: 'http://proxy:3128',
    UNDEF: undefined,
  };

  it('keeps the user environment, sets the login PATH, and strips credentials', () => {
    const env = localAgentEnv({ kind: 'claude-code', auth: 'signed-in' }, base, '/login/bin:/usr/bin');
    expect(env).toEqual({ HOME: '/home/u', USER: 'u', SHELL: '/bin/zsh', PATH: '/login/bin:/usr/bin', LANG: 'en_US.UTF-8', HTTPS_PROXY: 'http://proxy:3128' });
  });

  it('passes the model key only for a harness that reports api-key auth, and only its own', () => {
    expect(localAgentEnv({ kind: 'claude-code', auth: 'api-key' }, base, '/p')['ANTHROPIC_API_KEY']).toBe('sk-ant');
    expect(localAgentEnv({ kind: 'claude-code', auth: 'api-key' }, base, '/p')['OPENAI_API_KEY']).toBeUndefined();
    const codex = localAgentEnv({ kind: 'codex', auth: 'api-key' }, base, '/p');
    expect(codex['OPENAI_API_KEY']).toBe('sk-oai');
    expect(codex['CODEX_API_KEY']).toBe('sk-cx');
    expect(codex['ANTHROPIC_API_KEY']).toBeUndefined();
    expect(localAgentEnv({ kind: 'codex', auth: 'signed-in' }, base, '/p')['OPENAI_API_KEY']).toBeUndefined();
  });

  it('api-key auth without the key set passes nothing', () => {
    expect(localAgentEnv({ kind: 'claude-code', auth: 'api-key' }, { HOME: '/h' }, '/p')).toEqual({ HOME: '/h', PATH: '/p' });
  });
});
