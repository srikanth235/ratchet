/**
 * Simulation runtime: the real Engine wired to local / mock ports, so a whole
 * optimization runs on one machine against examples/demo-search.
 *
 * - AWS: MockSandboxProvider + MockCloudCompute (in-memory instances, real cost math)
 * - Git: LocalGitHost on a bare copy of the demo repo ("GitHub")
 * - Bench: LocalBenchRunner (real build, real tests, real evaluator)
 * - Agents: LocalRunnerControl running the real runner loop with ScriptedAgent
 *   (real patches from packages/agents/scripts/<demo>)
 *
 * Pack-aware: pick a demo with `project` ('search' = perf pack, 'forecast' = forecasting pack). A demo adapts its
 * ratchet.yaml for local runs with an optional `ratchet.sim.yaml` overlay (see `SimOverlay`).
 */
import { mkdirSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import YAML from 'yaml';
import { parseConfig, type AgentKind, type Clock, type Ledger, type RatchetConfig, type SandboxProvider } from '@ratchet/core';
import { Engine, type EngineSettings } from '@ratchet/engine';
import { FsArtifactStore, MockCloudCompute, MockSandboxProvider, StaticPriceBook } from '@ratchet/aws';
import { LocalGitHost, createBareRemote } from '@ratchet/github';
import { LocalBenchRunner } from '@ratchet/bench';
import { LocalRunnerControl, ScriptedAgent, loadDemoForecastScript, loadDemoSearchScript, type AgentScript } from '@ratchet/agents';
import { systemClock } from './clock.js';

/** The shipped demo projects the simulator can optimize. */
export type SimulationProject = 'search' | 'forecast';

export const SIMULATION_PROJECTS: Record<SimulationProject, { dir: string; pack: string; scripts: (variant: 'cc' | 'cx') => AgentScript }> = {
  search: { dir: 'examples/demo-search', pack: 'perf', scripts: loadDemoSearchScript },
  forecast: { dir: 'examples/demo-forecast', pack: 'forecasting', scripts: loadDemoForecastScript },
};

export interface SimulationOptions {
  ledger: Ledger;
  /** Scratch root. Must live inside the monorepo so bench checkouts resolve the workspace's node_modules. */
  workRoot: string;
  /** The monorepo root (for node_modules/.bin and the demo source). */
  repoRoot: string;
  /** Which shipped demo to optimize. Default 'search'. Ignored for the source dir when `projectDir` is given. */
  project?: SimulationProject;
  /** Source project to optimize. Default: the `project`'s directory under `repoRoot`. */
  projectDir?: string;
  /** Load-generator duration per bench run, seconds (the overlay's `{duration}`). Default 5. */
  benchDurationSec?: number;
  /** Evaluator repeats (overrides the overlay and ratchet.yaml). Default: the overlay's, else ratchet.yaml's, else 3. */
  repeats?: number;
  /** Agent scripts per kind. Default: the project's `cc` for claude-code, `cx` for codex. */
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
  /** The simulated repo's ratchet.yaml (after the overlay), parsed: objective, value model, guardrails. */
  config: RatchetConfig;
  git: LocalGitHost;
  dispose(): Promise<void>;
}

/**
 * `ratchet.sim.yaml`: how a demo's ratchet.yaml is adapted for local runs.
 *
 *   sealed:                # optional: produce the sealed holdout/trace OUTSIDE the repo (run in the built project dir)
 *     run: node dist/eval/holdout.js --seed 424242 --out "{out}"
 *     file: sealed-holdout.json
 *   evaluator:             # any evaluator keys, merged over ratchet.yaml's
 *     run: node dist/eval/backtest.js
 *     repeats: 2            # numbers go unquoted; the `repeats` option overrides it
 *     holdout: "{sealed}"
 *
 * Placeholders in strings: `{out}` (path the sealed command must write), `{sealed}` (that path, for use in the
 * overlay) and `{duration}` (load-generator seconds). `evaluator.repeats`: the `repeats` option, else the overlay's,
 * else ratchet.yaml's, else 3. Without an overlay ratchet.yaml is used as it is.
 */
export interface SimOverlay {
  sealed?: { run: string; file: string };
  evaluator?: Record<string, unknown>;
}

const SIM_OVERLAY = 'ratchet.sim.yaml';

function fill(value: unknown, vars: Record<string, string>): unknown {
  if (typeof value === 'string') return value.replace(/\{(out|sealed|duration)\}/g, (_, k: string) => vars[k] ?? `{${k}}`);
  if (Array.isArray(value)) return value.map((v) => fill(v, vars));
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, fill(v, vars)]));
  return value;
}

