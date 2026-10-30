import { Octokit } from '@octokit/rest';

/** The slice of Octokit used by {@link checkGitHubAccess}; tests inject a fake. */
export interface OctokitReposLike {
  rest: {
    repos: {
      get(p: { owner: string; repo: string }): Promise<{
        data: { full_name: string; default_branch: string; private?: boolean; permissions?: { push?: boolean; admin?: boolean; pull?: boolean } };
      }>;
    };
  };
}

export interface GitHubAccess {
  fullName: string;
  defaultBranch: string;
  private: boolean;
  /** The token can push branches (needed for candidate branches and the PR). */
  canPush: boolean;
}

export interface CheckGitHubAccessOptions {
  /** `owner/name`. */
  repo: string;
  token: string;
  /** REST API base URL, for GitHub Enterprise. */
  apiUrl?: string;
  octokit?: OctokitReposLike;
}

const REPO_RE = /^([\w.-]+)\/([\w.-]+?)(?:\.git)?$/;

/**
 * Read-only check that `token` can see `repo` and may push to it (one `GET /repos/{owner}/{name}`). The token is
 * sent only in the Authorization header and never appears in a thrown message.
 */
export async function checkGitHubAccess(opts: CheckGitHubAccessOptions): Promise<GitHubAccess> {
  const m = REPO_RE.exec(opts.repo);
  if (!m) throw new Error(`repo must look like "owner/name", got "${opts.repo}"`);
  if (!opts.token) throw new Error('a GitHub token is required');
  const octokit = opts.octokit ?? (new Octokit({ auth: opts.token, ...(opts.apiUrl ? { baseUrl: opts.apiUrl } : {}) }) as unknown as OctokitReposLike);
  let data: Awaited<ReturnType<OctokitReposLike['rest']['repos']['get']>>['data'];
  try {
    data = (await octokit.rest.repos.get({ owner: m[1]!, repo: m[2]! })).data;
  } catch (err) {
    const status = (err as { status?: number }).status;
    if (status === 401) throw new Error('GitHub rejected the token (401): it is wrong, expired or revoked');
    if (status === 404) throw new Error(`repository ${m[1]}/${m[2]} not found, or the token cannot see it (404)`);
    throw new Error(`GitHub request failed${status ? ` (${status})` : ''}: ${(err as Error).message.split(opts.token).join('***')}`);
  }
  return {
    fullName: data.full_name,
    defaultBranch: data.default_branch,
    private: data.private ?? false,
    canPush: data.permissions?.push === true || data.permissions?.admin === true,
  };
}
