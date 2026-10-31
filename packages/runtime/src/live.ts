/**
 * Live runtime: the real Engine wired to real ports.
 * NOT exercised against a real account yet — see docs/REAL_ENV_TESTING.md for the test order.
 *
 * - Sandbox: TofuSandboxProvider (bundled `tofu`, infra/aws)
 * - AWS: AwsCloudCompute (EC2, IAM, SSM), AwsPriceBook, S3ArtifactStore
 * - Git: GitHubGitHost (repo-scoped token)
 * - Bench: RemoteBenchRunner (pinned EC2 bench instance via SSM)
 * - Agents: where the optimization says. `local` (the default of `createHybridRuntime`): the real Claude Code / Codex adapters on this
 *   machine through LocalRunnerControl, the user's own CLI login, one git worktree per agent. `cloud`: SsmRunnerControl (headless
 *   Claude Code / Codex on agent runners, needs API keys). The evaluator is always the cloud sandbox.
 *
 * The runtime is built in stages so the sandbox can be applied from the app: with `sandbox: null` (nothing applied
 * yet) only the tofu-backed sandbox ports work; everything that needs the sandbox's outputs, the GitHub repo or the
 * AMIs is an "unready" port whose calls fail with a `not_configured` error that says what is missing. Apply the
 * sandbox, then build the runtime again with its outputs (from the ledger).
 *
 * With `autoSandbox` (the product default, see `createHybridRuntime`) there is no such staging: the engine creates the sandbox at the
 * first run start and destroys it after the idle window, and the AWS-backed ports are late-bound: each call reads the sandbox's outputs
 * from the ledger, so they work as soon as the sandbox is applied and fail with `not_configured` before that.
 */
import { join } from 'node:path';
import type { Clock, Ledger } from '@ratchet/core';
import { AgentEnvironmentDrafter, Engine, EngineError, type EngineSettings } from '@ratchet/engine';
import {
  AwsCloudCompute,
  AwsCostReconciler,
  AwsPriceBook,
  S3ArtifactStore,
  StaticPriceBook,
  TofuEnvironmentChecker,
  TofuSandboxProvider,
  awsComputeConfigFromOutputs,
  regionForOptimization,
  resolveImageRefs,
  type AwsSandboxOutputs,
  type ProcessRunner,
} from '@ratchet/aws';
import { GitHubGitHost } from '@ratchet/github';
import { RemoteBenchRunner } from '@ratchet/bench';
import {
  SsmRunnerControl,
  createLocalAgentControl,
  detectHarnesses,
  gitAuthEnv,
  localAdapterFor,
  resolveLoginPath,
  type DetectOptions,
  type HarnessInfo,
  type LocalAgentControlOptions,
  type LocalRunnerControl,
} from '@ratchet/agents';
import type { AgentsWhere, EnvironmentDrafter } from '@ratchet/core';
import { systemClock } from './clock.js';

