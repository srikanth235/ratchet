import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { GitHubGitHost, LocalGitHost, createBareRemote, createGitRunner, type BareRemote, type GitRunOptions, type GitRunner, type OctokitLike } from './index.js';

const ENV = { ...process.env, GIT_AUTHOR_NAME: 'A', GIT_AUTHOR_EMAIL: 'a@x', GIT_COMMITTER_NAME: 'A', GIT_COMMITTER_EMAIL: 'a@x' };
const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], { cwd, env: ENV, encoding: 'utf8' }).trim();
const TOKEN = 'ghp_SuperSecretToken1234567890';
const GH_URL = 'https://github.com/acme/search.git';

const FILES = [
  { path: 'ratchet.yaml', content: 'version: 0\n' },
  { path: 'RATCHET.md', content: '# brief\n' },
  { path: 'bench/run.mjs', content: 'console.log("{}")\n', executable: true },
  { path: 'src/new/deep/file.txt', content: 'x' },
];

let scratch: string;
let remote: BareRemote;

beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), 'ratchet-commitfiles-'));
  const src = join(scratch, 'src-project');
  mkdirSync(join(src, 'src'), { recursive: true });
  writeFileSync(join(src, 'src/app.js'), 'old\n');
  writeFileSync(join(src, 'RATCHET.md'), 'the old brief\n');
  writeFileSync(join(src, 'keep.txt'), 'keep\n');
  remote = await createBareRemote(src, { remoteDir: join(scratch, 'remote.git'), workDir: join(scratch, 'work') });
});
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const show = (path: string, ref: string) => git(remote.remoteDir, 'show', `${ref}:${path}`);

describe('LocalGitHost.commitFiles', () => {
  it('creates a branch with one commit on top of the base that adds the files (and replaces existing ones)', async () => {
    const host = new LocalGitHost({ remoteDir: remote.remoteDir, prDir: join(scratch, 'prs') });
    const { sha } = await host.commitFiles({ baseRef: 'main', branch: 'ratchet/template-x', message: 'Add template\n\nbody', files: FILES });

    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/template-x')).toBe(sha);
    expect(git(remote.remoteDir, 'rev-parse', `${sha}^`)).toBe(remote.baseSha); // exactly one commit on the base
    expect(git(remote.remoteDir, 'log', '-1', '--format=%B', sha)).toBe('Add template\n\nbody');
    expect(git(remote.remoteDir, 'log', '-1', '--format=%an <%ae>', sha)).toBe('Ratchet <ratchet@users.noreply.github.com>');
    expect(show('ratchet.yaml', sha)).toBe('version: 0');
    expect(show('RATCHET.md', sha)).toBe('# brief'); // replaced
    expect(show('src/new/deep/file.txt', sha)).toBe('x');
    expect(show('keep.txt', sha)).toBe('keep'); // the rest of the base tree is untouched
    expect(show('src/app.js', sha)).toBe('old');
    expect(git(remote.remoteDir, 'ls-tree', sha, 'bench/run.mjs').split(' ')[0]).toBe('100755');
    expect(git(remote.remoteDir, 'ls-tree', sha, 'ratchet.yaml').split(' ')[0]).toBe('100644');
    // main is untouched, and the diff is what the PR will show
    expect(git(remote.remoteDir, 'rev-parse', 'main')).toBe(remote.baseSha);
    expect((await host.changedFiles('main', sha)).sort()).toEqual(['RATCHET.md', 'bench/run.mjs', 'ratchet.yaml', 'src/new/deep/file.txt']);
  });

  it('refuses an existing branch, unsafe or duplicate paths and an empty list, and leaves the remote alone', async () => {
    const host = new LocalGitHost({ remoteDir: remote.remoteDir });
    const before = git(remote.remoteDir, 'for-each-ref');
    await expect(host.commitFiles({ baseRef: 'main', branch: 'ratchet/template-x', message: 'm', files: FILES })).rejects.toThrow();
    for (const path of ['../evil', '/abs', 'a/../b', '.git/config', 'a/.git/hooks/x', 'a//b', './a', 'a\\b', '']) {
      await expect(host.commitFiles({ baseRef: 'main', branch: 'ratchet/bad', message: 'm', files: [{ path, content: 'x' }] })).rejects.toThrow(/unsafe path/);
    }
    await expect(host.commitFiles({ baseRef: 'main', branch: 'ratchet/bad', message: 'm', files: [{ path: 'a', content: '1' }, { path: 'a', content: '2' }] })).rejects.toThrow(/duplicate/);
    await expect(host.commitFiles({ baseRef: 'main', branch: 'ratchet/bad', message: 'm', files: [] })).rejects.toThrow(/no files/);
    await expect(host.commitFiles({ baseRef: 'nope', branch: 'ratchet/bad', message: 'm', files: FILES })).rejects.toThrow(/cannot resolve/);
    expect(git(remote.remoteDir, 'for-each-ref')).toBe(before);
  });

  it('the branch is a normal PR head: openPullRequest with no shas opens it as is', async () => {
    const host = new LocalGitHost({ remoteDir: remote.remoteDir, prDir: join(scratch, 'prs') });
    const { sha } = await host.commitFiles({ baseRef: 'main', branch: 'ratchet/template-y', message: 'Add y', files: [{ path: 'y.txt', content: 'y\n' }] });
    const { url } = await host.openPullRequest({ base: 'main', head: 'ratchet/template-y', title: 'Add y', body: 'what and how', shas: [] });
    expect(readFileSync(new URL(url), 'utf8')).toContain('what and how');
    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/template-y')).toBe(sha); // not squashed again
  });

  it('can base the commit on a sha and handles binary-looking and multi-line content verbatim', async () => {
    const host = new LocalGitHost({ remoteDir: remote.remoteDir });
    const content = 'line1\r\nline2\n\n  indented \ttab\nunicode: é ✓\n';
    const { sha } = await host.commitFiles({ baseRef: remote.baseSha, branch: 'ratchet/template-z', message: 'z', files: [{ path: 'z.txt', content }] });
    expect(execFileSync('git', ['--git-dir', remote.remoteDir, 'show', `${sha}:z.txt`], { encoding: 'utf8' })).toBe(content);
  });
});

