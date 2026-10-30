import { mkdir } from 'node:fs/promises';
import { dirname } from 'node:path';
import { Git, execGit, type GitRunner } from './git.js';

export interface PrepareWorkdirOptions {
  repoUrl: string;
  /** The agent's branch; created from `baseRef` when the remote does not have it yet. */
  branch: string;
  baseRef: string;
  dir: string;
  git?: GitRunner;
  /** Extra environment for git (credentials, e.g. GIT_CONFIG_* for an http extraheader). */
  env?: Record<string, string>;
}

/** Clone `repoUrl` into `dir` and check out the agent's branch. */
export async function prepareWorkdir(options: PrepareWorkdirOptions): Promise<Git> {
  const runner = options.git ?? execGit;
  await mkdir(dirname(options.dir), { recursive: true });
  const parent = new Git(dirname(options.dir), runner, options.env);
  await parent.run(['clone', '--quiet', options.repoUrl, options.dir]);
  const git = new Git(options.dir, runner, options.env);
  const remoteBranch = await git.revParse(`refs/remotes/origin/${options.branch}`);
  if (remoteBranch) await git.run(['checkout', '--quiet', '-B', options.branch, `refs/remotes/origin/${options.branch}`]);
  else {
    const base = (await git.revParse(`refs/remotes/origin/${options.baseRef}`)) ?? (await git.revParse(options.baseRef));
    if (!base) throw new Error(`base ref '${options.baseRef}' not found in ${options.repoUrl}`);
    await git.run(['checkout', '--quiet', '-B', options.branch, base]);
  }
  return git;
}

/**
 * Environment that makes git authenticate to github.com with a token, without writing it to disk
 * or the command line. Scoped to the host of `repoUrl`.
 */
export function gitAuthEnv(repoUrl: string, token: string): Record<string, string> {
  if (!/^https:\/\//.test(repoUrl)) return {};
  const origin = new URL(repoUrl).origin;
  const basic = Buffer.from(`x-access-token:${token}`).toString('base64');
  return {
    GIT_CONFIG_COUNT: '1',
    GIT_CONFIG_KEY_0: `http.${origin}/.extraheader`,
    GIT_CONFIG_VALUE_0: `AUTHORIZATION: basic ${basic}`,
  };
}
