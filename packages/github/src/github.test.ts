import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { GitHubGitHost, createBareRemote, createGitRunner, type BareRemote, type GitRunOptions, type GitRunner, type OctokitLike } from './index.js';

const DEMO = resolve(dirname(fileURLToPath(import.meta.url)), '../../../examples/demo-search');
const TOKEN = 'ghp_SuperSecretToken1234567890';
const TOKEN_B64 = Buffer.from(`x-access-token:${TOKEN}`).toString('base64');
const GH_URL = 'https://github.com/acme/search.git';
const ENV = { ...process.env, GIT_AUTHOR_NAME: 'A', GIT_AUTHOR_EMAIL: 'a@x', GIT_COMMITTER_NAME: 'A', GIT_COMMITTER_EMAIL: 'a@x' };
const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], { cwd, env: ENV, encoding: 'utf8' }).trim();

interface Call {
  args: string[];
  env: Record<string, string>;
}

let scratch: string;
let remote: BareRemote;
let candidateSha: string;

beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), 'ratchet-gh-test-'));
  remote = await createBareRemote(DEMO, { remoteDir: join(scratch, 'remote.git'), workDir: join(scratch, 'work') });
  git(remote.workDir, 'checkout', '-q', '-b', 'ratchet/o1/claude');
  writeFileSync(join(remote.workDir, 'src/a.txt'), 'one\n');
  git(remote.workDir, 'add', '-A');
  git(remote.workDir, 'commit', '-q', '-m', 'Cache\n\nRatchet-Candidate: true');
  candidateSha = git(remote.workDir, 'rev-parse', 'HEAD');
  git(remote.workDir, 'push', '-q', 'origin', 'ratchet/o1/claude');
});
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

/** Real git, but the GitHub URL is swapped for the local bare remote; records what the host asked for. */
function recordingRunner(calls: Call[]): GitRunner {
  const real = createGitRunner([TOKEN, TOKEN_B64]);
  return (args, opts: GitRunOptions = {}) => {
    calls.push({ args: [...args], env: { ...opts.env } });
    return real(args.map((a) => a.replace(GH_URL, remote.remoteDir)), opts);
  };
}

function fakeOctokit() {
  const create = vi.fn<OctokitLike['rest']['pulls']['create']>(async () => ({ data: { html_url: 'https://github.com/acme/search/pull/7', number: 7 } }));
  const list = vi.fn<OctokitLike['rest']['pulls']['list']>(async () => ({ data: [] }));
  const update = vi.fn<OctokitLike['rest']['pulls']['update']>(async () => ({ data: { html_url: 'https://github.com/acme/search/pull/7' } }));
  return { create, list, update, client: { rest: { pulls: { create, list, update } } } as OctokitLike };
}

function mkHost(calls: Call[], octokit: OctokitLike) {
  return new GitHubGitHost({
    repo: 'acme/search',
    token: TOKEN,
    cacheDir: join(mkdtempSync(join(scratch, 'cache-')), 'mirror.git'),
    runner: recordingRunner(calls),
    octokit,
  });
}