/** Copy the project into a bare "GitHub" remote, adapted for fast local runs, with a protected `ratchet/bench` ref. */
export async function prepareSimulationRepo(opts: Required<Pick<SimulationOptions, 'workRoot' | 'repoRoot'>> & Pick<SimulationOptions, 'project' | 'projectDir' | 'benchDurationSec' | 'repeats'>) {
  const projectDir = resolve(opts.projectDir ?? join(opts.repoRoot, SIMULATION_PROJECTS[opts.project ?? 'search'].dir));
  const root = resolve(opts.workRoot);
  mkdirSync(root, { recursive: true });

  const overlayPath = join(projectDir, SIM_OVERLAY);
  const overlay: SimOverlay = existsSync(overlayPath) ? (YAML.parse(readFileSync(overlayPath, 'utf8')) as SimOverlay) : {};
  const duration = opts.benchDurationSec ?? 5;

  // Sealed holdout / hidden trace: same distribution family, a seed the agents never see, kept outside the repo.
  let sealed: string | undefined;
  if (overlay.sealed) {
    const sealedDir = join(root, 'sealed');
    mkdirSync(sealedDir, { recursive: true });
    sealed = join(sealedDir, overlay.sealed.file);
    if (!existsSync(sealed)) {
      const cmd = String(fill(overlay.sealed.run, { out: sealed }));
      try {
        execFileSync('sh', ['-c', cmd], { cwd: projectDir, stdio: 'pipe' });
      } catch (err) {
        throw new Error(`could not produce the sealed holdout with \`${cmd}\` in ${projectDir} (is the project built? run \`pnpm build\`): ${(err as Error).message.split('\n')[0]}`);
      }
    }
  }

  const remote = await createBareRemote(projectDir, { remoteDir: join(root, 'remote.git'), workDir: join(root, 'seed'), branch: 'main' });

  const cfgPath = join(remote.workDir, 'ratchet.yaml');
  const cfg = YAML.parse(readFileSync(cfgPath, 'utf8'));
  const ev = cfg.evaluator ?? cfg.bench; // `bench:` is the legacy alias of `evaluator:`
  const vars = { duration: String(duration), sealed: sealed ?? '' };
  Object.assign(ev, fill(overlay.evaluator ?? {}, vars) as Record<string, unknown>);
  if (opts.repeats) ev.repeats = opts.repeats;
  else if (ev.repeats === undefined) ev.repeats = 3;
  delete ev.confirm_trace;
  if (sealed) ev.holdout = sealed;
  else if (overlay.sealed === undefined && overlay.evaluator?.holdout === undefined) delete ev.holdout;
  writeFileSync(cfgPath, YAML.stringify(cfg));
  rmSync(join(remote.workDir, SIM_OVERLAY), { force: true });
  const git = (args: string[]) => execFileSync('git', args, { cwd: remote.workDir, env: { ...process.env, GIT_AUTHOR_NAME: 'Ratchet', GIT_AUTHOR_EMAIL: 'ratchet@localhost', GIT_COMMITTER_NAME: 'Ratchet', GIT_COMMITTER_EMAIL: 'ratchet@localhost' } });
  git(['-c', 'commit.gpgsign=false', 'commit', '-q', '-am', 'Simulation: local evaluator settings']);
  git(['push', '-q', 'origin', 'main']);
  // The trusted evaluator lives on a protected ref; agents never push here.
  git(['push', '-q', 'origin', `main:refs/heads/${ev.ref}`]);
  return { remoteDir: remote.remoteDir, baseRef: 'main', benchRef: ev.ref as string, config: cfg as Record<string, unknown> };
}

export async function createSimulationRuntime(opts: SimulationOptions): Promise<SimulationRuntime> {
  const clock = opts.clock ?? systemClock;
  const root = resolve(opts.workRoot);
  const project = opts.project ?? 'search';
  const repo = await prepareSimulationRepo({ workRoot: root, repoRoot: opts.repoRoot, project, ...(opts.projectDir ? { projectDir: opts.projectDir } : {}), ...(opts.benchDurationSec ? { benchDurationSec: opts.benchDurationSec } : {}), ...(opts.repeats ? { repeats: opts.repeats } : {}) });
  const parsed = parseConfig(YAML.stringify(repo.config));
  if (!parsed.ok) throw new Error(`simulated ratchet.yaml is invalid: ${parsed.errors.map((e) => `${e.path}: ${e.message}`).join('; ')}`);
  const git = new LocalGitHost({ remoteDir: repo.remoteDir, prDir: join(root, 'prs') });
  const bin = join(opts.repoRoot, 'node_modules/.bin');
  const benchScratch = join(root, 'bench');
  mkdirSync(benchScratch, { recursive: true });
  const artifacts = new FsArtifactStore(join(root, 'artifacts'));
  const compute = new MockCloudCompute({ clock });
  const sandbox = new MockSandboxProvider();

  const scripts: Record<string, AgentScript> = {
    'claude-code': opts.scripts?.['claude-code'] ?? SIMULATION_PROJECTS[project].scripts('cc'),
    codex: opts.scripts?.codex ?? SIMULATION_PROJECTS[project].scripts('cx'),
    mock: opts.scripts?.mock ?? SIMULATION_PROJECTS[project].scripts('cc'),
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
    config: parsed.config,
    git,
    async dispose() {
      engine.close();
    },
  };
}
