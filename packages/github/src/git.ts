import { execFile } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import type { CandidateCommit, CommitFilesInput, FileDiff, GitHost } from '@ratchet/core';

export interface GitRunOptions {
  cwd?: string;
  /** Extra environment variables (merged over `process.env`). Secrets go here, never in argv. */
  env?: Record<string, string>;
  /** Written to the process's stdin. */
  input?: string;
}

/** Runs `git <args>` and resolves with stdout. Rejects on a non-zero exit. */
export type GitRunner = (args: string[], opts?: GitRunOptions) => Promise<{ stdout: string }>;

/** Default runner: the real git CLI. Strings in `redact` are scrubbed from error messages. */
export function createGitRunner(redact: string[] = []): GitRunner {
  const scrub = (s: string) => redact.filter(Boolean).reduce((acc, secret) => acc.split(secret).join('***'), s);
  return (args, opts = {}) =>
    new Promise((resolve, reject) => {
      const child = execFile(
        'git',
        args,
        {
          cwd: opts.cwd,
          env: { ...process.env, GIT_TERMINAL_PROMPT: '0', ...opts.env },
          maxBuffer: 256 * 1024 * 1024,
        },
        (err, stdout, stderr) => {
          if (err) {
            reject(new Error(scrub(`git ${args.join(' ')} failed: ${String(stderr || err.message).trim()}`)));
          } else resolve({ stdout });
        },
      );
      if (opts.input !== undefined) child.stdin?.end(opts.input);
    });
}

export const CANDIDATE_TRAILER = 'Ratchet-Candidate';

/** Extra inputs for `openPullRequest` beyond the `GitHost` port. */
export interface PullRequestInput {
  base: string;
  /** Branch to open the PR from, e.g. `ratchet/<opt>/result`. */
  head: string;
  title: string;
  body: string;
  /**
   * Verified SHAs in ratchet order. When given, `head` is created as one squashed commit whose tree is
   * the final SHA's tree (later wins are rebased onto earlier ones). When omitted, `head` must already exist.
   */
  shas?: string[];
  /** Squash commit message; defaults to `title`. */
  commitMessage?: string;
}

export interface BaseGitHostOptions {
  runner?: GitRunner;
  /** Author/committer for the squash commit. */
  identity?: { name: string; email: string };
}

const SHA_RE = /^[0-9a-f]{40}$/;
const US = '\x1f';
const RS = '\x1e';

/** Shared git logic over a (bare) repository `gitDir`; subclasses decide where branches live and how they sync. */
export abstract class BaseGitHost implements GitHost {
  protected readonly runner: GitRunner;
  private readonly identity: { name: string; email: string };

  constructor(options: BaseGitHostOptions = {}) {
    this.runner = options.runner ?? createGitRunner();
    this.identity = options.identity ?? { name: 'Ratchet', email: 'ratchet@users.noreply.github.com' };
  }

  abstract remoteUrl(): string;
  /** Directory of the (bare) repo that git commands operate on. */
  protected abstract gitDir(): Promise<string>;
  /** Namespace holding the remote's branches inside `gitDir`. */
  protected abstract readonly branchPrefix: string;
  /** Bring `gitDir` up to date with the remote before reading. */
  protected async sync(): Promise<void> {}
  /** Make `ref` (a local SHA) visible as `refs/heads/<branch>` on the remote. */
  protected abstract publish(branch: string, sha: string, opts: { force: boolean; expected?: string }): Promise<void>;
  protected abstract createPullRequest(input: PullRequestInput): Promise<{ url: string }>;

  protected async git(args: string[], extra: GitRunOptions = {}): Promise<string> {
    const dir = await this.gitDir();
    const { stdout } = await this.runner(['--git-dir', dir, ...args], extra);
    return stdout;
  }

  /** Resolve a branch name, SHA or ref to a full commit SHA. */
  protected async resolve(ref: string): Promise<string> {
    if (SHA_RE.test(ref)) return ref;
    const attempts = ref.startsWith('refs/') ? [ref] : [`${this.branchPrefix}${ref}`, ref];
    let lastErr: unknown;
    for (const candidate of attempts) {
      try {
        return (await this.git(['rev-parse', '--verify', '--quiet', `${candidate}^{commit}`])).trim();
      } catch (err) {
        lastErr = err;
      }
    }
    throw new Error(`cannot resolve ref "${ref}"${lastErr ? `: ${(lastErr as Error).message}` : ''}`);
  }

