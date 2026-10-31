import type {
  ArtifactStore,
  BenchRunner,
  CloudCompute,
  CostReconciler,
  EnvironmentDrafter,
  GitHost,
  PriceBook,
  RunnerControl,
  SandboxProvider,
} from '@ratchet/core';

/** Everything the engine talks to, as the port interfaces from @ratchet/core. */
export interface EnginePorts {
  /** Plan / apply / destroy go through the engine (planSandbox, applySandbox, destroySandbox) so the ledger records them. */
  sandbox: SandboxProvider;
  compute: CloudCompute;
  prices: PriceBook;
  artifacts: ArtifactStore;
  /** One host, or a factory when the app drives several repositories. */
  git: GitHost | ((repoUrl: string) => GitHost);
  /** Cloud agent runners (EC2 + SSM). */
  runners: RunnerControl;
  /** Agents on the user's machine (their own CLI login, a git worktree each). Optional; see `EngineSettings.agentsWhere`. */
  localRunners?: RunnerControl;
  bench: BenchRunner;
  /** Billing truth for the cost estimate (Cost Explorer). Optional: without it `reconcileCosts` reports it is unavailable. */
  costs?: CostReconciler;
  /**
   * Drafts, revises and approves environments (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"). Optional: without it the
   * engine's `draftEnvironment` & co. report `unavailable`. Applying an approved environment needs none of this: the engine
   * reads it from the protected ref at run start.
   */
  environment?: EnvironmentDrafter;
}