export interface LiveOptions {
  ledger: Ledger;
  region: string;
  /**
   * Outputs of the applied sandbox (from the ledger / TofuSandboxProvider.outputs): bench subnet, split
   * bench/confirm profiles, proxy. `null` before the first apply: the runtime then only plans and applies.
   */
  sandbox: AwsSandboxOutputs | null;
  sandboxId: string;
  /** `owner/name` of the repository to optimize. Without it (or the token) git, bench and runners are unready. */
  githubRepo?: string | undefined;
  githubToken?: string | undefined;
  /**
   * AMI override for agent runners and bench instances ("Advanced", development only). Normally left unset: the AMIs are found by
   * tag (`ratchet:image=runner|bench` + `ratchet:image_version` matching `appVersion`) by `resolveLiveImages` /
   * `createLiveRuntimeFor`, so the user never types an AMI id.
   */
  runnerImageRef?: string | undefined;
  benchImageRef?: string | undefined;
  /** The app version the published AMIs are matched against (`ratchet:image_version`). */
  appVersion?: string | undefined;
  /**
   * Validate the module's outputs against infra/contract.md after every apply even for the module that ships with the app. Not
   * needed for `infra:` in ratchet.yaml or a drafted environment: the engine reads those from the protected ref and hands them to
   * the sandbox provider, which validates them by itself (a custom module always, a drafted one against the base module's allow-list).
   */
  infraOverride?: boolean | undefined;
  /** Directory of the base module drafted environments are built on (`infra/modules/sandbox`); default: next to `moduleDir`. */
  baseModuleDir?: string | undefined;
  /**
   * Candidate setup run on the bench before the guardrails, e.g. `pnpm install --frozen-lockfile && pnpm build`. The bench has no
   * network: when this is set, dependencies come from a snapshot of the lockfile at `evaluator.ref` that a separate instance with
   * egress builds once (`evaluator.deps`, docs/real-env/bench.md), and a candidate that changes the lockfile is blocked.
   */
  benchSetup?: string | undefined;
  /** Pinned `tofu` binary and the infra/aws module directory. */
  tofuBinary: string;
  moduleDir: string;
  stateRoot: string;
  /** Model API keys etc., looked up by agent kind + secret name. */
  secrets?: EngineSettings['secrets'];
  /** Named AWS profile (static, SSO or assume-role) used by every AWS client and by tofu. */
  awsProfile?: string | undefined;
  /** Extra environment for tofu (merged over AWS_PROFILE). */
  awsEnv?: Record<string, string>;
  /** Replace the process runner that spawns `tofu` (tests). */
  tofuRunner?: ProcessRunner;
  clock?: Clock;
  settings?: EngineSettings;
  /**
   * Where agents run by default. `local` needs `localAgents`. Default: `local` when `localAgents` is given, else `cloud` (the
   * behaviour before local agents existed). An optimization can still choose (`createOptimization({ agentsWhere })`).
   */
  agentsWhere?: AgentsWhere | undefined;
  /** Agents on this machine: the detected harnesses and the login-shell PATH. See `createHybridRuntime`, which detects them. */
  localAgents?: LocalAgentsConfig | undefined;
  /**
   * The engine creates the sandbox at the first run start and destroys it after `sandboxIdleMs` without a run (default 30 min); the
   * AWS ports bind to it late. Default false here (staged apply from Environments), true in `createHybridRuntime`.
   */
  autoSandbox?: boolean | undefined;
  sandboxIdleMs?: number | undefined;
}

export type LocalAgentsConfig = Pick<LocalAgentControlOptions, 'harnesses' | 'loginPath'> &
  Partial<Pick<LocalAgentControlOptions, 'rootDir' | 'baseEnv' | 'spawner' | 'loop' | 'stopGraceMs' | 'claude' | 'codex'>>;

export interface LiveRuntime {
  engine: Engine;
  sandbox: TofuSandboxProvider;
  /**
   * Drafts, revises and approves the environment of an optimization (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"). Null when
   * there is no local agent to draft with or no GitHub repository to commit to; the engine's `draftEnvironment` then reports `unavailable`.
   */
  environment: EnvironmentDrafter | null;
  /** The GitHub remote, or null while git is not configured. */
  repoUrl: string | null;
  /** What still blocks running an optimization (empty = fully wired). The sandbox ports work regardless. */
  missing: string[];
  /** Where agents run unless the optimization says otherwise. */
  agentsWhere: AgentsWhere;
  /** The local agents control (null when no `localAgents` were given). */
  localRunners: LocalRunnerControl | null;
  /** The harnesses the local agents were built from (empty when none). */
  harnesses: HarnessInfo[];
  dispose(): Promise<void>;
}

/** A port whose every call fails with a clear, non-secret message about what has to be configured first. */
function unready<T extends object>(port: string, needs: string[]): T {
  const message = `${port} is not ready: ${needs.join('; ')}`;
  return new Proxy({} as T, {
    get: (_target, prop) => {
      if (prop === 'then' || typeof prop === 'symbol') return undefined; // not thenable, inspectable
      return async () => {
        throw new EngineError('not_configured', message);
      };
    },
  });
}

/** The AWS-backed ports, built from one sandbox's outputs. */
interface AwsPorts {
  compute: AwsCloudCompute;
  artifacts: S3ArtifactStore;
  bench: RemoteBenchRunner | undefined;
  runners: SsmRunnerControl;
}

/** A port that resolves its target on every call: the sandbox may be created (or replaced) after the runtime was built. */
function lateBound<T extends object>(port: string, target: () => T | undefined, needs: string[]): T {
  return new Proxy({} as T, {
    get: (_t, prop) => {
      if (prop === 'then' || typeof prop === 'symbol') return undefined;
      return async (...args: unknown[]) => {
        const t = target();
        if (!t) throw new EngineError('not_configured', `${port} is not ready: ${needs.join('; ')}`);
        return (t as Record<string, (...a: unknown[]) => unknown>)[prop]!.apply(t, args);
      };
    },
  });
}