  /** GitHost port: resolve a branch, tag or sha to a full commit sha. */
  async resolveRef(ref: string): Promise<string> {
    await this.sync();
    return this.resolve(ref);
  }

  async createBranch(fromRef: string, branch: string): Promise<void> {
    await this.sync();
    const sha = await this.resolve(fromRef);
    await this.publish(branch, sha, { force: false });
  }

  async listCandidates(branch: string, sinceSha?: string): Promise<CandidateCommit[]> {
    await this.sync();
    const tip = await this.resolve(branch);
    const range = sinceSha ? `${await this.resolve(sinceSha)}..${tip}` : tip;
    // %(trailers:...) matches keys case-insensitively and understands real trailer blocks only.
    const format = [
      '%H',
      '%ct',
      `%(trailers:key=${CANDIDATE_TRAILER},valueonly,separator=%x2C)`,
      '%B',
    ].join(US);
    const out = await this.git(['log', '--reverse', `--format=${format}${RS}`, range]);
    const commits: CandidateCommit[] = [];
    for (const record of out.split(RS)) {
      const rec = record.replace(/^\n/, '');
      if (!rec.trim()) continue;
      const [sha, ct, trailerValues = '', ...rest] = rec.split(US);
      if (!sha || !ct) continue;
      // if the trailer is repeated, the last one wins
      const last = trailerValues.split(/[,\n]/).map((v) => v.trim()).filter(Boolean).at(-1);
      if (last?.toLowerCase() !== 'true') continue;
      commits.push({ sha, branch, message: rest.join(US).trim(), committedAt: Number(ct) * 1000 });
    }
    return commits;
  }

  async changedFiles(baseRef: string, sha: string): Promise<string[]> {
    await this.sync();
    const base = await this.resolve(baseRef);
    const head = await this.resolve(sha);
    const out = await this.git(['diff', '--name-only', '-z', base, head]);
    return out.split('\0').filter(Boolean);
  }

  async diff(baseRef: string, sha: string): Promise<FileDiff[]> {
    await this.sync();
    const base = await this.resolve(baseRef);
    const head = await this.resolve(sha);
    let from = base;
    try {
      from = (await this.git(['merge-base', base, head])).trim() || base;
    } catch {
      // unrelated histories: compare the trees directly
    }
    const opts = ['--no-color', '--no-ext-diff', '--no-renames'];
    const numstat = await this.git(['diff', ...opts, '--numstat', '-z', from, head]);
    const patch = await this.git(['diff', ...opts, '-U3', from, head]);
    return parseGitDiff(numstat, patch);
  }

