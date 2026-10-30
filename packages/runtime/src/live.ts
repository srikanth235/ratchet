/**
 * Live runtime: the real Engine wired to real ports.
 * NOT exercised against a real account yet — see docs/REAL_ENV_TESTING.md for the test order.
 *
 * - Sandbox: TofuSandboxProvider (bundled `tofu`, infra/aws)
 * - AWS: AwsCloudCompute (EC2, IAM, SSM), AwsPriceBook, S3ArtifactStore
 * - Git: GitHubGitHost (repo-scoped token)
 * - Bench: RemoteBenchRunner (pinned EC2 bench instance via SSM)
 * - Agents: SsmRunnerControl (headless Claude Code / Codex on agent runners)
 *
 * The runtime is built in stages so the sandbox can be applied from the app: with `sandbox: null` (nothing applied
 * yet) only the tofu-backed sandbox ports work; everything that needs the sandbox's outputs, the GitHub repo or the
 * AMIs is an "unready" port whose calls fail with a `not_configured` error that says what is missing. Apply the
 * sandbox, then build the runtime again with its outputs (from the ledger).
 */
import { join } from 'node:path';
import type { Clock, Ledger } from '@ratchet/core';
import { Engine, EngineError, type EngineSettings } from '@ratchet/engine';
import {
  AwsCloudCompute,
  AwsCostReconciler,
  AwsPriceBook,
  S3ArtifactStore,
  StaticPriceBook,
  TofuSandboxProvider,
  awsComputeConfigFromOutputs,
  type AwsSandboxOutputs,
  type ProcessRunner,
} from '@ratchet/aws';
import { GitHubGitHost } from '@ratchet/github';
import { RemoteBenchRunner } from '@ratchet/bench';
import { SsmRunnerControl } from '@ratchet/agents';
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
  /** AMIs built for agent runners and bench instances. */
  runnerImageRef?: string | undefined;
  benchImageRef?: string | undefined;
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
}

export interface LiveRuntime {
  engine: Engine;
  sandbox: TofuSandboxProvider;
  /** The GitHub remote, or null while git is not configured. */
  repoUrl: string | null;
  /** What still blocks running an optimization (empty = fully wired). The sandbox ports work regardless. */
  missing: string[];
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

export function createLiveRuntime(opts: LiveOptions): LiveRuntime {
  const clock = opts.clock ?? systemClock;
  const profile = opts.awsProfile ? { profile: opts.awsProfile } : {};
  const out = opts.sandbox;
  const missing: string[] = [];
  const needSandbox = 'apply the sandbox first (Environments)';
  if (!out) missing.push(needSandbox);
  const haveGit = !!opts.githubRepo && !!opts.githubToken;
  if (!haveGit) missing.push('set the GitHub repository and token (Settings)');
  if (!opts.runnerImageRef) missing.push('set the runner AMI (Settings)');
  if (!opts.benchImageRef) missing.push('set the bench AMI (Settings)');

  // Derived from outputs so the private bench subnet, the confirm-only profile and the egress proxy are never dropped.
  const compute = out
    ? new AwsCloudCompute({ ...awsComputeConfigFromOutputs(out, opts.sandboxId), region: opts.region }, { clock, ...profile })
    : unready<AwsCloudCompute>('AWS compute', [needSandbox]);
  const artifacts = out
    ? new S3ArtifactStore({ bucket: out.artifactsBucket, region: opts.region, ...profile })
    : unready<S3ArtifactStore>('artifact store', [needSandbox]);
  const git = haveGit
    ? new GitHubGitHost({ repo: opts.githubRepo!, token: opts.githubToken!, cacheDir: join(opts.stateRoot, 'git-cache') })
    : unready<GitHubGitHost>('GitHub', ['set the GitHub repository and token (Settings)']);

  const sandbox = new TofuSandboxProvider({
    tofuBinary: opts.tofuBinary,
    moduleDir: opts.moduleDir,
    stateRoot: opts.stateRoot,
    env: { ...(opts.awsProfile ? { AWS_PROFILE: opts.awsProfile } : {}), ...opts.awsEnv },
    ...(opts.tofuRunner ? { runner: opts.tofuRunner } : {}),
    // Teardown guard: runtime agent roles must be gone before `tofu destroy`. No outputs = nothing was ever launched.
    listRuntimeAgentRoles: async () =>
      out ? (await compute.listTagged()).filter((i) => i.role === 'agent_runner' && i.state !== 'terminated').map((i) => i.id) : [],
  });

  const prices = new AwsPriceBook({ region: opts.region, clock, fallback: new StaticPriceBook(), ...profile });
  const bench =
    out && haveGit && opts.benchImageRef
      ? new RemoteBenchRunner({
          compute,
          git: git as GitHubGitHost,
          clock,
          imageRef: opts.benchImageRef,
          artifacts,
          // Bench instances have no internet: inputs are staged as tarballs under bench-inputs/ in the artifacts bucket.
          stagingBucket: out.artifactsBucket,
        })
      : unready<RemoteBenchRunner>('bench runner', [!out ? needSandbox : '', !haveGit ? 'set the GitHub repository and token (Settings)' : '', !opts.benchImageRef ? 'set the bench AMI (Settings)' : ''].filter(Boolean));
  const runners = out
    ? new SsmRunnerControl({
        compute,
        store: artifacts,
        runnerEnv: { RATCHET_ARTIFACTS_BUCKET: out.artifactsBucket, AWS_REGION: opts.region },
      })
    : unready<SsmRunnerControl>('agent runners', [needSandbox]);

  const engine = new Engine({
    ledger: opts.ledger,
    mode: 'live',
    clock,
    ports: { sandbox, compute, prices, artifacts, git, runners, bench, costs: new AwsCostReconciler() },
    settings: {
      ...(opts.secrets ? { secrets: opts.secrets } : {}),
      ...(opts.runnerImageRef ? { runnerImageRef: opts.runnerImageRef } : {}),
      scratchDir: join(opts.stateRoot, 'scratch'),
      ...opts.settings,
    },
  });

  return {
    engine,
    sandbox,
    repoUrl: haveGit ? (git as GitHubGitHost).remoteUrl() : null,
    missing,
    async dispose() {
      await engine.close();
    },
  };
}
