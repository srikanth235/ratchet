import { execFile } from 'node:child_process';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

/** What "Local folder…" learns about a folder: its GitHub remote, default branch and ratchet.yaml. */
export interface LocalRepoInfo {
  path: string;
  /** `owner/name` of `origin` when it is on GitHub. */
  repo: string | null;
  baseRef: string;
  configYaml: string | null;
  problem: string | null;
}

export type GitRun = (cwd: string, args: string[]) => Promise<string>;

const nodeGit: GitRun = (cwd, args) =>
  new Promise((res, rej) => {
    execFile('git', args, { cwd, timeout: 10_000, env: { ...process.env, GIT_TERMINAL_PROMPT: '0' } }, (err, stdout) => (err ? rej(err) : res(stdout.trim())));
  });

/** `https://github.com/acme/api.git`, `git@github.com:acme/api`, `ssh://git@github.com/acme/api` -> `acme/api`. */
export function githubSlug(remote: string): string | null {
  const m = /github\.com[/:]([A-Za-z0-9][A-Za-z0-9-]*)\/([A-Za-z0-9._-]+?)(?:\.git)?\/?$/.exec(remote.trim());
  return m ? `${m[1]}/${m[2]}` : null;
}

export async function inspectLocalFolder(path: string, run: GitRun = nodeGit): Promise<LocalRepoInfo> {
  const dir = resolve(path);
  const base: LocalRepoInfo = { path: dir, repo: null, baseRef: 'main', configYaml: null, problem: null };
  try {
    if (!statSync(dir).isDirectory()) return { ...base, problem: `${dir} is not a folder.` };
  } catch {
    return { ...base, problem: `${dir} does not exist.` };
  }
  if (!existsSync(join(dir, '.git'))) return { ...base, problem: 'This folder is not a git repository.' };

  const cfg = join(dir, 'ratchet.yaml');
  const configYaml = existsSync(cfg) ? readFileSync(cfg, 'utf8') : null;

  let remote = '';
  try {
    remote = await run(dir, ['remote', 'get-url', 'origin']);
  } catch {
    /* no origin */
  }
  const repo = remote ? githubSlug(remote) : null;

  let baseRef = 'main';
  try {
    baseRef = (await run(dir, ['symbolic-ref', '--short', 'refs/remotes/origin/HEAD'])).replace(/^origin\//, '') || baseRef;
  } catch {
    try {
      baseRef = (await run(dir, ['symbolic-ref', '--short', 'HEAD'])) || baseRef;
    } catch {
      /* keep main */
    }
  }

  const problem = repo
    ? null
    : remote
      ? 'The origin remote is not on GitHub. Ratchet pushes candidate branches to GitHub and the cloud evaluator fetches them from there.'
      : 'This folder has no GitHub remote (origin). Ratchet pushes candidate branches to GitHub and the cloud evaluator fetches them from there.';
  return { path: dir, repo, baseRef, configYaml, problem };
}
