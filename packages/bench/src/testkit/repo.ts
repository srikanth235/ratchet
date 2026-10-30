import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { parseConfig, type ArtifactStore, type Clock, type GitHost, type RatchetConfig } from '@ratchet/core';

/** Test helpers: a tiny git repo with a node bench script, a local GitHost, a Clock and a config. */

export const realClock: Clock = {
  now: () => Date.now(),
  sleep: (ms, signal) =>
    new Promise((resolve) => {
      const t = setTimeout(resolve, ms);
      signal?.addEventListener('abort', () => (clearTimeout(t), resolve()), { once: true });
    }),
};

/** A Clock whose sleeps return immediately. */
export const instantClock: Clock = { now: () => Date.now(), sleep: async () => undefined };

export function tmp(prefix = 'ratchet-bench-test-'): string {
  return mkdtempSync(join(tmpdir(), prefix));
}

export function rmrf(dir: string): void {
  rmSync(dir, { recursive: true, force: true });
}

export function git(cwd: string, ...args: string[]): string {
  return execFileSync('git', args, { cwd, encoding: 'utf8', env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_SYSTEM: '/dev/null' } }).trim();
}

/** GitHost that materializes any ref with `git archive` (no .git in the result). */
export class LocalGitHost implements GitHost {
  checkouts: Array<{ ref: string; dir: string }> = [];
  constructor(readonly repo: string) {}
  async createBranch(fromRef: string, branch: string): Promise<void> {
    git(this.repo, 'branch', branch, fromRef);
  }
  async listCandidates(): Promise<never[]> {
    return [];
  }
  async changedFiles(baseRef: string, sha: string): Promise<string[]> {
    return git(this.repo, 'diff', '--name-only', baseRef, sha).split('\n').filter(Boolean);
  }
  async checkout(ref: string, dir: string): Promise<void> {
    this.checkouts.push({ ref, dir });
    mkdirSync(dir, { recursive: true });
    execFileSync('sh', ['-c', `git -C "$1" archive "$2" | tar -x -C "$3"`, 'sh', this.repo, ref, dir], { stdio: 'pipe' });
  }
  async resolveRef(ref: string): Promise<string> {
    return git(this.repo, 'rev-parse', '--verify', `${ref}^{commit}`);
  }
  async openPullRequest(): Promise<{ url: string }> {
    return { url: 'https://example.invalid/pr/1' };
  }
  remoteUrl(): string {
    return this.repo;
  }
}

export const BENCH_MJS = `
import { appendFileSync, existsSync, readFileSync } from 'node:fs';
const num = (f) => Number(readFileSync('src/' + f, 'utf8'));
const marker = process.env.MARKER;
let n = 1;
if (marker) {
  appendFileSync(marker, process.pid + '\\n');
  n = readFileSync(marker, 'utf8').trim().split('\\n').length;
}
const trace = process.env.RATCHET_TRACE;
const out = {
  http_req_duration: { p95: num('p95.txt') + (process.env.JITTER ? (n - 1) * 0.1 : 0), p50: 1 },
  error_rate: num('err.txt'),
  peak_rss_mb: num('rss.txt'),
  trace_set: trace ? 1 : 0,
  trace_bytes: trace ? readFileSync(trace).length : 0,
  run: Number(process.env.RATCHET_RUN ?? 0),
  phase_is_confirm: process.env.RATCHET_PHASE === 'confirm' ? 1 : 0,
  label: 'text',
  arr: [1, 2],
};
if (existsSync('src/hang.txt')) await new Promise((r) => setTimeout(r, 60000));
console.log('some log line');
console.log(JSON.stringify(out));
`;

export const CHECK_MJS = `
import { readFileSync } from 'node:fs';
if (readFileSync('src/p95.txt', 'utf8').includes('BROKEN')) { console.error('check failed: broken'); process.exit(1); }
`;

export const BASE_FILES: Record<string, string> = {
  'bench/bench.mjs': BENCH_MJS,
  'bench/data/trace-notes.txt': 'trusted data\n',
  'tests/check.mjs': CHECK_MJS,
  'src/p95.txt': '100',
  'src/rss.txt': '100',
  'src/err.txt': '0',
  'ratchet.yaml': '# placeholder\n',
};

export interface FixtureRepo {
  dir: string;
  repo: string;
  host: LocalGitHost;
  /** Commit sha of the base on branch `main` and of the trusted bench on branch `ratchet/bench`. */
  base: string;
  /** Create a commit on top of base with these file changes (null deletes); returns its sha. */
  candidate(changes: Record<string, string | null | { symlink: string }>, name?: string): string;
  cleanup(): void;
}

