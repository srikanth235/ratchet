import { execFile } from 'node:child_process';

export interface GitResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Runs `git <args>` in `cwd`. Injectable so tests and callers can fake or audit git. */
export type GitRunner = (args: string[], options: { cwd: string; env?: Record<string, string> }) => Promise<GitResult>;

/** Non-interactive: a missing credential must fail, not prompt. */
export const execGit: GitRunner = (args, { cwd, env }) =>
  new Promise((resolve) => {
    execFile(
      'git',
      args,
      { cwd, env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GIT_EDITOR: 'true', LC_ALL: 'C', ...env }, maxBuffer: 64 * 1024 * 1024 },
      (error, stdout, stderr) => {
        const code = error ? (typeof error.code === 'number' ? error.code : 1) : 0;
        resolve({ code, stdout: String(stdout), stderr: String(stderr) + (error && typeof error.code !== 'number' ? error.message : '') });
      },
    );
  });

export class GitError extends Error {
  constructor(readonly args: string[], readonly result: GitResult) {
    super(`git ${args.join(' ')} failed (${result.code}): ${(result.stderr || result.stdout).trim()}`);
    this.name = 'GitError';
  }
}

/** Thin wrapper bound to one working directory. */
export class Git {
  constructor(
    readonly cwd: string,
    private readonly runner: GitRunner = execGit,
    private readonly env?: Record<string, string>,
  ) {}

  try(args: string[]): Promise<GitResult> {
    return this.runner(args, { cwd: this.cwd, ...(this.env ? { env: this.env } : {}) });
  }

  async run(args: string[]): Promise<string> {
    const res = await this.try(args);
    if (res.code !== 0) throw new GitError(args, res);
    return res.stdout;
  }

  async head(): Promise<string> {
    return (await this.run(['rev-parse', 'HEAD'])).trim();
  }

  async revParse(ref: string): Promise<string | null> {
    const res = await this.try(['rev-parse', '--verify', '--quiet', `${ref}^{commit}`]);
    return res.code === 0 ? res.stdout.trim() : null;
  }

  async isDirty(): Promise<boolean> {
    return (await this.run(['status', '--porcelain'])).trim() !== '';
  }

  async isAncestor(ancestor: string, descendant: string): Promise<boolean> {
    return (await this.try(['merge-base', '--is-ancestor', ancestor, descendant])).code === 0;
  }

  async changedFiles(base: string, head: string): Promise<string[]> {
    const out = await this.run(['diff', '--name-only', `${base}`, head]);
    return out.split('\n').filter(Boolean);
  }

  async message(ref = 'HEAD'): Promise<string> {
    return this.run(['log', '-1', '--format=%B', ref]);
  }
}

export const CANDIDATE_TRAILER = 'Ratchet-Candidate: true';

/** Add the candidate trailer to a commit message (idempotent), separated by a blank line from the body. */
export function withCandidateTrailer(message: string): string {
  const body = message.replace(/\s+$/, '');
  if (hasCandidateTrailer(body)) return `${body}\n`;
  const lastParagraph = body.split(/\n\s*\n/).pop() ?? '';
  const isTrailerBlock = lastParagraph.split('\n').every((l) => /^[A-Za-z][A-Za-z0-9-]*: \S/.test(l)) && body.includes('\n\n');
  return `${body}${isTrailerBlock ? '\n' : '\n\n'}${CANDIDATE_TRAILER}\n`;
}

export function hasCandidateTrailer(message: string): boolean {
  return message.split('\n').some((l) => /^Ratchet-Candidate:\s*true\s*$/i.test(l));
}
