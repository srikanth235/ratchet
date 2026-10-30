import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { LocalGitHost, buildSquashPlan, createBareRemote, type BareRemote } from './index.js';

const DEMO = resolve(dirname(fileURLToPath(import.meta.url)), '../../../examples/demo-search');
const ENV = {
  ...process.env,
  GIT_AUTHOR_NAME: 'Agent',
  GIT_AUTHOR_EMAIL: 'agent@example.com',
  GIT_COMMITTER_NAME: 'Agent',
  GIT_COMMITTER_EMAIL: 'agent@example.com',
};
const git = (cwd: string, ...args: string[]) =>
  execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], { cwd, env: ENV, encoding: 'utf8' }).trim();

let remote: BareRemote;
let host: LocalGitHost;
let scratch: string;
const shas: Record<string, string> = {};

function commit(file: string, content: string, message: string): string {
  writeFileSync(join(remote.workDir, file), content);
  git(remote.workDir, 'add', '-A');
  git(remote.workDir, 'commit', '-q', '-m', message);
  return git(remote.workDir, 'rev-parse', 'HEAD');
}

beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), 'ratchet-local-test-'));
  remote = await createBareRemote(DEMO, { remoteDir: join(scratch, 'remote.git'), workDir: join(scratch, 'work') });
  host = new LocalGitHost({ remoteDir: remote.remoteDir, prDir: join(scratch, 'prs') });

  git(remote.workDir, 'checkout', '-q', '-b', 'ratchet/o1/claude');
  shas.c1 = commit('src/a.txt', 'one\n', 'Cache tokenizer\n\nWarm the cache.\n\nRatchet-Candidate: true\nRatchet-Agent: claude');
  shas.plain = commit('notes.txt', 'wip\n', 'wip: not a candidate');
  shas.mentions = commit('src/m.txt', 'm\n', 'Mentions Ratchet-Candidate: true in prose\n\nThe trailer below is not real.\nRatchet-Candidate: true\n\nTrailing paragraph.');
  shas.no = commit('src/n.txt', 'n\n', 'Explicitly not\n\nRatchet-Candidate: false');
  shas.c2 = commit('src/a.txt', 'two\n', 'Index postings\n\nSigned-off-by: A <a@example.com>\nratchet-candidate: true\nRatchet-Agent: claude');
  shas.flipped = commit('src/f.txt', 'f\n', 'Repeated trailer\n\nRatchet-Candidate: true\nRatchet-Candidate: false');
  git(remote.workDir, 'push', '-q', 'origin', 'ratchet/o1/claude');
});

afterAll(() => rmSync(scratch, { recursive: true, force: true }));

