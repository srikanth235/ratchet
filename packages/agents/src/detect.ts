/**
 * Harness detection: which agent CLIs are installed on this machine, and whether the user is signed in to them.
 *
 * Runs in the app's main process (never the renderer) on launch and on demand. Three things make it more than
 * `which claude`:
 *  - PATH. A macOS GUI app inherits launchd's bare PATH, not the user's shell PATH, so `claude` / `codex` (installed by npm,
 *    Homebrew, nvm, volta, ...) are invisible. The PATH is resolved the way a login shell would (`$SHELL -lc`), merged with the
 *    process PATH and the usual install directories.
 *  - Auth. The CLIs' own non-interactive status command is asked first (`codex login status`, `claude auth status`); then the
 *    signals they use (credential files, the macOS keychain item), then API-key environment variables.
 *  - Secrets. Only the existence of credentials is ever checked. No credential file is opened and no key value is returned,
 *    logged or put in a hint (`security find-generic-password` runs without `-w` / `-g`, so the secret is not even fetched).
 *
 * Everything that touches the machine is injectable (spawner, fs, env, platform, home) and every probe has a timeout.
 */
import { accessSync, constants, existsSync, readdirSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, delimiter, join } from 'node:path';
import { nodeSpawner, runProcess, type Spawner } from './process.js';

export type HarnessKind = 'claude-code' | 'codex';
export type HarnessAuth = 'signed-in' | 'api-key' | 'signed-out';

export interface HarnessInfo {
  kind: HarnessKind;
  installed: boolean;
  /** Parsed from `<binary> --version` (e.g. `2.1.4`); null when not installed or the version could not be read. */
  version: string | null;
  /** Absolute path of the binary, null when not found. */
  path: string | null;
  /** `signed-in`: the user's own login; `api-key`: no login but a key is set in the environment; else `signed-out`. */
  auth: HarnessAuth;
  /** What to do next, or a one-line description of what was found. Never contains a credential. */
  hint: string;
}

/** The file system the probes use. Existence checks only: nothing here can read a file's contents. */
export interface DetectFs {
  isExecutable(path: string): boolean;
  exists(path: string): boolean;
  listDir(path: string): string[];
}

export const nodeDetectFs: DetectFs = {
  isExecutable(path) {
    try {
      if (!statSync(path).isFile()) return false;
      accessSync(path, constants.X_OK);
      return true;
    } catch {
      return false;
    }
  },
  exists: (path) => existsSync(path),
  listDir(path) {
    try {
      return readdirSync(path);
    } catch {
      return [];
    }
  },
};

export interface DetectOptions {
  /** Runs the probes (`--version`, status commands, the login shell, `security`). Default: real processes. */
  spawner?: Spawner;
  fs?: DetectFs;
  /** Default `process.env`. */
  env?: Record<string, string | undefined>;
  platform?: NodeJS.Platform;
  /** Default `os.homedir()`. */
  home?: string;
  /** The login shell to ask for PATH. Default `env.SHELL`, else `/bin/zsh` on macOS and `/bin/sh` elsewhere. */
  shell?: string;
  /** Timeout of every probe except the login shell. Default 5 s. */
  timeoutMs?: number;
  /** Timeout of the login-shell PATH probe (rc files can be slow). Default 8 s. */
  shellTimeoutMs?: number;
  /** Which harnesses to look for. Default both. */
  kinds?: HarnessKind[];
  /** A PATH already resolved by `resolveLoginPath` (skips the login-shell probe). */
  loginPath?: string;
}

const DEFAULT_TIMEOUT = 5_000;
const DEFAULT_SHELL_TIMEOUT = 8_000;
const MARK = '__RATCHET_PATH__';

interface Ctx {
  spawner: Spawner;
  fs: DetectFs;
  env: Record<string, string | undefined>;
  platform: NodeJS.Platform;
  home: string;
  timeoutMs: number;
  shellTimeoutMs: number;
  shell: string | undefined;
}