describe('GitHubGitHost', () => {
  it('reads candidates, creates branches and opens a squashed PR via Octokit', async () => {
    const calls: Call[] = [];
    const octo = fakeOctokit();
    const host = mkHost(calls, octo.client);

    expect(host.remoteUrl()).toBe(GH_URL);
    const found = await host.listCandidates('ratchet/o1/claude');
    expect(found.map((c) => c.sha)).toEqual([candidateSha]);
    expect(await host.changedFiles('main', candidateSha)).toEqual(['src/a.txt']);

    await host.createBranch('main', 'ratchet/o1/codex');
    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/o1/codex')).toBe(remote.baseSha);

    const { url } = await host.openPullRequest({
      base: 'main',
      head: 'ratchet/o1/result',
      title: 'ratchet: p95 100 → 80',
      body: 'REPORT',
      shas: [candidateSha],
      commitMessage: 'ratchet: squash\n\nCo-authored-by: Claude Code <noreply@anthropic.com>',
    });
    expect(url).toBe('https://github.com/acme/search/pull/7');
    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/o1/result^{tree}')).toBe(
      git(remote.remoteDir, 'rev-parse', `${candidateSha}^{tree}`),
    );
    expect(octo.create).toHaveBeenCalledWith({
      owner: 'acme',
      repo: 'search',
      title: 'ratchet: p95 100 → 80',
      head: 'ratchet/o1/result',
      base: 'main',
      body: 'REPORT',
    });

    // checkout works from the mirror
    const dir = join(scratch, 'co');
    await host.checkout(candidateSha, dir);
    expect(readFileSync(join(dir, 'src/a.txt'), 'utf8')).toBe('one\n');
  });

  it('never puts the token in argv; it travels only in the git config env of network calls', async () => {
    const calls: Call[] = [];
    const host = mkHost(calls, fakeOctokit().client);
    await host.listCandidates('ratchet/o1/claude');
    await host.createBranch('main', 'ratchet/o1/another');
    await host.openPullRequest({ base: 'main', head: 'ratchet/o1/result3', title: 't', body: 'b', shas: [candidateSha] });

    expect(calls.length).toBeGreaterThan(5);
    for (const c of calls) {
      const argv = c.args.join('\n');
      expect(argv).not.toContain(TOKEN);
      expect(argv).not.toContain(TOKEN_B64);
      expect(argv).not.toMatch(/https:\/\/[^/\s]*@/); // no credentials in a URL
    }
    const network = calls.filter((c) => c.args.includes('fetch') || c.args.includes('push'));
    expect(network.length).toBeGreaterThanOrEqual(3);
    for (const c of network) {
      expect(c.env.GIT_CONFIG_COUNT).toBe('1');
      expect(c.env.GIT_CONFIG_KEY_0).toBe('http.https://github.com/.extraheader');
      expect(c.env.GIT_CONFIG_VALUE_0).toBe(`AUTHORIZATION: basic ${TOKEN_B64}`);
    }
  });

  it('diff: syncs the mirror, diffs against the merge base and never puts the token in argv (mocked git)', async () => {
    const calls: Call[] = [];
    const H = 'a'.repeat(40);
    const B = 'b'.repeat(40);
    const M = 'c'.repeat(40);
    const runner: GitRunner = async (args, opts = {}) => {
      calls.push({ args: [...args], env: { ...opts.env } });
      const cmd = args.slice(2); // drop --git-dir <dir>
      if (cmd[0] === 'rev-parse') return { stdout: `${cmd.at(-1)!.includes('main') ? B : H}\n` };
      if (cmd[0] === 'merge-base') return { stdout: `${M}\n` };
      if (cmd[0] === 'diff' && cmd.includes('--numstat')) return { stdout: '3\t1\tsrc/search.ts\0-\t-\tlogo.png\0' };
      if (cmd[0] === 'diff') {
        return { stdout: 'diff --git a/src/search.ts b/src/search.ts\nindex 1..2 100644\n--- a/src/search.ts\n+++ b/src/search.ts\n@@ -1,2 +1,4 @@\n-a\n+b\n+c\n+d\n x\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n' };
      }
      return { stdout: '' };
    };
    const host = new GitHubGitHost({ repo: 'acme/search', token: TOKEN, cacheDir: join(mkdtempSync(join(scratch, 'cache-')), 'mirror.git'), runner, octokit: fakeOctokit().client });
    const files = await host.diff('main', H);
    expect(files).toEqual([
      { path: 'src/search.ts', additions: 3, deletions: 1, binary: false, patch: '@@ -1,2 +1,4 @@\n-a\n+b\n+c\n+d\n x' },
      { path: 'logo.png', additions: 0, deletions: 0, binary: true, patch: '' },
    ]);
    const fetch = calls.find((c) => c.args.includes('fetch'))!;
    expect(fetch.env.GIT_CONFIG_VALUE_0).toBe(`AUTHORIZATION: basic ${TOKEN_B64}`);
    const diffs = calls.filter((c) => c.args.includes('diff'));
    expect(diffs.map((c) => c.args.slice(-2))).toEqual([[M, H], [M, H]]);
    for (const c of calls) expect(c.args.join(' ')).not.toContain(TOKEN);
  });

  it('scrubs the token from git errors', async () => {
    // default runner against an unreachable host
    const host = new GitHubGitHost({ repo: 'acme/search', token: TOKEN, cacheDir: join(scratch, 'dead.git'), host: 'localhost.invalid' });
    const err = await host.listCandidates('main').catch((e: Error) => e);
    expect(err).toBeInstanceOf(Error);
    expect((err as Error).message).not.toContain(TOKEN);
    expect((err as Error).message).not.toContain(TOKEN_B64);
  });

  it('createGitRunner redacts secrets from failure messages', async () => {
    const run = createGitRunner([TOKEN]);
    const err = await run([`--bogus-${TOKEN}`]).catch((e: Error) => e);
    expect((err as Error).message).toContain('***');
    expect((err as Error).message).not.toContain(TOKEN);
  });

  it('refreshes the existing PR when GitHub answers 422', async () => {
    const octo = fakeOctokit();
    octo.create.mockRejectedValueOnce(Object.assign(new Error('Validation Failed'), { status: 422 }));
    octo.list.mockResolvedValueOnce({ data: [{ html_url: 'https://github.com/acme/search/pull/3', number: 3 }] });
    const host = mkHost([], octo.client);
    const { url } = await host.openPullRequest({ base: 'main', head: 'ratchet/o1/claude', title: 'T', body: 'B' });
    expect(url).toBe('https://github.com/acme/search/pull/7');
    expect(octo.list).toHaveBeenCalledWith(expect.objectContaining({ head: 'acme:ratchet/o1/claude' }));
    expect(octo.update).toHaveBeenCalledWith({ owner: 'acme', repo: 'search', pull_number: 3, title: 'T', body: 'B' });
  });

  it('rethrows non-422 Octokit errors', async () => {
    const octo = fakeOctokit();
    octo.create.mockRejectedValueOnce(Object.assign(new Error('Forbidden'), { status: 403 }));
    const host = mkHost([], octo.client);
    await expect(host.openPullRequest({ base: 'main', head: 'ratchet/o1/claude', title: 'T', body: 'B' })).rejects.toThrow('Forbidden');
  });

  it('validates repo and token', () => {
    expect(() => new GitHubGitHost({ repo: 'nope', token: TOKEN })).toThrow(/owner\/name/);
    expect(() => new GitHubGitHost({ repo: 'a/b', token: '' })).toThrow(/token/);
  });
});