const GITHUB_NEEDED = 'set the GitHub repository and token (Settings)';

export function createLiveRuntime(opts: LiveOptions): LiveRuntime {
  const clock = opts.clock ?? systemClock;
  const profile = opts.awsProfile ? { profile: opts.awsProfile } : {};
  const dynamic = opts.autoSandbox === true;
  const agentsWhere: AgentsWhere = opts.agentsWhere ?? (opts.localAgents ? 'local' : 'cloud');
  if (agentsWhere === 'local' && !opts.localAgents) throw new Error('agentsWhere is local but no localAgents (detected harnesses) were given');
  const missing: string[] = [];
  const needSandbox = 'apply the sandbox first (Environments)';
  if (!opts.sandbox && !dynamic) missing.push(needSandbox);
  const haveGit = !!opts.githubRepo && !!opts.githubToken;
  if (!haveGit) missing.push(GITHUB_NEEDED);
  if (agentsWhere === 'cloud' && !opts.runnerImageRef) missing.push('set the runner AMI (Settings)');
  if (!opts.benchImageRef) missing.push('set the bench AMI (Settings)');
  if (agentsWhere === 'local' && opts.localAgents && !opts.localAgents.harnesses.some((h) => h.installed && h.auth !== 'signed-out')) {
    missing.push('install and sign in to Claude Code or Codex on this machine (Settings > Agents)');
  }

  const git = haveGit
    ? new GitHubGitHost({ repo: opts.githubRepo!, token: opts.githubToken!, cacheDir: join(opts.stateRoot, 'git-cache') })
    : unready<GitHubGitHost>('GitHub', [GITHUB_NEEDED]);

  // The outputs the AWS ports are built from: the ones given, else (auto sandbox) the applied AWS sandbox in the ledger.
  const outputsNow = (): { out: AwsSandboxOutputs; region: string; sandboxId: string } | null => {
    if (opts.sandbox) return { out: opts.sandbox, region: opts.region, sandboxId: opts.sandboxId };
    if (!dynamic) return null;
    const rows = opts.ledger.listSandboxes().filter((r) => r.provider === 'aws' && r.status === 'applied' && r.outputs);
    const row = rows.find((r) => r.id === opts.sandboxId) ?? rows[0]; // listSandboxes is newest first
    return row ? { out: row.outputs as unknown as AwsSandboxOutputs, region: row.region, sandboxId: row.id } : null;
  };
  let cached: { key: string; ports: AwsPorts } | undefined;
  const awsNow = (): AwsPorts | undefined => {
    const cur = outputsNow();
    if (!cur) return undefined;
    const key = `${cur.sandboxId}\0${cur.region}\0${JSON.stringify(cur.out)}`;
    if (cached?.key === key) return cached.ports;
    // Derived from outputs so the private bench subnet, the confirm-only profile and the egress proxy are never dropped.
    const compute = new AwsCloudCompute({ ...awsComputeConfigFromOutputs(cur.out, cur.sandboxId), region: cur.region }, { clock, ...profile });
    const artifacts = new S3ArtifactStore({ bucket: cur.out.artifactsBucket, region: cur.region, ...profile });
    const bench =
      haveGit && opts.benchImageRef
        ? new RemoteBenchRunner({
            compute,
            git: git as GitHubGitHost,
            clock,
            imageRef: opts.benchImageRef,
            artifacts,
            // Bench instances have no internet: inputs are staged as tarballs under bench-inputs/ in the artifacts bucket.
            stagingBucket: cur.out.artifactsBucket,
            ...(opts.benchSetup ? { setup: opts.benchSetup } : {}),
          })
        : undefined;
    const runners = new SsmRunnerControl({ compute, store: artifacts, runnerEnv: { RATCHET_ARTIFACTS_BUCKET: cur.out.artifactsBucket, AWS_REGION: cur.region } });
    cached = { key, ports: { compute, artifacts, bench, runners } };
    return cached.ports;
  };
  const eager = !dynamic && opts.sandbox ? awsNow() : undefined; // a contract piece missing from the outputs fails here, as it always did

  const aws = <K extends keyof AwsPorts>(name: string, key: K, needs: string[]): NonNullable<AwsPorts[K]> => {
    type P = NonNullable<AwsPorts[K]>;
    if (dynamic) return lateBound<P & object>(name, () => awsNow()?.[key] as P | undefined, needs) as P;
    if (eager?.[key]) return eager[key] as P;
    return unready<P & object>(name, needs) as P;
  };
  const compute = aws('AWS compute', 'compute', [needSandbox]);
  const artifacts = aws('artifact store', 'artifacts', [needSandbox]);
  const benchNeeds = [!opts.sandbox && !dynamic ? needSandbox : '', !haveGit ? GITHUB_NEEDED : '', !opts.benchImageRef ? 'set the bench AMI (Settings)' : ''].filter(Boolean);
  const bench = aws('bench runner', 'bench', benchNeeds.length ? benchNeeds : [needSandbox]);
  const runners = aws('agent runners', 'runners', [needSandbox]);

  const sandbox = new TofuSandboxProvider({
    tofuBinary: opts.tofuBinary,
    moduleDir: opts.moduleDir,
    stateRoot: opts.stateRoot,
    env: { ...(opts.awsProfile ? { AWS_PROFILE: opts.awsProfile } : {}), ...opts.awsEnv },
    ...(opts.tofuRunner ? { runner: opts.tofuRunner } : {}),
    ...(opts.infraOverride ? { validateContract: true } : {}),
    ...(opts.baseModuleDir ? { baseModuleDir: opts.baseModuleDir } : {}),
    // The machines of a drafted environment launch from the same bench image the evaluator uses.
    ...(opts.benchImageRef ? { machineImageId: opts.benchImageRef } : opts.appVersion ? { imageVersion: opts.appVersion } : {}),
    // Teardown guard: runtime agent roles must be gone before `tofu destroy`. No outputs = nothing was ever launched.
    listRuntimeAgentRoles: async () => {
      const ports = awsNow();
      return ports ? (await ports.compute.listTagged()).filter((i) => i.role === 'agent_runner' && i.state !== 'terminated').map((i) => i.id) : [];
    },
  });

  const prices = new AwsPriceBook({ region: opts.region, clock, fallback: new StaticPriceBook(), ...profile });

  // Agents on this machine: the real adapters, the user's login, a git worktree each. Git credentials stay with the control.
  const localRunners = opts.localAgents
    ? createLocalAgentControl({
        ...opts.localAgents,
        rootDir: opts.localAgents.rootDir ?? join(opts.stateRoot, 'local-agents'),
        clock,
        ...(haveGit ? { gitEnv: gitAuthEnv((git as GitHubGitHost).remoteUrl(), opts.githubToken!) } : {}),
      })
    : null;

  // The setup agent that drafts environments: the user's own Claude Code / Codex on this machine, plus `tofu plan` in their account.
  const environment: EnvironmentDrafter | null =
    opts.localAgents && haveGit
      ? new AgentEnvironmentDrafter({
          git,
          adapterFor: (kind) => localAdapterFor({ ...opts.localAgents!, clock }, kind === 'mock' ? 'claude-code' : kind),
          clock,
          prices,
          defaultRegion: opts.region,
          scratchDir: join(opts.stateRoot, 'scratch'),
          checker: new TofuEnvironmentChecker({
            tofuBinary: opts.tofuBinary,
            moduleDir: opts.moduleDir,
            ...(opts.baseModuleDir ? { baseModuleDir: opts.baseModuleDir } : {}),
            workRoot: join(opts.stateRoot, 'scratch'),
            ...(opts.tofuRunner ? { runner: opts.tofuRunner } : {}),
            env: { ...(opts.awsProfile ? { AWS_PROFILE: opts.awsProfile } : {}), ...opts.awsEnv },
            ...(opts.benchImageRef ? { machineImageId: opts.benchImageRef } : opts.appVersion ? { imageVersion: opts.appVersion } : {}),
          }),
        })
      : null;

  const engine = new Engine({
    ledger: opts.ledger,
    mode: 'live',
    clock,
    ports: { sandbox, compute, prices, artifacts, git, runners, bench, ...(localRunners ? { localRunners } : {}), ...(environment ? { environment } : {}), costs: new AwsCostReconciler() },
    settings: {
      ...(opts.secrets ? { secrets: opts.secrets } : {}),
      ...(opts.runnerImageRef ? { runnerImageRef: opts.runnerImageRef } : {}),
      scratchDir: join(opts.stateRoot, 'scratch'),
      agentsWhere,
      sandbox: { auto: dynamic, defaultRegion: opts.region, ...(opts.sandboxIdleMs !== undefined ? { idleMs: opts.sandboxIdleMs } : {}) },
      ...opts.settings,
    },
  });

  return {
    engine,
    sandbox,
    environment,
    repoUrl: haveGit ? (git as GitHubGitHost).remoteUrl() : null,
    missing,
    agentsWhere,
    localRunners,
    harnesses: opts.localAgents?.harnesses ?? [],
    async dispose() {
      await engine.close();
      // Local agents die with the app; stopping them here also removes their worktrees.
      await localRunners?.stopAll().catch(() => undefined);
    },
  };
}