function ctxOf(opts: DetectOptions): Ctx {
  const env = opts.env ?? process.env;
  const platform = opts.platform ?? process.platform;
  return {
    spawner: opts.spawner ?? nodeSpawner,
    fs: opts.fs ?? nodeDetectFs,
    env,
    platform,
    home: opts.home ?? env['HOME'] ?? homedir(),
    timeoutMs: opts.timeoutMs ?? DEFAULT_TIMEOUT,
    shellTimeoutMs: opts.shellTimeoutMs ?? DEFAULT_SHELL_TIMEOUT,
    shell: opts.shell ?? env['SHELL'] ?? (platform === 'darwin' ? '/bin/zsh' : platform === 'win32' ? undefined : '/bin/sh'),
  };
}

// ---------------------------------------------------------------- probing

interface ProbeResult {
  /** Exited 0 within the timeout. */
  ok: boolean;
  code: number | null;
  timedOut: boolean;
  /** The process could not be started at all. */
  spawnFailed: boolean;
  stdout: string;
  stderr: string;
}

async function probe(ctx: Ctx, command: string, args: string[], env: Record<string, string>, timeoutMs: number): Promise<ProbeResult> {
  const ac = new AbortController();
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    ac.abort();
  }, timeoutMs);
  const out: string[] = [];
  try {
    const res = await runProcess(ctx.spawner, command, args, {
      cwd: ctx.home,
      env,
      signal: ac.signal,
      killGraceMs: 500,
      onStdoutLine: (l) => out.push(l),
    });
    return {
      ok: !timedOut && res.code === 0,
      code: res.code,
      timedOut,
      spawnFailed: res.signal?.startsWith('spawn-error') ?? false,
      stdout: out.join('\n'),
      stderr: res.stderrTail,
    };
  } catch (err) {
    return { ok: false, code: null, timedOut, spawnFailed: true, stdout: out.join('\n'), stderr: String((err as Error).message) };
  } finally {
    clearTimeout(timer);
  }
}

const cleanEnv = (env: Record<string, string | undefined>): Record<string, string> =>
  Object.fromEntries(Object.entries(env).filter((e): e is [string, string] => e[1] !== undefined));

// ---------------------------------------------------------------- PATH

/** Directories where the CLIs usually live, that exist on this machine. Exported for tests. */
export function fallbackPathDirs(ctx: Pick<Ctx, 'fs' | 'env' | 'home' | 'platform'>): string[] {
  const { fs, env, home } = ctx;
  const dirs: string[] = [
    '/opt/homebrew/bin',
    '/opt/homebrew/sbin',
    '/usr/local/bin',
    join(home, '.local', 'bin'),
    join(home, '.npm-global', 'bin'),
    join(home, '.claude', 'local'),
    join(home, '.volta', 'bin'),
    join(home, '.asdf', 'shims'),
    join(home, '.bun', 'bin'),
    join(home, 'Library', 'pnpm'),
    join(home, '.local', 'share', 'pnpm'),
  ];
  if (env['NVM_BIN']) dirs.push(env['NVM_BIN']);
  if (env['VOLTA_HOME']) dirs.push(join(env['VOLTA_HOME'], 'bin'));
  if (env['PNPM_HOME']) dirs.push(env['PNPM_HOME']);
  if (env['ASDF_DATA_DIR']) dirs.push(join(env['ASDF_DATA_DIR'], 'shims'));
  if (env['BUN_INSTALL']) dirs.push(join(env['BUN_INSTALL'], 'bin'));
  if (env['npm_config_prefix']) dirs.push(join(env['npm_config_prefix'], 'bin'));
  // nvm and fnm keep one directory per node version: newest first.
  const newestFirst = (names: string[]): string[] =>
    [...names].sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
  for (const v of newestFirst(fs.listDir(join(home, '.nvm', 'versions', 'node')))) dirs.push(join(home, '.nvm', 'versions', 'node', v, 'bin'));
  for (const v of newestFirst(fs.listDir(join(home, '.fnm', 'node-versions')))) dirs.push(join(home, '.fnm', 'node-versions', v, 'installation', 'bin'));
  dirs.push('/usr/bin', '/bin');
  return dirs.filter((d) => fs.exists(d));
}

