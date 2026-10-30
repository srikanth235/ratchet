export * from './errors.js';
export { runProcess, type ProcessOptions, type ProcessResult } from './exec.js';
export { flattenNumeric, medianMetrics, resolveMetric } from './flatten.js';
export { BenchLog } from './log.js';
export { applyOverlay, restoreProtected, isProtected, listFiles, type OverlayResult } from './overlay.js';
export {
  DEFAULT_TIMEOUTS,
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
export { LocalBenchRunner, type LocalBenchRunnerOptions } from './local.js';
export { RemoteBenchRunner, type RemoteBenchRunnerOptions } from './remote.js';
export { protectedPcre } from './remote-scripts.js';
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
