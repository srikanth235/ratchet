import type {
  ArtifactStore,
  BenchRunner,
  CloudCompute,
  CostReconciler,
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
  runners: RunnerControl;
  bench: BenchRunner;
  /** Billing truth for the cost estimate (Cost Explorer). Optional: without it `reconcileCosts` reports it is unavailable. */
  costs?: CostReconciler;
}