export interface LoginPath {
  /** Login-shell PATH, then the process PATH, then the usual install directories; no duplicates. */
  path: string;
  /** Whether the login shell answered. When false only the process PATH and the fallbacks are in `path`. */
  fromLoginShell: boolean;
}

function parseMarked(stdout: string): string | null {
  const end = stdout.lastIndexOf(MARK);
  if (end < 0) return null;
  const start = stdout.lastIndexOf(MARK, end - 1);
  if (start < 0) return null;
  const value = stdout.slice(start + MARK.length, end).trim();
  return value === '' ? null : value;
}

/**
 * The PATH the user's terminal would have. Runs `$SHELL -lc 'echo $PATH'` (markers around the value, so noise that rc files
 * print is ignored; fish gets `string join :` because its `$PATH` is a list), with a timeout, and falls back to the process
 * PATH plus the common install directories when the shell is missing, slow or prints nothing.
 */
export async function resolveLoginPath(opts: DetectOptions = {}): Promise<LoginPath> {
  const ctx = ctxOf(opts);
  const processPath = (ctx.env['PATH'] ?? '').split(delimiter).filter(Boolean);
  let login: string[] = [];
  if (ctx.platform !== 'win32' && ctx.shell) {
    const fish = basename(ctx.shell) === 'fish';
    const script = fish ? `printf '%s%s%s' ${MARK} (string join : $PATH) ${MARK}` : `printf '%s' "${MARK}$PATH${MARK}"`;
    const res = await probe(ctx, ctx.shell, ['-lc', script], cleanEnv(ctx.env), ctx.shellTimeoutMs);
    const value = res.timedOut || res.spawnFailed ? null : parseMarked(res.stdout);
    if (value) login = value.split(delimiter).filter(Boolean);
  }
  const fromLoginShell = login.length > 0;
  const merged = [...new Set([...login, ...processPath, ...fallbackPathDirs(ctx)])];
  return { path: merged.join(delimiter), fromLoginShell };
}

function findOnPath(name: string, path: string, fs: DetectFs): string | null {
  for (const dir of path.split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, name);
    if (fs.isExecutable(candidate)) return candidate;
  }
  return null;
}

// ---------------------------------------------------------------- version + auth

const SEMVER = /\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?/;

export function parseVersion(output: string): string | null {
  const first = output.split('\n').find((l) => l.trim() !== '') ?? '';
  return SEMVER.exec(first)?.[0] ?? SEMVER.exec(output)?.[0] ?? (first.trim().slice(0, 40) || null);
}

/** Variables that would make a status command report the key instead of the stored login. */
const KEY_VARS: Record<HarnessKind, string[]> = {
  'claude-code': ['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN'],
  codex: ['OPENAI_API_KEY', 'CODEX_API_KEY'],
};

const hasKey = (env: Record<string, string | undefined>, kind: HarnessKind): boolean => KEY_VARS[kind].some((k) => !!env[k]);

const withoutKeys = (env: Record<string, string>, kind: HarnessKind): Record<string, string> => {
  const out = { ...env };
  for (const k of KEY_VARS[kind]) delete out[k];
  return out;
};

type AuthSignal = { auth: HarnessAuth; via: string } | null;

