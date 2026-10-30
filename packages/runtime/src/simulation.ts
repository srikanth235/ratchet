/**
 * Simulation runtime: the real Engine wired to local / mock ports, so a whole
 * optimization runs on one machine against examples/demo-search.
 *
 * - AWS: MockSandboxProvider + MockCloudCompute (in-memory instances, real cost math)
 * - Git: LocalGitHost on a bare copy of the demo repo ("GitHub")
 * - Bench: LocalBenchRunner (real build, real tests, real load generator)
 * - Agents: LocalRunnerControl running the real runner loop with ScriptedAgent
 *   (real patches from packages/agents/scripts/demo-search)
 */
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import YAML from 'yaml';
import type { AgentKind, Clock, Ledger, SandboxProvider } from '@ratchet/core';
import { Engine, type EngineSettings } from '@ratchet/engine';
import { FsArtifactStore, MockCloudCompute, MockSandboxProvider, StaticPriceBook } from '@ratchet/aws';
import { LocalGitHost, createBareRemote } from '@ratchet/github';
import { LocalBenchRunner } from '@ratchet/bench';
import { LocalRunnerControl, ScriptedAgent, loadDemoSearchScript, type AgentScript } from '@ratchet/agents';
import { systemClock } from './clock.js';

export interface SimulationOptions {
  ledger: Ledger;
  /** Scratch root. Must live inside the monorepo so bench checkouts resolve the workspace's node_modules. */
  workRoot: string;
  /** The monorepo root (for node_modules/.bin and the demo source). */
  repoRoot: string;
  /** Source project to optimize. Default `<repoRoot>/examples/demo-search`. */
  projectDir?: string;
  /** Load-generator duration per bench run, seconds. Default 5. */
  benchDurationSec?: number;
  /** Bench repeats (overrides ratchet.yaml). Default 3. */
  repeats?: number;
  /** Agent scripts per kind. Default: demo-search `cc` for claude-code, `cx` for codex. */
  scripts?: Partial<Record<AgentKind, AgentScript>>;
  clock?: Clock;
  settings?: EngineSettings;
}

export interface SimulationRuntime {
  engine: Engine;
  sandbox: SandboxProvider;
  /** The in-memory cloud, exposed so tests can assert teardown left nothing tagged. */
  compute: MockCloudCompute;
  /** `repoUrl` to pass to createOptimization. */
  repoUrl: string;
  baseRef: string;
  git: LocalGitHost;
  dispose(): Promise<void>;
}