describe('LocalGitHost', () => {
  it('lists only commits with a real Ratchet-Candidate: true trailer, oldest first', async () => {
    const found = await host.listCandidates('ratchet/o1/claude');
    expect(found.map((c) => c.sha)).toEqual([shas.c1, shas.c2]);
    expect(found[0]).toMatchObject({ branch: 'ratchet/o1/claude' });
    expect(found[0]!.message).toContain('Cache tokenizer');
    expect(found[0]!.committedAt).toBeGreaterThan(1_600_000_000_000);
  });

  it('respects sinceSha', async () => {
    const found = await host.listCandidates('ratchet/o1/claude', shas.c1);
    expect(found.map((c) => c.sha)).toEqual([shas.c2]);
    expect(await host.listCandidates('ratchet/o1/claude', shas.c2)).toEqual([]);
  });

  it('createBranch branches from a ref and refuses to overwrite', async () => {
    await host.createBranch('main', 'ratchet/o1/codex');
    expect(await host.listCandidates('ratchet/o1/codex')).toEqual([]);
    expect(git(remote.remoteDir, 'rev-parse', 'refs/heads/ratchet/o1/codex')).toBe(remote.baseSha);
    await expect(host.createBranch(shas.c1!, 'ratchet/o1/codex')).rejects.toThrow();
  });

  it('changedFiles lists paths changed relative to base', async () => {
    expect((await host.changedFiles('main', shas.c1!)).sort()).toEqual(['src/a.txt']);
    expect((await host.changedFiles('main', shas.c2!)).sort()).toEqual(['notes.txt', 'src/a.txt', 'src/m.txt', 'src/n.txt']);
    expect(await host.changedFiles(shas.c1!, shas.c2!)).toContain('notes.txt');
  });

  it('checkout materializes a sha without .git or repo side effects', async () => {
    const dir = join(scratch, 'co');
    await host.checkout(shas.c1!, dir);
    expect(readFileSync(join(dir, 'src/a.txt'), 'utf8')).toBe('one\n');
    expect(existsSync(join(dir, 'ratchet.yaml'))).toBe(true);
    expect(existsSync(join(dir, '.git'))).toBe(false);
    expect(existsSync(join(dir, 'notes.txt'))).toBe(false);
    const dir2 = join(scratch, 'co2');
    await host.checkout(shas.c2!, dir2);
    expect(readFileSync(join(dir2, 'src/a.txt'), 'utf8')).toBe('two\n');
    expect(git(remote.remoteDir, 'worktree', 'list')).not.toContain('co');
  });

  it('openPullRequest squashes to the final verified tree, writes the body to a file', async () => {
    // c2 was authored on top of c1 (rebased), so its tree contains both wins
    const plan = buildSquashPlan({
      metric: 'p95',
      direction: 'minimize',
      wins: [
        { sha: shas.c1!, agentKind: 'claude-code', message: 'Cache tokenizer', fromValue: 100, toValue: 80 },
        { sha: shas.c2!, agentKind: 'codex', message: 'Index postings', fromValue: 80, toValue: 60 },
      ],
    });
    const { url } = await host.openPullRequest({
      base: 'main',
      head: 'ratchet/o1/result',
      title: plan.title,
      body: '# report body',
      shas: plan.shas,
      commitMessage: plan.message,
    });

    const tree = (ref: string) => git(remote.remoteDir, 'rev-parse', `${ref}^{tree}`);
    expect(tree('refs/heads/ratchet/o1/result')).toBe(tree(shas.c2!));
    expect(git(remote.remoteDir, 'rev-list', '--count', 'main..ratchet/o1/result')).toBe('1');
    expect(git(remote.remoteDir, 'rev-parse', 'ratchet/o1/result^')).toBe(remote.baseSha);
    const msg = git(remote.remoteDir, 'log', '-1', '--format=%B', 'ratchet/o1/result');
    expect(msg).toContain('Co-authored-by: Claude Code <noreply@anthropic.com>');
    expect(msg).toContain('Co-authored-by: Codex <noreply@openai.com>');

    expect(url.startsWith('file://')).toBe(true);
    expect(readFileSync(fileURLToPath(url), 'utf8')).toContain('# report body');
  });

  it('squash is parented at the merge-base when main has moved on', async () => {
    const other = join(scratch, 'other');
    execFileSync('git', ['clone', '-q', remote.remoteDir, other]);
    writeFileSync(join(other, 'later.txt'), 'later\n');
    git(other, 'add', '-A');
    git(other, 'commit', '-q', '-m', 'main moves');
    git(other, 'push', '-q', 'origin', 'HEAD:main');

    await host.openPullRequest({ base: 'main', head: 'ratchet/o1/result2', title: 't', body: 'b', shas: [shas.c2!] });
    expect(git(remote.remoteDir, 'rev-parse', 'ratchet/o1/result2^')).toBe(remote.baseSha);
    expect(git(remote.remoteDir, 'rev-parse', 'ratchet/o1/result2^{tree}')).toBe(git(remote.remoteDir, 'rev-parse', `${shas.c2}^{tree}`));
  });

  it('remoteUrl is a file:// url', () => {
    expect(host.remoteUrl()).toMatch(/^file:\/\/.*remote\.git$/);
  });
});
