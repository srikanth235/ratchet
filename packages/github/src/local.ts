import { cpSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { BaseGitHost, createGitRunner, type BaseGitHostOptions, type GitRunner, type PullRequestInput } from './git.js';

export interface LocalGitHostOptions extends BaseGitHostOptions {
  /** Path of the bare repository acting as "GitHub". */
  remoteDir: string;
  /** Where PR bodies are written (`<head with / as __>.md` + `.json`). Defaults to `<remoteDir>/ratchet-prs`. */
  prDir?: string;
}

/** Simulation's GitHub: the real git CLI against a local bare repo. Branches live in `refs/heads/*`. */
export class LocalGitHost extends BaseGitHost {
  protected readonly branchPrefix = 'refs/heads/';
  private readonly remoteDir: string;
  private readonly prDir: string;

  constructor(options: LocalGitHostOptions) {
    super(options);
    this.remoteDir = resolve(options.remoteDir);
    this.prDir = resolve(options.prDir ?? join(this.remoteDir, 'ratchet-prs'));
  }

  remoteUrl(): string {
    return pathToFileURL(this.remoteDir).href;
  }

  protected async gitDir(): Promise<string> {
    return this.remoteDir;
  }

  protected async publish(branch: string, sha: string, opts: { force: boolean }): Promise<void> {
    const ref = `refs/heads/${branch}`;
    // old value of all zeros = "must not exist yet"
    const args = opts.force ? ['update-ref', ref, sha] : ['update-ref', ref, sha, '0'.repeat(40)];
    await this.git(args);
  }

  protected async createPullRequest(input: PullRequestInput): Promise<{ url: string }> {
    mkdirSync(this.prDir, { recursive: true });
    const stem = input.head.replace(/[\\/]/g, '__');
    const bodyPath = join(this.prDir, `${stem}.md`);
    writeFileSync(bodyPath, `# ${input.title}\n\n${input.body}\n`);
    writeFileSync(
      join(this.prDir, `${stem}.json`),
      JSON.stringify({ base: input.base, head: input.head, title: input.title, shas: input.shas ?? [] }, null, 2),
    );
    return { url: pathToFileURL(bodyPath).href };
  }
}

export interface CreateBareRemoteOptions {
  /** Directory to create the bare repo in; defaults to a fresh temp dir. */
  remoteDir?: string;
  /** Working checkout location; defaults to a fresh temp dir. */
  workDir?: string;
  branch?: string;
  message?: string;
  runner?: GitRunner;
}

export interface BareRemote {
  remoteDir: string;
  /** Clone of the remote an agent (or test) commits in; `origin` points at `remoteDir`. */
  workDir: string;
  branch: string;
  /** Commit holding the initial copy of the source. */
  baseSha: string;
}

/** Copy `srcDir` (minus .git/node_modules/dist) into a fresh repo, commit it and push it to a new bare remote. */
export async function createBareRemote(srcDir: string, options: CreateBareRemoteOptions = {}): Promise<BareRemote> {
  const run = options.runner ?? createGitRunner();
  const branch = options.branch ?? 'main';
  const root = options.remoteDir && options.workDir ? undefined : mkdtempSync(join(tmpdir(), 'ratchet-remote-'));
  const remoteDir = resolve(options.remoteDir ?? join(root!, 'remote.git'));
  const workDir = resolve(options.workDir ?? join(root!, 'work'));
  const env = {
    GIT_AUTHOR_NAME: 'Ratchet',
    GIT_AUTHOR_EMAIL: 'ratchet@localhost',
    GIT_COMMITTER_NAME: 'Ratchet',
    GIT_COMMITTER_EMAIL: 'ratchet@localhost',
  };
  mkdirSync(workDir, { recursive: true });
  cpSync(srcDir, workDir, {
    recursive: true,
    filter: (p) => !/(^|[\\/])(\.git|node_modules|dist)$/.test(p),
  });
  await run(['init', '-q', '-b', branch, workDir]);
  await run(['add', '-A'], { cwd: workDir });
  await run(['-c', 'commit.gpgsign=false', 'commit', '-q', '-m', options.message ?? 'Initial import'], { cwd: workDir, env });
  const baseSha = (await run(['rev-parse', 'HEAD'], { cwd: workDir })).stdout.trim();
  await run(['clone', '-q', '--bare', workDir, remoteDir]);
  await run(['remote', 'add', 'origin', remoteDir], { cwd: workDir });
  await run(['fetch', '-q', 'origin'], { cwd: workDir });
  await run(['branch', '-q', '--set-upstream-to', `origin/${branch}`, branch], { cwd: workDir });
  return { remoteDir, workDir, branch, baseSha };
}
