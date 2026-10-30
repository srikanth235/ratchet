export * from './errors.js';
export { runProcess, type ProcessOptions, type ProcessResult } from './exec.js';
export { flattenNumeric, medianMetrics, resolveMetric } from './flatten.js';
export { BenchLog } from './log.js';
export { applyOverlay, restoreProtected, isProtected, listFiles, type OverlayResult } from './overlay.js';
export {
  DEFAULT_TIMEOUTS,
  type CheckoutVeto,
  commandGuardrailResult,
  evaluateThresholds,
  logKey,
  parseBenchOutput,
  type BenchProgress,
  type PipelineExecutor,
  type ProgressBenchRunner,
  type StepOutcome,
  type StepSpec,
  type StepTimeouts,
} from './pipeline.js';
export { currentNodeRuntime, ensureNodeShim, nodeDirFor, nodeShimScript, resolveNodeOnPath, withNodeOnPath, type NodeRuntimeInfo, type WithNodeOnPathOptions } from './node-path.js';
export { LocalBenchRunner, type LocalBenchRunnerOptions } from './local.js';
export { RemoteBenchRunner, type RemoteBenchRunnerOptions } from './remote.js';
export { protectedPcre, BENCH_USER, CANDIDATE_LAUNCHER, HOLDOUT_USER, type DepsRestore, type Isolation, type StagedArchive } from './remote-scripts.js';
export { stageDepsBase, stageSource, STAGING_PREFIX, type StagedSource, type StageInput } from './stage.js';
export {
  DEPS_BASE_ARCHIVE,
  DEPS_PREFIX,
  DEPS_STRATEGIES,
  DEPS_USER,
  NO_DEPS,
  NPM,
  PNPM,
  archFromInstanceType,
  dependencyChangeGuardrail,
  depsKey,
  hashNamedFiles,
  lockfileChanges,
  manifestKey,
  offlineSetupHint,
  parseDepsManifest,
  resolveDepsStrategy,
  type DepsKeyInput,
  type DepsManifest,
  type DepsPlan,
  type DepsStrategy,
  type DepsStrategyId,
  type ResolvedDeps,
} from './deps.js';
export { depsBuildScript, type DepsBuildInput } from './deps-scripts.js';
export {
  containerSetupHint,
  environmentChangeGuardrail,
  environmentChanges,
  environmentFilesOf,
  imageKey,
  imageManifestKey,
  imageTag,
  IMAGE_PREFIX,
  normalizeRepoRelative,
  parseDockerfile,
  parseImageManifest,
  parseJsonc,
  readEnvironment,
  resolveImageSource,
  type EnvironmentSettings,
  type ImageKeyInput,
  type ImageManifest,
  type ImagePlan,
  type ImageSpec,
  type ResolvedImage,
} from './image.js';
export {
  CANDIDATE_HELPER,
  containerRun,
  imageBuildScript,
  imageRestoreFragment,
  type ContainerRunSpec,
  type ContainerRuntime,
  type ImageBuildInput,
  type ImageRestore,
} from './image-scripts.js';
export { DepsSnapshots, type DepsSnapshotOptions, type PreparedEnvironment } from './remote-deps.js';
export { BenchQueue, stateLabel, type BenchQueueOptions, type BenchRequest, type BenchTicket, type QueueEvent } from './queue.js';
export {
  benchResultRecord,
  benchStats,
  evaluateCandidate,
  metricsFromStats,
  type BenchResultRecord,
  type CandidateEvaluation,
  type EvaluateCandidateInput,
} from './evaluate.js';
