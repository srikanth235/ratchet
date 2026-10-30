import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import type { AgentEvent, CandidateVerdict, RunnerConfig, RunnerMessage } from '@ratchet/core';

export const sh = (cwd: string, ...args: string[]): string =>
  execFileSync('git', args, { cwd, encoding: 'utf8', env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GIT_AUTHOR_NAME: 't', GIT_AUTHOR_EMAIL: 't@t', GIT_COMMITTER_NAME: 't', GIT_COMMITTER_EMAIL: 't@t' } }).trim();

export interface Sandbox {
  dir: string;
  remote: string;
  /** A scratch clone for the "engine" side (updating ratchet/best, reading branches). */
  engine: string;
  cleanup(): void;
}

/** A bare remote with `main` seeded from `files`, plus agent branches off main. */
export function makeSandbox(files: Record<string, string> = { 'src/app.txt': 'line 1\nline 2\nline 3\n', 'bench/run.sh': 'echo bench\n', 'README.md': '# demo\n' }, branches: string[] = ['ratchet/a']): Sandbox {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-test-'));
  const remote = join(dir, 'remote.git');
  const seed = join(dir, 'seed');
  mkdirSync(seed);
  execFileSync('git', ['init', '--quiet', '--bare', '--initial-branch=main', remote]);
  sh(seed, 'init', '--quiet', '--initial-branch=main');
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(seed, path)), { recursive: true });
    writeFileSync(join(seed, path), content);
  }
  sh(seed, 'add', '-A');
  sh(seed, 'commit', '--quiet', '-m', 'seed');
  sh(seed, 'remote', 'add', 'origin', remote);
  sh(seed, 'push', '--quiet', 'origin', 'main');
  for (const b of branches) sh(seed, 'push', '--quiet', 'origin', `main:refs/heads/${b}`);
  const engine = join(dir, 'engine');
  execFileSync('git', ['clone', '--quiet', remote, engine]);
  return { dir, remote, engine, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

/** Engine side: point `ratchet/best` at `sha` (fetching it from the remote first). */
export function setBest(sb: Sandbox, sha: string): void {
  sh(sb.engine, 'fetch', '--quiet', 'origin');
  sh(sb.engine, 'push', '--quiet', '--force', 'origin', `${sha}:refs/heads/ratchet/best`);
}

export const baseConfig = (sb: Sandbox, over: Partial<RunnerConfig> = {}): RunnerConfig => ({
  optimizationId: 'opt1',
  agentId: 'a',
  kind: 'mock',
  repoUrl: sb.remote,
  branch: 'ratchet/a',
  baseRef: 'main',
  config: {
    version: 0,
    objective: { metric: 'http_req_duration.p95', direction: 'minimize', min_gain: 0.03 },
    ...(() => {
      const evaluator = { kind: 'benchmark' as const, ref: 'ratchet/bench', run: 'k6 run bench/k6/search.js', repeats: 5, hardware: 'c7i.2xlarge', deps: 'auto' as const };
      return { evaluator, bench: evaluator };
    })(),
    guardrails: [{ kind: 'command', run: 'pnpm test' }, { kind: 'threshold', metric: 'error_rate', op: '<', value: 0.001, relative: false, raw: '< 0.001' }],
    protected: ['bench/**', 'ratchet.yaml'],
    agent: { instructions: 'RATCHET.md', allowed_instance_types: ['c7i.2xlarge'] },
  },
  brief: '# Brief\n\nMake it faster. Do not touch bench/**.\n',
  baselineValue: 20,
  budgetUsd: 50,
  deadlineAt: Date.now() + 60_000,
  ...over,
});

export type VerdictRule = (commit: { sha: string; files: string[]; n: number }) => { verdict: CandidateVerdict; reasons?: string[]; best?: number } | undefined;

/**
 * Plays the engine: answers every `commit` event with a verdict (default: verified, best improves 10 per win).
 * `onVerdict` runs before the verdict is delivered (e.g. to move ratchet/best).
 */
export function autoVerdicts(opts: {
  push: (m: RunnerMessage) => void;
  rule?: VerdictRule;
  start?: number;
  onVerdict?: (sha: string, verdict: CandidateVerdict) => void;
  delayMs?: number;
}): (e: AgentEvent) => void {
  let best = opts.start ?? 20;
  let n = 0;
  return (e) => {
    if (e.type !== 'commit') return;
    const sha = String(e.data?.['sha']);
    const files = (e.data?.['files'] as string[]) ?? [];
    const decision = opts.rule?.({ sha, files, n: ++n }) ?? { verdict: 'verified' as CandidateVerdict };
    if (decision.verdict === 'verified') best = decision.best ?? best - 2;
    opts.onVerdict?.(sha, decision.verdict);
    const deliver = () => opts.push({ type: 'verdict', sha, verdict: decision.verdict, reasons: decision.reasons ?? [], currentBest: best });
    if (opts.delayMs) setTimeout(deliver, opts.delayMs);
    else deliver();
  };
}

export async function until(cond: () => boolean | Promise<boolean>, ms = 5000, what = 'condition'): Promise<void> {
  const start = Date.now();
  while (!(await cond())) {
    if (Date.now() - start > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}
