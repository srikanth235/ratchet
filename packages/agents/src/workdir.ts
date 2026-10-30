import { existsSync } from 'node:fs';
import { mkdir, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
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

// ---------------------------------------------------------------- local mode: one clone, a git worktree per agent

export interface LocalCloneOptions {
  repoUrl: string;
  /** Where the clone lives (not the user's own working copy: Ratchet keeps a separate clone under its app data). */
  dir: string;
  git?: GitRunner;
  env?: Record<string, string>;
}

/**
 * Make sure `dir` holds a clone of `repoUrl`: clone it, or fetch when it is already there (a second run of the same
 * optimization after a restart). Per-worktree git config is switched on so each agent can have its own identity.
 */
export async function ensureLocalClone(options: LocalCloneOptions): Promise<Git> {
  const runner = options.git ?? execGit;
  await mkdir(dirname(options.dir), { recursive: true });
  if (existsSync(join(options.dir, '.git'))) {
    const clone = new Git(options.dir, runner, options.env);
    await clone.run(['fetch', '--quiet', '--prune', 'origin']);
    await clone.run(['config', 'extensions.worktreeConfig', 'true']);
    return clone;
  }
  const parent = new Git(dirname(options.dir), runner, options.env);
  await parent.run(['clone', '--quiet', options.repoUrl, options.dir]);
  const clone = new Git(options.dir, runner, options.env);
  await clone.run(['config', 'extensions.worktreeConfig', 'true']);
  return clone;
}

export interface AddWorktreeOptions {
  clone: Git;
  /** The worktree directory (one per agent). */
  dir: string;
  branch: string;
  baseRef: string;
  /** Commit identity for this agent, stored in the worktree's own config so agents sharing the clone do not overwrite each other. */
  identity: { name: string; email: string };
  git?: GitRunner;
  env?: Record<string, string>;
}

/**
 * `git worktree add -B <branch> <dir> <start>`: the agent's branch is the remote's when it exists, else created from `baseRef`
 * (same rule as `prepareWorkdir`). Commit signing is switched off in the worktree: an agent must never wait on a pinentry prompt.
 */
export async function addWorktree(options: AddWorktreeOptions): Promise<Git> {
  const { clone, dir, branch, baseRef } = options;
  if (existsSync(dir)) await removeWorktree(clone, dir); // left over from a crashed run
  const remoteBranch = await clone.revParse(`refs/remotes/origin/${branch}`);
  let start = remoteBranch;
  if (!start) {
    start = (await clone.revParse(`refs/remotes/origin/${baseRef}`)) ?? (await clone.revParse(baseRef));
    if (!start) throw new Error(`base ref '${baseRef}' not found in the local clone`);
  }
  await mkdir(dirname(dir), { recursive: true });
  await clone.run(['worktree', 'add', '--quiet', '-B', branch, dir, start]);
  const tree = new Git(dir, options.git ?? execGit, options.env);
  await tree.run(['config', '--worktree', 'user.name', options.identity.name]);
  await tree.run(['config', '--worktree', 'user.email', options.identity.email]);
  await tree.run(['config', '--worktree', 'commit.gpgsign', 'false']);
  return tree;
}

/** Remove an agent's worktree (and its directory, even if git refuses); never throws. */
export async function removeWorktree(clone: Git, dir: string): Promise<void> {
  await clone.try(['worktree', 'remove', '--force', dir]).catch(() => undefined);
  await rm(dir, { recursive: true, force: true }).catch(() => undefined);
  await clone.try(['worktree', 'prune']).catch(() => undefined);
}
