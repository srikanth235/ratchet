import { existsSync, mkdirSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Octokit } from '@octokit/rest';
import { BaseGitHost, createGitRunner, type BaseGitHostOptions, type PullRequestInput } from './git.js';

/** The slice of Octokit used here; tests inject a fake. */
export interface OctokitLike {
  rest: {
    pulls: {
      create(p: { owner: string; repo: string; title: string; head: string; base: string; body: string }): Promise<{ data: { html_url: string; number: number } }>;
      list(p: { owner: string; repo: string; head: string; base: string; state: 'open' }): Promise<{ data: Array<{ html_url: string; number: number }> }>;
      update(p: { owner: string; repo: string; pull_number: number; title: string; body: string }): Promise<{ data: { html_url: string } }>;
    };
  };
}

export interface GitHubGitHostOptions extends BaseGitHostOptions {
  /** `owner/name`. */
  repo: string;
  /** Repo-scoped token (fine-grained PAT or GitHub App installation token). Never logged, never in argv. */
  token: string;
  /** Local bare mirror used to read branches; defaults to a temp dir. */
  cacheDir?: string;
  /** Git host name, for GitHub Enterprise. Default `github.com`. */
  host?: string;
  /** REST API base URL, for GitHub Enterprise. */
  apiUrl?: string;
  octokit?: OctokitLike;
}

/**
 * Same git operations as {@link LocalGitHost} against a GitHub remote. Reads go through a local bare mirror
 * (`refs/remotes/origin/*`); pushes and the PR use the token. The token only ever travels in the environment
 * (`GIT_CONFIG_*` extra header) or in Octokit's Authorization header.
 */
export class GitHubGitHost extends BaseGitHost {
  protected readonly branchPrefix = 'refs/remotes/origin/';
  private readonly owner: string;
  private readonly name: string;
  private readonly url: string;
  private readonly host: string;
  private readonly token: string;
  private readonly cacheDir: string;
  private readonly octokitOpt: OctokitLike | undefined;
  private readonly apiUrl: string | undefined;
  private octokit: OctokitLike | undefined;
  private ready: Promise<void> | undefined;

  constructor(options: GitHubGitHostOptions) {
    const token = options.token;
    if (!token) throw new Error('GitHubGitHost requires a token');
    super({ ...options, runner: options.runner ?? createGitRunner([token, basicAuth(token)]) });
    const m = /^([\w.-]+)\/([\w.-]+?)(?:\.git)?$/.exec(options.repo);
    if (!m) throw new Error(`repo must look like "owner/name", got "${options.repo}"`);
    this.owner = m[1]!;
    this.name = m[2]!;
    this.host = options.host ?? 'github.com';
    this.url = `https://${this.host}/${this.owner}/${this.name}.git`;
    this.token = token;
    this.cacheDir = options.cacheDir ?? join(mkdtempSync(join(tmpdir(), 'ratchet-gh-')), 'mirror.git');
    this.octokitOpt = options.octokit;
    this.apiUrl = options.apiUrl;
  }

  remoteUrl(): string {
    return this.url;
  }

  /** Environment that authenticates git over HTTPS without putting the token in argv or the URL. */
  private authEnv(): Record<string, string> {
    return {
      GIT_CONFIG_COUNT: '1',
      GIT_CONFIG_KEY_0: `http.https://${this.host}/.extraheader`,
      GIT_CONFIG_VALUE_0: `AUTHORIZATION: basic ${basicAuth(this.token)}`,
    };
  }

  protected async gitDir(): Promise<string> {
    this.ready ??= (async () => {
      if (!existsSync(join(this.cacheDir, 'HEAD'))) {
        mkdirSync(this.cacheDir, { recursive: true });
        await this.runner(['init', '--bare', '-q', this.cacheDir]);
      }
    })();
    await this.ready;
    return this.cacheDir;
  }

  protected override async sync(): Promise<void> {
    await this.git(['fetch', '--prune', '--no-tags', '-q', this.url, '+refs/heads/*:refs/remotes/origin/*'], {
      env: this.authEnv(),
    });
  }

  protected async publish(branch: string, sha: string, opts: { force: boolean; expected?: string }): Promise<void> {
    const refspec = `${opts.force ? '+' : ''}${sha}:refs/heads/${branch}`;
    await this.git(['push', '-q', this.url, refspec], { env: this.authEnv() });
    // keep the mirror consistent with what we just pushed
    await this.git(['update-ref', `refs/remotes/origin/${branch}`, sha]);
  }

  private client(): OctokitLike {
    this.octokit ??=
      this.octokitOpt ??
      (new Octokit({ auth: this.token, ...(this.apiUrl ? { baseUrl: this.apiUrl } : {}) }) as unknown as OctokitLike);
    return this.octokit;
  }

  protected async createPullRequest(input: PullRequestInput): Promise<{ url: string }> {
    const { owner, name: repo } = this;
    const pulls = this.client().rest.pulls;
    try {
      const { data } = await pulls.create({ owner, repo, title: input.title, head: input.head, base: input.base, body: input.body });
      return { url: data.html_url };
    } catch (err) {
      if ((err as { status?: number }).status !== 422) throw err;
      // most likely "a pull request already exists for <head>": refresh it instead
      const open = await pulls.list({ owner, repo, head: `${owner}:${input.head}`, base: input.base, state: 'open' });
      const existing = open.data[0];
      if (!existing) throw err;
      const { data } = await pulls.update({ owner, repo, pull_number: existing.number, title: input.title, body: input.body });
      return { url: data.html_url };
    }
  }
}

function basicAuth(token: string): string {
  return Buffer.from(`x-access-token:${token}`).toString('base64');
}