async function claudeAuth(ctx: Ctx, binary: string, env: Record<string, string>): Promise<AuthSignal> {
  // 1. The CLI itself (`claude auth status` prints JSON and exits non-zero when signed out). Key variables are removed so the
  //    answer is about the stored login, not about the environment.
  const status = await probe(ctx, binary, ['auth', 'status'], withoutKeys(env, 'claude-code'), ctx.timeoutMs);
  if (!status.spawnFailed && !status.timedOut) {
    const parsed = parseJsonObject(status.stdout);
    if (parsed && typeof parsed['loggedIn'] === 'boolean') {
      if (parsed['loggedIn']) {
        const method = String(parsed['authMethod'] ?? '');
        return /api[-_ ]?key/i.test(method) ? { auth: 'api-key', via: 'claude auth status' } : { auth: 'signed-in', via: 'claude auth status' };
      }
      // Definitive: the CLI says there is no login. An API key in the environment may still work.
      return hasKey(ctx.env, 'claude-code') ? { auth: 'api-key', via: 'ANTHROPIC_API_KEY' } : { auth: 'signed-out', via: 'claude auth status' };
    }
  }
  // 2. Credential presence, the way the CLI stores them.
  const configDir = ctx.env['CLAUDE_CONFIG_DIR'] || join(ctx.home, '.claude');
  if (ctx.fs.exists(join(configDir, '.credentials.json'))) return { auth: 'signed-in', via: 'credentials file' };
  if (ctx.platform === 'darwin') {
    // Without -w / -g `security` prints the item's attributes and never fetches the secret (no keychain prompt).
    const kc = await probe(ctx, '/usr/bin/security', ['find-generic-password', '-s', 'Claude Code-credentials'], env, ctx.timeoutMs);
    if (kc.ok) return { auth: 'signed-in', via: 'keychain' };
  }
  // 3. API key variables.
  if (hasKey(ctx.env, 'claude-code')) return { auth: 'api-key', via: 'ANTHROPIC_API_KEY' };
  return null;
}

async function codexAuth(ctx: Ctx, binary: string, env: Record<string, string>): Promise<AuthSignal> {
  // 1. `codex login status`: "Logged in using ChatGPT" / "API key configured - sk-***" / "Not logged in" (on stderr, exit 1).
  const status = await probe(ctx, binary, ['login', 'status'], withoutKeys(env, 'codex'), ctx.timeoutMs);
  if (!status.spawnFailed && !status.timedOut) {
    const text = `${status.stdout}\n${status.stderr}`;
    if (/not logged in/i.test(text)) {
      return hasKey(ctx.env, 'codex') ? { auth: 'api-key', via: 'OPENAI_API_KEY' } : { auth: 'signed-out', via: 'codex login status' };
    }
    if (status.ok) {
      if (/chatgpt/i.test(text)) return { auth: 'signed-in', via: 'codex login status' };
      if (/api key/i.test(text)) return { auth: 'api-key', via: 'codex login status' };
      return { auth: 'signed-in', via: 'codex login status' };
    }
  }
  // 2. ~/.codex/auth.json present (its contents are not read, so ChatGPT login and stored key look the same).
  const home = ctx.env['CODEX_HOME'] || join(ctx.home, '.codex');
  if (ctx.fs.exists(join(home, 'auth.json'))) return { auth: 'signed-in', via: 'auth.json' };
  if (hasKey(ctx.env, 'codex')) return { auth: 'api-key', via: 'OPENAI_API_KEY' };
  return null;
}

