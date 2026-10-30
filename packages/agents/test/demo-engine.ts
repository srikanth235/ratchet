import { execFileSync } from 'node:child_process';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, appendFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { AgentEvent, CandidateVerdict, RunnerConfig, RunnerMessage } from '@ratchet/core';
import { baseConfig, sh, type Sandbox } from './harness.js';

export const REPO_ROOT = resolve(fileURLToPath(new URL('../../..', import.meta.url)));
export const DEMO_DIR = join(REPO_ROOT, 'examples/demo-search');
export const PROTECTED = ['bench/', 'ratchet.yaml', 'tests/contract/', 'SOLUTIONS.md'];

/** Copy examples/demo-search into its own git remote (main + one branch per agent + ratchet/best). */
export function makeDemoSandbox(agents: string[]): Sandbox {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-demo-'));
  const remote = join(dir, 'remote.git');
  const seed = join(dir, 'seed');
  cpSync(DEMO_DIR, seed, { recursive: true, filter: (src) => !/[\\/](node_modules|dist)([\\/]|$)/.test(src) });
  execFileSync('git', ['init', '--quiet', '--bare', '--initial-branch=main', remote]);
  sh(seed, 'init', '--quiet', '--initial-branch=main');
  sh(seed, 'add', '-A');
  sh(seed, 'commit', '--quiet', '-m', 'demo-search baseline');
  sh(seed, 'remote', 'add', 'origin', remote);
  sh(seed, 'push', '--quiet', 'origin', 'main', 'main:refs/heads/ratchet/best', ...agents.map((a) => `main:refs/heads/ratchet/${a}`));
  const engine = join(dir, 'engine');
  execFileSync('git', ['clone', '--quiet', remote, engine]);
  return { dir, remote, engine, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

/** Make `dir` (a clone of the demo) able to run the demo's tests, without the link being committed. */
export function linkNodeModules(dir: string): void {
  mkdirSync(join(dir, 'node_modules'), { recursive: true });
  for (const [name, target] of [['yaml', join(DEMO_DIR, 'node_modules/yaml')], ['vitest', join(REPO_ROOT, 'node_modules/vitest')]] as const) {
    if (!existsSync(join(dir, 'node_modules', name))) symlinkSync(target, join(dir, 'node_modules', name), 'dir');
  }
  // demo-search's tsconfig extends ../../tsconfig.base.json
  copyFileSync(join(REPO_ROOT, 'tsconfig.base.json'), join(dir, '..', '..', 'tsconfig.base.json'));
  appendFileSync(join(dir, '.git', 'info', 'exclude'), '\nnode_modules\n');
}

export const demoTestsPass = (dir: string): boolean => {
  try {
    execFileSync(process.execPath, [join(REPO_ROOT, 'node_modules/vitest/vitest.mjs'), 'run'], { cwd: dir, stdio: 'pipe', timeout: 120_000 });
    return true;
  } catch {
    return false;
  }
};

export function demoConfig(sb: Sandbox, agentId: string, over: Partial<RunnerConfig> = {}): RunnerConfig {
  const base = baseConfig(sb, { agentId, branch: `ratchet/${agentId}`, kind: 'mock', deadlineAt: Date.now() + 365 * 24 * 3600_000, ...over });
  return {
    ...base,
    brief: readFileSync(join(DEMO_DIR, 'RATCHET.md'), 'utf8'),
    config: { ...base.config, protected: ['bench/**', 'ratchet.yaml', 'tests/contract/**', 'SOLUTIONS.md'] },
  };
}

export interface EngineOptions {
  /** Run the demo's real test suite on each unblocked candidate (slow, exact) instead of the structural check. */
  realTests?: boolean;
  /** Where the candidate is checked out for tests (the agent's workdir). */
  workdirOf?: (agentId: string) => string;
}

export interface Decision {
  agentId: string;
  sha: string;
  verdict: CandidateVerdict;
  reasons: string[];
  files: string[];
}

/**
 * A fake engine for the demo: protected-path check, guardrails (tests), a noise rule, and a shared ratchet best
 * on the remote (`ratchet/best`). A verified candidate that does not descend from the best is cherry-picked onto it.
 */
export class DemoEngine {
  readonly decisions: Decision[] = [];
  bestValue = 20;
  private queue: Promise<void> = Promise.resolve();

  constructor(private readonly sb: Sandbox, private readonly options: EngineOptions = {}) {
    sh(sb.engine, 'config', 'user.name', 'engine');
    sh(sb.engine, 'config', 'user.email', 'engine@ratchet.local');
  }

  /** Subscribe: returns an `emit` handler for one agent that answers its commit events via `push`. */
  handler(agentId: string, push: (m: RunnerMessage) => void): (e: AgentEvent) => void {
    return (e) => {
      if (e.type !== 'commit') return;
      const sha = String(e.data?.['sha']);
      const files = (e.data?.['files'] as string[]) ?? [];
      // serialize decisions: they move the shared best
      this.queue = this.queue.then(() => {
        const d = this.decide(agentId, sha, files);
        this.decisions.push(d);
        push({ type: 'verdict', sha, verdict: d.verdict, reasons: d.reasons, currentBest: this.bestValue });
      });
    };
  }

  private decide(agentId: string, sha: string, files: string[]): Decision {
    const make = (verdict: CandidateVerdict, reasons: string[] = []): Decision => ({ agentId, sha, verdict, reasons, files });
    const touched = files.filter((f) => PROTECTED.some((p) => f === p || f.startsWith(p)));
    if (touched.length) return make('blocked', [`protected path changed: ${touched.join(', ')}`]);

    sh(this.sb.engine, 'fetch', '--quiet', 'origin', `+refs/heads/ratchet/${agentId}:refs/remotes/origin/ratchet/${agentId}`);
    const search = sh(this.sb.engine, 'show', `${sha}:src/search.ts`);
    const filterTs = sh(this.sb.engine, 'show', `${sha}:src/filter.ts`);
    if (this.options.realTests && this.options.workdirOf) {
      if (!demoTestsPass(this.options.workdirOf(agentId))) return make('blocked', ['guardrail failed: pnpm test']);
    } else if (/TAG_WEIGHT = 0;/.test(search)) {
      return make('blocked', ['guardrail failed: pnpm test:contract']);
    }

    // a candidate whose only change is the equivalent filter(Boolean) rewrite is below the noise floor
    const diff = sh(this.sb.engine, 'diff', `${sha}~1`, sha);
    if (/\+\s*return q\.toLowerCase\(\)\.split\(.*\)\.filter\(\(t\) => t\.length > 0\)/.test(diff) && files.length === 1) {
      return make('rejected', ['gain 0.6% is below the noise floor (4.1%)']);
    }
    void filterTs;

    // verified: move the shared best (cherry-pick when the candidate does not descend from it)
    sh(this.sb.engine, 'fetch', '--quiet', 'origin');
    const best = sh(this.sb.engine, 'rev-parse', 'origin/ratchet/best');
    let newBest = sha;
    if (execGitOk(this.sb.engine, ['merge-base', '--is-ancestor', best, sha])) {
      newBest = sha;
    } else {
      sh(this.sb.engine, 'checkout', '--quiet', '--detach', best);
      try {
        sh(this.sb.engine, 'cherry-pick', '--allow-empty', '--keep-redundant-commits', sha);
        newBest = sh(this.sb.engine, 'rev-parse', 'HEAD');
      } catch {
        sh(this.sb.engine, 'cherry-pick', '--abort');
        return make('rejected', ['does not apply on top of the current best']);
      }
    }
    sh(this.sb.engine, 'push', '--quiet', '--force', 'origin', `${newBest}:refs/heads/ratchet/best`);
    this.bestValue = Math.max(1, Math.round((this.bestValue * 0.72) * 100) / 100);
    return make('verified', [`p95 improved; best is now ${this.bestValue}`]);
  }
}

function execGitOk(cwd: string, args: string[]): boolean {
  try {
    execFileSync('git', args, { cwd, stdio: 'pipe' });
    return true;
  } catch {
    return false;
  }
}

export { mkdirSync };