export interface HybridOptions extends Omit<LiveOptions, 'localAgents' | 'autoSandbox' | 'agentsWhere'> {
  /** Default `local`. */
  agentsWhere?: AgentsWhere | undefined;
  /** Default true: no Environments step, the sandbox appears with the first run and goes away after the idle window. */
  autoSandbox?: boolean | undefined;
  /** Harnesses already detected (the app caches them); otherwise `detectHarnesses` runs. */
  harnesses?: HarnessInfo[] | undefined;
  /** The login-shell PATH already resolved; otherwise `resolveLoginPath` runs. */
  loginPath?: string | undefined;
  /** Injection for the detection probes (tests). */
  detect?: DetectOptions | undefined;
  /** Rest of the local agents configuration (worktree root, base env, adapter options). */
  localAgents?: Omit<LocalAgentsConfig, 'harnesses' | 'loginPath'> | undefined;
}

/**
 * The product runtime: agents on this machine by default (`agentsWhere: 'cloud'` for unattended runs), the evaluator in the cloud
 * sandbox (RemoteBenchRunner on AWS compute), and the sandbox itself managed by the engine. Probes the machine for Claude Code and
 * Codex unless the caller already has the result.
 */
export async function createHybridRuntime(opts: HybridOptions): Promise<LiveRuntime> {
  const { harnesses: given, loginPath: givenPath, detect, localAgents, agentsWhere, autoSandbox, ...rest } = opts;
  const loginPath = givenPath ?? (await resolveLoginPath(detect)).path;
  const harnesses = given ?? (await detectHarnesses({ ...detect, loginPath }));
  return createLiveRuntime({ ...rest, agentsWhere: agentsWhere ?? 'local', autoSandbox: autoSandbox ?? true, localAgents: { ...localAgents, harnesses, loginPath } });
}