export function makeFixtureRepo(): FixtureRepo {
  const dir = tmp();
  const repo = join(dir, 'repo');
  mkdirSync(repo);
  git(repo, 'init', '-q', '-b', 'main');
  git(repo, 'config', 'user.email', 'test@example.invalid');
  git(repo, 'config', 'user.name', 'Test');
  git(repo, 'config', 'commit.gpgsign', 'false');
  for (const [f, c] of Object.entries(BASE_FILES)) {
    mkdirSync(dirname(join(repo, f)), { recursive: true });
    writeFileSync(join(repo, f), c);
  }
  git(repo, 'add', '-A');
  git(repo, 'commit', '-q', '-m', 'base');
  const base = git(repo, 'rev-parse', 'HEAD');
  git(repo, 'branch', 'ratchet/bench', base);
  let n = 0;
  return {
    dir,
    repo,
    host: new LocalGitHost(repo),
    base,
    candidate(changes, name = `cand-${++n}`) {
      git(repo, 'checkout', '-q', '-B', name, base);
      for (const [f, c] of Object.entries(changes)) {
        const p = join(repo, f);
        if (c === null) {
          rmSync(p, { recursive: true, force: true });
          continue;
        }
        rmSync(p, { recursive: true, force: true });
        mkdirSync(dirname(p), { recursive: true });
        if (typeof c === 'string') writeFileSync(p, c);
        else execFileSync('ln', ['-s', c.symlink, p]);
      }
      git(repo, 'add', '-A');
      git(repo, 'commit', '-q', '--allow-empty', '-m', name);
      const sha = git(repo, 'rev-parse', 'HEAD');
      git(repo, 'checkout', '-q', 'main');
      return sha;
    },
    cleanup: () => rmrf(dir),
  };
}

export interface ConfigOptions {
  guardrails?: string[];
  protectedGlobs?: string[];
  repeats?: number;
  run?: string;
  confirmTrace?: string;
}

export function makeConfig(o: ConfigOptions = {}): RatchetConfig {
  const guardrails = o.guardrails ?? ['- run: node tests/check.mjs', '- error_rate: "< 0.001"', '- peak_rss: "< 1.2x"'];
  const globs = o.protectedGlobs ?? ['bench/**', 'ratchet.yaml', 'tests/**'];
  const yaml = `version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: ${JSON.stringify(o.run ?? 'node bench/bench.mjs')}
  repeats: ${o.repeats ?? 3}
  hardware: c7i.2xlarge
${o.confirmTrace ? `  confirm_trace: ${o.confirmTrace}\n` : ''}guardrails:${guardrails.length ? '\n' + guardrails.map((g) => '  ' + g).join('\n') : ' []'}
protected:
${globs.map((g) => `  - "${g}"`).join('\n')}
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge]
`;
  const res = parseConfig(yaml);
  if (!res.ok) throw new Error('bad test config: ' + JSON.stringify(res.errors));
  return res.config;
}

export function listDir(dir: string): string[] {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

/** In-memory ArtifactStore. */
export class MemoryStore {
  files = new Map<string, string>();
  async put(key: string, body: string | Uint8Array): Promise<void> {
    this.files.set(key, typeof body === 'string' ? body : Buffer.from(body).toString('utf8'));
  }
  async get(key: string): Promise<string | null> {
    return this.files.get(key) ?? null;
  }
  async list(prefix: string): Promise<string[]> {
    return [...this.files.keys()].filter((k) => k.startsWith(prefix));
  }
}

/** Is the process running? Zombies (killed, not yet reaped by a lazy init) count as dead. */
export function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
  } catch {
    return false;
  }
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, 'utf8');
    return stat.slice(stat.lastIndexOf(')') + 2, stat.lastIndexOf(')') + 3) !== 'Z';
  } catch {
    return true; // no /proc (not Linux): trust kill(0)
  }
}

/** ArtifactStore that writes `<root>/<key>`; point `root` at `<s3Dir>/<bucket>` and the fake `aws s3 cp` finds the objects. */
export class DirStore implements ArtifactStore {
  puts: string[] = [];
  constructor(private readonly root: string) {}
  async put(key: string, body: string | Uint8Array): Promise<void> {
    const p = join(this.root, key);
    mkdirSync(dirname(p), { recursive: true });
    writeFileSync(p, body);
    this.puts.push(key);
  }
  async get(key: string): Promise<string | null> {
    try {
      return readFileSync(join(this.root, key), 'utf8');
    } catch {
      return null;
    }
  }
  async list(prefix: string): Promise<string[]> {
    return this.puts.filter((k) => k.startsWith(prefix)).sort();
  }
}