  /** Materialize the tree of `sha` into `dir` (no `.git`, no side effects on the repo). */
  async checkout(sha: string, dir: string): Promise<void> {
    await this.sync();
    const commit = await this.resolve(sha);
    mkdirSync(dir, { recursive: true });
    const scratch = mkdtempSync(join(tmpdir(), 'ratchet-index-'));
    try {
      const env = { GIT_INDEX_FILE: join(scratch, 'index'), GIT_WORK_TREE: resolve(dir) };
      await this.git(['read-tree', commit], { env, cwd: resolve(dir) });
      await this.git(['checkout-index', '-a', '-f'], { env, cwd: resolve(dir) });
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }

  async openPullRequest(input: PullRequestInput): Promise<{ url: string }> {
    await this.sync();
    if (input.shas && input.shas.length > 0) await this.writeSquash(input);
    return this.createPullRequest(input);
  }

  /**
   * Port method: branch `input.branch` off `input.baseRef` with one commit that writes `input.files` over the base tree.
   * Built with plumbing (a scratch index, `hash-object`, `write-tree`, `commit-tree`), so no working tree is needed and
   * nothing but the new ref is touched. Refuses an existing branch (`publish` is not forced) and unsafe paths.
   */
  async commitFiles(input: CommitFilesInput): Promise<{ sha: string }> {
    if (input.files.length === 0) throw new Error('commitFiles: no files');
    const seen = new Set<string>();
    for (const f of input.files) {
      const parts = f.path.split('/');
      const unsafe =
        !f.path ||
        f.path.includes('\\') ||
        f.path.includes('\0') ||
        parts.some((x) => x === '' || x === '.' || x === '..' || x.toLowerCase() === '.git');
      if (unsafe) throw new Error(`commitFiles: unsafe path ${JSON.stringify(f.path)}`);
      if (seen.has(f.path)) throw new Error(`commitFiles: duplicate path ${f.path}`);
      seen.add(f.path);
    }
    await this.sync();
    const base = await this.resolve(input.baseRef);
    if (input.update) {
      // Straight onto an existing branch (a protected ref): only on top of the tip the caller based the files on.
      const tip = await this.resolve(input.branch).catch(() => {
        throw new Error(`commitFiles: branch ${input.branch} does not exist`);
      });
      if (tip !== base) throw new Error(`commitFiles: ${input.branch} is not at ${input.baseRef}; refusing to move it`);
    }
    const scratch = mkdtempSync(join(tmpdir(), 'ratchet-commit-'));
    try {
      const env = { GIT_INDEX_FILE: join(scratch, 'index') };
      await this.git(['read-tree', base], { env });
      for (const f of input.files) {
        const blob = (await this.git(['hash-object', '-w', '--stdin'], { env, input: f.content })).trim();
        await this.git(['update-index', '--add', '--cacheinfo', `${f.executable ? '100755' : '100644'},${blob},${f.path}`], { env });
      }
      const tree = (await this.git(['write-tree'], { env })).trim();
      const id = this.identity;
      const commitEnv = {
        GIT_AUTHOR_NAME: id.name,
        GIT_AUTHOR_EMAIL: id.email,
        GIT_COMMITTER_NAME: id.name,
        GIT_COMMITTER_EMAIL: id.email,
      };
      const message = `${input.message.replace(/\s+$/, '')}\n`;
      const sha = (await this.git(['commit-tree', tree, '-p', base, '-F', '-'], { env: commitEnv, input: message })).trim();
      await this.publish(input.branch, sha, input.update ? { force: false, expected: base } : { force: false });
      return { sha };
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }

  /** Create `input.head` as a single commit holding the final verified tree, parented at the merge-base with `base`. */
  private async writeSquash(input: PullRequestInput): Promise<void> {
    const shas = await Promise.all(input.shas!.map((s) => this.resolve(s)));
    const final = shas[shas.length - 1]!;
    const base = await this.resolve(input.base);
    let parent: string;
    try {
      parent = (await this.git(['merge-base', base, final])).trim();
    } catch {
      throw new Error(`verified commit ${final} shares no history with ${input.base}`);
    }
    const tree = (await this.git(['rev-parse', `${final}^{tree}`])).trim();
    const id = this.identity;
    const env = {
      GIT_AUTHOR_NAME: id.name,
      GIT_AUTHOR_EMAIL: id.email,
      GIT_COMMITTER_NAME: id.name,
      GIT_COMMITTER_EMAIL: id.email,
    };
    const message = `${(input.commitMessage ?? input.title).replace(/\s+$/, '')}\n`;
    const commit = (await this.git(['commit-tree', tree, '-p', parent, '-F', '-'], { env, input: message })).trim();
    await this.publish(input.head, commit, { force: true });
  }
}

/**
 * Combine `git diff --numstat -z` (paths and counts, robust to odd file names) with the unified `git diff` of the
 * same range (hunks). Both list files in the same order, so sections are matched by position.
 */
export function parseGitDiff(numstat: string, patch: string): FileDiff[] {
  const fields = numstat.split('\0');
  const stats: { path: string; additions: number; deletions: number; binary: boolean }[] = [];
  for (const f of fields) {
    const m = /^(-|\d+)\t(-|\d+)\t([\s\S]+)$/.exec(f.replace(/^\n/, ''));
    if (!m) continue;
    const binary = m[1] === '-' && m[2] === '-';
    stats.push({ path: m[3]!, additions: binary ? 0 : Number(m[1]), deletions: binary ? 0 : Number(m[2]), binary });
  }
  const sections = patch.split(/^diff --git .*$/m).slice(1);
  return stats.map((s, i) => {
    const body = sections[i] ?? '';
    const at = body.search(/^@@/m);
    return { ...s, patch: s.binary || at < 0 ? '' : body.slice(at).replace(/\n$/, '') };
  });
}
