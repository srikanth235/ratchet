import type {
  ArtifactStore,
  BenchRunner,
  CloudCompute,
  GitHost,
  PriceBook,
  RunnerControl,
  SandboxProvider,
} from '@ratchet/core';

/** Everything the engine talks to, as the port interfaces from @ratchet/core. */
export interface EnginePorts {
  /** Held for the app's setup flow; the engine itself does not apply or destroy the sandbox. */
  sandbox: SandboxProvider;
  compute: CloudCompute;
  prices: PriceBook;
  artifacts: ArtifactStore;
  /** One host, or a factory when the app drives several repositories. */
  git: GitHost | ((repoUrl: string) => GitHost);
  runners: RunnerControl;
  bench: BenchRunner;
}