describe('GitHubGitHost.commitFiles (mirror and pushes against a local bare remote, Octokit mocked)', () => {
  function setup() {
    const calls: Array<{ args: string[]; env: Record<string, string> }> = [];
    const real = createGitRunner([TOKEN]);
    const runner: GitRunner = (args, opts: GitRunOptions = {}) => {
      calls.push({ args: [...args], env: { ...opts.env } });
      return real(args.map((a) => a.replace(GH_URL, remote.remoteDir)), opts);
    };
    const create = vi.fn<OctokitLike['rest']['pulls']['create']>(async () => ({ data: { html_url: 'https://github.com/acme/search/pull/9', number: 9 } }));
    const octokit = {
      rest: {
        pulls: {
          create,
          list: vi.fn<OctokitLike['rest']['pulls']['list']>(async () => ({ data: [] })),
          update: vi.fn<OctokitLike['rest']['pulls']['update']>(async () => ({ data: { html_url: 'u' } })),
        },
      },
    } as OctokitLike;
    const host = new GitHubGitHost({ repo: 'acme/search', token: TOKEN, cacheDir: join(mkdtempSync(join(scratch, 'cache-')), 'mirror.git'), runner, octokit });
    return { host, calls, create };
  }

  it('pushes the branch and opens the PR from it (no squash, head already exists)', async () => {
    const { host, calls, create } = setup();
    const { sha } = await host.commitFiles({ baseRef: 'main', branch: 'ratchet/template-gh', message: 'Add template', files: FILES });
    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/template-gh')).toBe(sha);
    expect(git(remote.remoteDir, 'rev-parse', `${sha}^`)).toBe(remote.baseSha);

    const pr = await host.openPullRequest({ base: 'main', head: 'ratchet/template-gh', title: 'Add template', body: 'body', shas: [] });
    expect(pr.url).toBe('https://github.com/acme/search/pull/9');
    expect(create).toHaveBeenCalledWith({ owner: 'acme', repo: 'search', title: 'Add template', head: 'ratchet/template-gh', base: 'main', body: 'body' });
    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/template-gh')).toBe(sha);

    // the token only travels in the environment of the network calls, never in argv
    expect(calls.flatMap((c) => c.args).join(' ')).not.toContain(TOKEN);
    expect(calls.some((c) => c.args.includes('push') && Object.values(c.env).some((v) => v.includes('AUTHORIZATION')))).toBe(true);
  });

  it('refuses a branch that already exists on the remote', async () => {
    const { host } = setup();
    await expect(host.commitFiles({ baseRef: 'main', branch: 'ratchet/template-gh', message: 'again', files: FILES })).rejects.toThrow();
  });
});