/** Copy the project into a bare "GitHub" remote, tuned for fast local benches, with a protected `ratchet/bench` ref. */
export async function prepareSimulationRepo(opts: Required<Pick<SimulationOptions, 'workRoot' | 'repoRoot'>> & Pick<SimulationOptions, 'projectDir' | 'benchDurationSec' | 'repeats'>) {
  const projectDir = resolve(opts.projectDir ?? join(opts.repoRoot, 'examples/demo-search'));
  const root = resolve(opts.workRoot);
  mkdirSync(root, { recursive: true });

  // Hidden ("sealed") trace: same distribution family, different seed, kept outside the repo.
  const sealedDir = join(root, 'sealed');
  mkdirSync(sealedDir, { recursive: true });
  const hidden = join(sealedDir, 'hidden-mix.json');
  const generator = join(projectDir, 'dist/bench/traces/generate.js');
  if (!existsSync(hidden) && existsSync(generator)) {
    // Inside Electron, execPath is the Electron binary: ELECTRON_RUN_AS_NODE makes it behave as plain Node (no-op under Node).
    execFileSync(process.execPath, [generator, '--seed', '424242', '--out', hidden], { cwd: projectDir, stdio: 'ignore', env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' } });
  }

  const remote = await createBareRemote(projectDir, { remoteDir: join(root, 'remote.git'), workDir: join(root, 'seed'), branch: 'main' });

  // Tune ratchet.yaml for local runs: node load generator instead of k6, short runs, local hidden trace.
  const cfgPath = join(remote.workDir, 'ratchet.yaml');
  const cfg = YAML.parse(readFileSync(cfgPath, 'utf8'));
  const duration = opts.benchDurationSec ?? 5;
  const ev = cfg.evaluator ?? cfg.bench; // `bench:` is the legacy alias of `evaluator:`
  ev.run = `node dist/bench/local.js --duration ${duration} --warmup 1 --mix "\${RATCHET_TRACE:-bench/query-mix.json}"`;
  ev.repeats = opts.repeats ?? 3;
  delete ev.confirm_trace;
  if (existsSync(hidden)) ev.holdout = hidden;
  else delete ev.holdout;
  writeFileSync(cfgPath, YAML.stringify(cfg));
  const git = (args: string[]) => execFileSync('git', args, { cwd: remote.workDir, env: { ...process.env, GIT_AUTHOR_NAME: 'Ratchet', GIT_AUTHOR_EMAIL: 'ratchet@localhost', GIT_COMMITTER_NAME: 'Ratchet', GIT_COMMITTER_EMAIL: 'ratchet@localhost' } });
  git(['-c', 'commit.gpgsign=false', 'commit', '-q', '-am', 'Simulation: local bench settings']);
  git(['push', '-q', 'origin', 'main']);
  // The trusted bench lives on a protected ref; agents never push here.
  git(['push', '-q', 'origin', `main:refs/heads/${ev.ref}`]);
  return { remoteDir: remote.remoteDir, baseRef: 'main', benchRef: ev.ref as string };
}

export async function createSimulationRuntime(opts: SimulationOptions): Promise<SimulationRuntime> {
  const clock = opts.clock ?? systemClock;
  const root = resolve(opts.workRoot);
  const repo = await prepareSimulationRepo({ workRoot: root, repoRoot: opts.repoRoot, ...(opts.projectDir ? { projectDir: opts.projectDir } : {}), ...(opts.benchDurationSec ? { benchDurationSec: opts.benchDurationSec } : {}), ...(opts.repeats ? { repeats: opts.repeats } : {}) });
  const git = new LocalGitHost({ remoteDir: repo.remoteDir, prDir: join(root, 'prs') });
  const bin = join(opts.repoRoot, 'node_modules/.bin');
  const benchScratch = join(root, 'bench');
  mkdirSync(benchScratch, { recursive: true });
  const artifacts = new FsArtifactStore(join(root, 'artifacts'));
  const compute = new MockCloudCompute({ clock });
  const sandbox = new MockSandboxProvider();

  const scripts: Record<string, AgentScript> = {
    'claude-code': opts.scripts?.['claude-code'] ?? loadDemoSearchScript('cc'),
    codex: opts.scripts?.codex ?? loadDemoSearchScript('cx'),
    mock: opts.scripts?.mock ?? loadDemoSearchScript('cc'),
  };

  const bench = new LocalBenchRunner({
    git,
    scratchRoot: benchScratch,
    clock,
    setup: `node ${join(opts.repoRoot, 'node_modules/typescript/bin/tsc')} -p tsconfig.build.json`,
    artifacts,
    env: { PATH: `${bin}:${process.env.PATH ?? ''}`, CI: '1' },
  });

  const runners = new LocalRunnerControl({
    rootDir: join(root, 'agents'),
    clock,
    adapterFor: (config) => new ScriptedAgent(scripts[config.kind] ?? scripts['claude-code']!, { clock }),
  });

  const engine = new Engine({
    ledger: opts.ledger,
    mode: 'simulation',
    clock,
    ports: { sandbox, compute, prices: new StaticPriceBook(), artifacts, git, runners, bench },
    settings: {
      secrets: () => 'simulation-placeholder-secret',
      scratchDir: join(root, 'scratch'),
      tickIntervalMs: 1000,
      ...opts.settings,
    },
  });

  return {
    engine,
    sandbox,
    compute,
    repoUrl: repo.remoteDir,
    baseRef: repo.baseRef,
    git,
    async dispose() {
      engine.close();
    },
  };
}
