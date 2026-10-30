import { describe, expect, it } from 'vitest';
import { checkGitHubAccess, type OctokitReposLike } from './access.js';

const fake = (get: OctokitReposLike['rest']['repos']['get']): OctokitReposLike => ({ rest: { repos: { get } } });
const TOKEN = 'ghp_SuperSecretToken1234567890';

describe('checkGitHubAccess', () => {
  it('reports the repo, default branch and whether the token can push', async () => {
    let seen: { owner: string; repo: string } | undefined;
    const octokit = fake(async (p) => {
      seen = p;
      return { data: { full_name: 'acme/search', default_branch: 'main', private: true, permissions: { push: true } } };
    });
    expect(await checkGitHubAccess({ repo: 'acme/search', token: TOKEN, octokit })).toEqual({ fullName: 'acme/search', defaultBranch: 'main', private: true, canPush: true });
    expect(seen).toEqual({ owner: 'acme', repo: 'search' });
    const ro = fake(async () => ({ data: { full_name: 'acme/search', default_branch: 'main', permissions: { pull: true } } }));
    expect((await checkGitHubAccess({ repo: 'acme/search.git', token: TOKEN, octokit: ro })).canPush).toBe(false);
  });

  it('gives actionable errors and never echoes the token', async () => {
    const status = (n: number, message = 'x') => fake(async () => { throw Object.assign(new Error(message), { status: n }); });
    await expect(checkGitHubAccess({ repo: 'a/b', token: TOKEN, octokit: status(401) })).rejects.toThrow(/rejected the token/);
    await expect(checkGitHubAccess({ repo: 'a/b', token: TOKEN, octokit: status(404) })).rejects.toThrow(/not found/);
    const err = await checkGitHubAccess({ repo: 'a/b', token: TOKEN, octokit: status(500, `boom ${TOKEN}`) }).catch((e: Error) => e);
    expect((err as Error).message).toContain('(500)');
    expect((err as Error).message).not.toContain(TOKEN);
  });

  it('validates its inputs before any request', async () => {
    const octokit = fake(async () => { throw new Error('should not be called'); });
    await expect(checkGitHubAccess({ repo: 'nonsense', token: TOKEN, octokit })).rejects.toThrow(/owner\/name/);
    await expect(checkGitHubAccess({ repo: 'a/b', token: '', octokit })).rejects.toThrow(/token is required/);
  });
});
