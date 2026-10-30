import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { FileDiff, Ledger } from '@ratchet/core';
import { LocalGitHost, createBareRemote, createGitRunner } from '@ratchet/github';

/**
 * The real demo patches (packages/agents/scripts/demo-search), in the order the dev engine's seeded search run
 * "lands" them. The dev engine simulates agents and bench numbers; its diffs are real: the k-th verified win of a
 * search optimization is the k-th of these patches committed onto examples/demo-search.
 */
export const DEV_PATCHES = [
  '01-search-no-deep-clone.patch',
  '03-filter-memoize-parse.patch',
  '02-search-topk-collation-rank.patch',
  '04-search-filter-indexes.patch',
  '05-search-prefix-binary-search.patch',
  '06-filter-tokenizer-no-regex.patch',
];

const ENV = { GIT_AUTHOR_NAME: 'Ratchet', GIT_AUTHOR_EMAIL: 'ratchet@localhost', GIT_COMMITTER_NAME: 'Ratchet', GIT_COMMITTER_EMAIL: 'ratchet@localhost' };

/** Lazily builds a local repo of the demo with one commit per patch; `diff` maps verified wins onto those commits. */
export class DevGit {
  private ready: Promise<{ host: LocalGitHost; commits: string[] }> | undefined;
  private scratch: string | undefined;

  constructor(
    private readonly ledger: Ledger,
    private readonly repoRoot: string,
  ) {}

  /** Whether the demo sources and patches are on disk (they are in a checkout of the monorepo). */
  available(): boolean {
    return existsSync(join(this.repoRoot, 'examples/demo-search/package.json')) && existsSync(join(this.repoRoot, 'packages/agents/scripts/demo-search', DEV_PATCHES[0]!));
  }

  async diff(req: { optimizationId: string; repo: string; sha: string }): Promise<FileDiff[] | null> {
    if (!/search/i.test(req.repo) || !this.available()) return null;
    const k = this.ledger.ratchetHistory(req.optimizationId).findIndex((m) => m.sha === req.sha);
    if (k < 0 || k >= DEV_PATCHES.length) return null;
    const { host, commits } = await (this.ready ??= this.build());
    return host.diff('main', commits[k]!);
  }

  /** Remove the scratch repo, after a build that is still running has finished writing to it. */
  dispose(): void {
    const dir = this.scratch;
    if (!dir) return;
    const rm = () => {
      try {
        rmSync(dir, { recursive: true, force: true, maxRetries: 3 });
      } catch {
        // best effort: a temp dir
      }
    };
    if (this.ready) void this.ready.then(rm, rm);
    else rm();
  }

  private async build(): Promise<{ host: LocalGitHost; commits: string[] }> {
    this.scratch = mkdtempSync(join(tmpdir(), 'ratchet-dev-git-'));
    const remote = await createBareRemote(join(this.repoRoot, 'examples/demo-search'), { remoteDir: join(this.scratch, 'remote.git'), workDir: join(this.scratch, 'work') });
    const git = createGitRunner();
    const commits: string[] = [];
    for (const p of DEV_PATCHES) {
      await git(['apply', join(this.repoRoot, 'packages/agents/scripts/demo-search', p)], { cwd: remote.workDir });
      await git(['add', '-A'], { cwd: remote.workDir });
      await git(['-c', 'commit.gpgsign=false', 'commit', '-q', '-m', p.replace(/^\d+-|\.patch$/g, '')], { cwd: remote.workDir, env: ENV });
      commits.push((await git(['rev-parse', 'HEAD'], { cwd: remote.workDir })).stdout.trim());
    }
    await git(['push', '-q', 'origin', 'HEAD:refs/heads/ratchet/dev/wins'], { cwd: remote.workDir });
    return { host: new LocalGitHost({ remoteDir: remote.remoteDir }), commits };
  }
}
