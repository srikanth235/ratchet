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
import type { Clock, Ledger, SandboxOutputs } from '@ratchet/core';
import { Engine, type EngineSettings } from '@ratchet/engine';
import { AwsCloudCompute, AwsPriceBook, S3ArtifactStore, StaticPriceBook, TofuSandboxProvider } from '@ratchet/aws';
import { GitHubGitHost } from '@ratchet/github';
import { RemoteBenchRunner } from '@ratchet/bench';
import { SsmRunnerControl } from '@ratchet/agents';
import { systemClock } from './clock.js';

export interface LiveOptions {
  ledger: Ledger;
  region: string;
  /** Outputs of the applied sandbox (from TofuSandboxProvider.outputs). */
  sandbox: SandboxOutputs;
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
  const compute = new AwsCloudCompute(
    {
      sandboxId: opts.sandboxId,
      region: opts.region,
      agentBoundaryArn: opts.sandbox.agentBoundaryRef,
      agentRolePath: opts.sandbox.agentRolePath,
      subnetId: opts.sandbox.network.subnetId,
      agentRunnerSecurityGroupId: opts.sandbox.network.agentRunnerSecurityGroupId,
      benchSecurityGroupId: opts.sandbox.network.benchSecurityGroupId,
      benchInstanceProfile: opts.sandbox.evalIdentityRef,
      artifactsBucket: opts.sandbox.artifactsBucket,
    },
    { clock },
  );
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
  const bench = new RemoteBenchRunner({ compute, git, clock, imageRef: opts.benchImageRef, artifacts });
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