/**
 * Fills `runnerImageRef` / `benchImageRef` from the published AMIs (EC2 DescribeImages by tag and `appVersion`) when they are not
 * given. An explicit ref always wins. Without `appVersion` nothing is looked up (the refs stay as given, and `missing` says so).
 * Lookup errors are `ImageLookupError`s that say which image to build.
 */
export async function resolveLiveImages(opts: LiveOptions, lookup: Partial<Parameters<typeof resolveImageRefs>[0]> = {}): Promise<LiveOptions> {
  if (!opts.appVersion || (opts.runnerImageRef && opts.benchImageRef)) return opts;
  const need = (['runner', 'bench'] as const).filter((k) => !(k === 'runner' ? opts.runnerImageRef : opts.benchImageRef));
  const r = await resolveImageRefs({
    appVersion: opts.appVersion,
    region: opts.region,
    need,
    profile: opts.awsProfile,
    ...lookup,
  });
  return { ...opts, runnerImageRef: opts.runnerImageRef ?? r.runner, benchImageRef: opts.benchImageRef ?? r.bench };
}

/**
 * The live runtime for ONE optimization: the region comes from the repo (`environment.region` in ratchet.yaml, else the settings'
 * region, else us-east-1) and the AMIs are found by tag in that region. `config` is the parsed ratchet.yaml (read defensively).
 */
export async function createLiveRuntimeFor(opts: Omit<LiveOptions, 'region'> & { region?: string | undefined }, config: unknown, lookup?: Parameters<typeof resolveLiveImages>[1]): Promise<LiveRuntime> {
  const region = regionForOptimization(config, opts.region);
  const withRegion: LiveOptions = { ...opts, region };
  // Only look images up once a sandbox exists in that region: before the first apply there is nothing to launch.
  const resolved = opts.sandbox ? await resolveLiveImages(withRegion, lookup) : withRegion;
  return createLiveRuntime(resolved);
}
