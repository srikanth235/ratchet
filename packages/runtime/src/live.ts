/**
 * Live runtime: the real Engine wired to real ports.
 * NOT exercised yet — see docs/REAL_ENV_TESTING.md for the test order.
 *
 * - Sandbox: TofuSandboxProvider (bundled `tofu`, infra/aws)
 * - AWS: AwsCloudCompute (EC2, IAM, SSM), AwsPriceBook, S3ArtifactStore
 * - Git: GitHubGitHost (repo-scoped token)
 * - Bench: RemoteBenchRunner (pinned EC2 bench instance via SSM)
 * - Agents: SsmRunnerControl (headless Claude Code / Codex on agent runners)
 */
import { join } from 'node:path';
import type { Clock, Ledger } from '@ratchet/core';
import { Engine, type EngineSettings } from '@ratchet/engine';
import { AwsCloudCompute, AwsPriceBook, S3ArtifactStore, StaticPriceBook, TofuSandboxProvider, awsComputeConfigFromOutputs, type AwsSandboxOutputs } from '@ratchet/aws';
import { GitHubGitHost } from '@ratchet/github';
import { RemoteBenchRunner } from '@ratchet/bench';
import { SsmRunnerControl } from '@ratchet/agents';
import { systemClock } from './clock.js';

export interface LiveOptions {
  ledger: Ledger;
  region: string;
  /** Outputs of the applied sandbox (from TofuSandboxProvider.outputs): bench subnet, split bench/confirm profiles, proxy. */
  sandbox: AwsSandboxOutputs;
  sandboxId: string;
  /** `owner/name` of the repository to optimize. */
  githubRepo: string;
  githubToken: string;
  /** AMIs built for agent runners and bench instances. */
  runnerImageRef: string;
  benchImageRef: string;
  /** Pinned `tofu` binary and the infra/aws module directory. */
  tofuBinary: string;
  moduleDir: string;
  stateRoot: string;
  /** Model API keys etc., looked up by agent kind + secret name. */
  secrets: EngineSettings['secrets'];
  /** AWS credentials for the app itself (profile / SSO), passed to tofu. */
  awsEnv?: Record<string, string>;
  clock?: Clock;
  settings?: EngineSettings;
}

export interface LiveRuntime {
  engine: Engine;
  sandbox: TofuSandboxProvider;
  repoUrl: string;
  dispose(): Promise<void>;
}

export function createLiveRuntime(opts: LiveOptions): LiveRuntime {
  const clock = opts.clock ?? systemClock;
  // Derived from outputs so the private bench subnet, the confirm-only profile and the egress proxy are never dropped.
  const compute = new AwsCloudCompute({ ...awsComputeConfigFromOutputs(opts.sandbox, opts.sandboxId), region: opts.region }, { clock });
  const sandbox = new TofuSandboxProvider({
    tofuBinary: opts.tofuBinary,
    moduleDir: opts.moduleDir,
    stateRoot: opts.stateRoot,
    ...(opts.awsEnv ? { env: opts.awsEnv } : {}),
    // Teardown guard: runtime agent roles must be gone before `tofu destroy`.
    listRuntimeAgentRoles: async () => (await compute.listTagged()).filter((i) => i.role === 'agent_runner' && i.state !== 'terminated').map((i) => i.id),
  });
  const artifacts = new S3ArtifactStore({ bucket: opts.sandbox.artifactsBucket, region: opts.region });
  const git = new GitHubGitHost({ repo: opts.githubRepo, token: opts.githubToken, cacheDir: join(opts.stateRoot, 'git-cache') });
  const prices = new AwsPriceBook({ region: opts.region, clock, fallback: new StaticPriceBook() });
  const bench = new RemoteBenchRunner({
    compute,
    git,
    clock,
    imageRef: opts.benchImageRef,
    artifacts,
    // Bench instances have no internet: inputs are staged as tarballs under bench-inputs/ in the artifacts bucket.
    stagingBucket: opts.sandbox.artifactsBucket,
  });
  const runners = new SsmRunnerControl({
    compute,
    store: artifacts,
    runnerEnv: { RATCHET_ARTIFACTS_BUCKET: opts.sandbox.artifactsBucket, AWS_REGION: opts.region },
  });

  const engine = new Engine({
    ledger: opts.ledger,
    mode: 'live',
    clock,
    ports: { sandbox, compute, prices, artifacts, git, runners, bench },
    settings: {
      ...(opts.secrets ? { secrets: opts.secrets } : {}),
      runnerImageRef: opts.runnerImageRef,
      scratchDir: join(opts.stateRoot, 'scratch'),
      ...opts.settings,
    },
  });

  return {
    engine,
    sandbox,
    repoUrl: git.remoteUrl(),
    async dispose() {
      engine.close();
    },
  };
}