function parseJsonObject(text: string): Record<string, unknown> | null {
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start < 0 || end < start) return null;
  try {
    const v: unknown = JSON.parse(text.slice(start, end + 1));
    return typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------- per-harness

interface HarnessSpec {
  binary: string;
  label: string;
  install: string;
  signIn: string;
  authenticate(ctx: Ctx, binary: string, env: Record<string, string>): Promise<AuthSignal>;
}

const SPECS: Record<HarnessKind, HarnessSpec> = {
  'claude-code': {
    binary: 'claude',
    label: 'Claude Code',
    install: 'Install it with `npm install -g @anthropic-ai/claude-code` (see https://docs.claude.com/en/docs/claude-code/setup), then sign in with `claude`.',
    signIn: 'Run `claude` once and sign in (/login), or set ANTHROPIC_API_KEY.',
    authenticate: claudeAuth,
  },
  codex: {
    binary: 'codex',
    label: 'Codex',
    install: 'Install it with `npm install -g @openai/codex` or `brew install --cask codex`, then run `codex login`.',
    signIn: 'Run `codex login` (ChatGPT account or API key), or set OPENAI_API_KEY.',
    authenticate: codexAuth,
  },
};

const AUTH_NOTE: Record<HarnessAuth, (label: string, via: string) => string> = {
  'signed-in': (label, via) => `${label} is signed in on this machine (${via}); agents use your own login.`,
  'api-key': (label, via) => `${label} will use the API key from ${via}.`,
  'signed-out': (label) => `${label} is not signed in.`,
};

async function detectOne(kind: HarnessKind, ctx: Ctx, searchPath: string): Promise<HarnessInfo> {
  const spec = SPECS[kind];
  const path = findOnPath(spec.binary, searchPath, ctx.fs);
  if (!path) {
    return { kind, installed: false, version: null, path: null, auth: 'signed-out', hint: `${spec.label} was not found on PATH. ${spec.install}` };
  }
  const env = { ...cleanEnv(ctx.env), PATH: searchPath };
  // The shebang of an npm-installed CLI is `#!/usr/bin/env node`: it needs the resolved PATH too.
  const v = await probe(ctx, path, ['--version'], env, ctx.timeoutMs);
  if (!v.ok) {
    const why = v.timedOut ? 'timed out' : v.spawnFailed ? `cannot run it (${v.stderr || 'spawn failed'})` : `exited with ${v.code}${v.stderr ? `: ${v.stderr.split('\n')[0]}` : ''}`;
    return { kind, installed: false, version: null, path, auth: 'signed-out', hint: `Found ${spec.label} at ${path} but \`--version\` ${why}. Reinstall it. ${spec.install}` };
  }
  const version = parseVersion(v.stdout);
  const signal = await spec.authenticate(ctx, path, env);
  const auth = signal?.auth ?? 'signed-out';
  const hint = auth === 'signed-out' ? `${AUTH_NOTE['signed-out'](spec.label, '')} ${spec.signIn}` : AUTH_NOTE[auth](spec.label, signal!.via);
  return { kind, installed: true, version, path, auth, hint };
}

/**
 * Probe this machine for the Claude Code and Codex CLIs. Never throws: a probe that fails or times out yields a result with a
 * hint. Results come back in a stable order (claude-code, codex).
 */
export async function detectHarnesses(opts: DetectOptions = {}): Promise<HarnessInfo[]> {
  const ctx = ctxOf(opts);
  const searchPath = opts.loginPath ?? (await resolveLoginPath(opts)).path;
  const kinds = opts.kinds ?? (['claude-code', 'codex'] as HarnessKind[]);
  const order: HarnessKind[] = ['claude-code', 'codex'];
  return Promise.all(order.filter((k) => kinds.includes(k)).map((k) => detectOne(k, ctx, searchPath)));
}

// ---------------------------------------------------------------- environment for local agents

/** Variables that carry credentials the agents must not inherit from the app: cloud, git hosting, registries, model keys. */
const SECRET_NAME = /(^|_)(API_?KEY|TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIALS?|AUTH)(_|$)|^AWS_|^AZURE_|^GOOGLE_APPLICATION_CREDENTIALS$|^NPM_CONFIG__AUTH/i;

/** Model key variable(s) each harness uses when it runs on an API key. */
export const MODEL_KEY_VARS: Record<HarnessKind, string[]> = {
  'claude-code': ['ANTHROPIC_API_KEY'],
  codex: ['OPENAI_API_KEY', 'CODEX_API_KEY'],
};

/**
 * The environment a local agent process gets: the user's own environment (HOME, so the CLI finds its login) with the login-shell
 * PATH, minus anything that looks like a credential (AWS, GitHub and registry tokens, every `*_API_KEY`). The model key of the
 * harness is added back only when it reports auth `api-key` and the key is set; with a `signed-in` harness no key is passed, so
 * the CLI uses the user's login rather than an API key that happens to be in the environment.
 */
export function localAgentEnv(info: Pick<HarnessInfo, 'kind' | 'auth'>, base: Record<string, string | undefined>, path: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(base)) {
    if (v === undefined || SECRET_NAME.test(k)) continue;
    env[k] = v;
  }
  env['PATH'] = path;
  if (info.auth === 'api-key') {
    for (const k of MODEL_KEY_VARS[info.kind]) if (base[k]) env[k] = base[k]!;
  }
  return env;
}
