import type { AgentKind, AgentsWhere, Ledger, NoiseFloorOptions, RatchetConfig, SandboxInput } from '@ratchet/core';

/** Where API keys come from (desktop: OS keychain). Values go to `putSecret` and nowhere else. */
export type SecretSource = (kind: AgentKind, name: string) => string | undefined | Promise<string | undefined>;

export type ReportBuilder = (ledger: Ledger, optimizationId: string) => string | Promise<string>;

/**
 * The invisible sandbox. Off by default (the sandbox is then applied and destroyed explicitly, as in the tests and for
 * advanced use); the live and hybrid runtimes turn it on.
 */
export interface SandboxSettings {
  /** Create the sandbox lazily at the first run start and destroy it after the idle window. Default false. */
  auto?: boolean;
  /** How long after the last run ends the sandbox is destroyed (default 30 min). */
  idleMs?: number;
  /** Region when ratchet.yaml names none (`environment.region`). Default `us-east-1`. */
  defaultRegion?: string;
  /** After a failed automatic destroy, wait this long before trying again (default 5 min). */
  retryMs?: number;
  /** Sandbox id for a region. Default `ratchet-sandbox` (`ratchet-sim` in simulation) in the default region, `<id>-<region>` elsewhere. */
  sandboxId?(region: string): string;
  /** Override the plan inputs (instance types, limits) derived from ratchet.yaml. */
  input?(ctx: { sandboxId: string; region: string; config: RatchetConfig }): SandboxInput | undefined;
}

export interface EngineSettings {
  sandbox?: SandboxSettings;
  /**
   * Where agents run when an optimization does not say. Default: `local` if the engine was given a `localRunners` port, else
   * `cloud`.
   */
  agentsWhere?: AgentsWhere;
  secrets?: SecretSource;
  /** Secret names pushed to each agent's runner. Defaults below. */
  secretNames?: Partial<Record<AgentKind, string[]>>;
  /** `run()` tick period = metering interval = max budget overshoot (default 15 s). */
  tickIntervalMs?: number;
  /** Where ratchet.yaml / RATCHET.md are checked out to read them (default: OS tmp dir). */
  scratchDir?: string;
  runnerImageRef?: string;
  /** Used when an `usage` event carries tokens but no `costUsd`. */
  tokenPricing?: { inputUsdPerMTok: number; outputUsdPerMTok: number };
  /** Fraction of the budget at which a warning is emitted (default 0.8). */
  warnFraction?: number;
  noise?: NoiseFloorOptions;
  /** Attempts per bench job before the candidate is rejected as an infrastructure failure (default 2). */
  maxBenchAttempts?: number;
  /** After a graceful stop, how long queued candidates may still be evaluated (default 30 min). */
  drainTimeoutMs?: number;
  /** Also bench the baseline against the hidden trace so `confirm` compares like with like (default true). */
  confirmBaseline?: boolean;
  /** Price used when the PriceBook fails and no earlier price is known (USD/h, default 10: conservative). */
  fallbackHourlyUsd?: number;
  reaper?: {
    maxExperimentLifetimeMs?: number; // default 2 h (IAM boundary default)
    maxBenchLifetimeMs?: number; // default 2 h
    runnerGraceMs?: number; // runner lifetime = time limit + grace (default 30 min)
  };
  reportBuilder?: ReportBuilder;
  /** How long after an optimization ends billing data is assumed incomplete before `reconcileCosts` writes corrections (default 24 h, Cost Explorer's lag). */
  costLagMs?: number;
  /** Called for non-fatal errors the engine swallowed to keep ticking. */
  onError?: (err: unknown, context: string) => void;
  /** Id source for optimizations (default `opt-<8 hex>`). Must produce [a-z0-9-] ids. */
  newId?: () => string;
}

export const DEFAULT_SECRET_NAMES: Record<AgentKind, string[]> = {
  'claude-code': ['ANTHROPIC_API_KEY', 'GITHUB_TOKEN'],
  codex: ['OPENAI_API_KEY', 'GITHUB_TOKEN'],
  mock: [],
};

export const DEFAULT_TOKEN_PRICING = { inputUsdPerMTok: 3, outputUsdPerMTok: 15 };

export interface ResolvedSandboxSettings {
  auto: boolean;
  idleMs: number;
  defaultRegion: string;
  retryMs: number;
  sandboxId: SandboxSettings['sandboxId'];
  input: SandboxSettings['input'];
}

export interface ResolvedSettings {
  sandbox: ResolvedSandboxSettings;
  agentsWhere: AgentsWhere | undefined;
  secrets: SecretSource;
  secretNames: Record<AgentKind, string[]>;
  tickIntervalMs: number;
  scratchDir: string | undefined;
  runnerImageRef: string;
  tokenPricing: { inputUsdPerMTok: number; outputUsdPerMTok: number };
  warnFraction: number;
  noise: NoiseFloorOptions;
  maxBenchAttempts: number;
  drainTimeoutMs: number;
  confirmBaseline: boolean;
  fallbackHourlyUsd: number;
  reaper: { maxExperimentLifetimeMs: number; maxBenchLifetimeMs: number; runnerGraceMs: number };
  reportBuilder: ReportBuilder | undefined;
  costLagMs: number;
  onError: (err: unknown, context: string) => void;
  newId: (() => string) | undefined;
}

const HOUR = 3_600_000;

export function resolveSettings(s: EngineSettings = {}): ResolvedSettings {
  return {
    sandbox: {
      auto: s.sandbox?.auto ?? false,
      idleMs: s.sandbox?.idleMs ?? 30 * 60_000,
      defaultRegion: s.sandbox?.defaultRegion ?? 'us-east-1',
      retryMs: s.sandbox?.retryMs ?? 5 * 60_000,
      sandboxId: s.sandbox?.sandboxId,
      input: s.sandbox?.input,
    },
    agentsWhere: s.agentsWhere,
    secrets: s.secrets ?? (() => undefined),
    secretNames: { ...DEFAULT_SECRET_NAMES, ...s.secretNames },
    tickIntervalMs: s.tickIntervalMs ?? 15_000,
    scratchDir: s.scratchDir,
    runnerImageRef: s.runnerImageRef ?? 'ratchet-runner',
    tokenPricing: s.tokenPricing ?? DEFAULT_TOKEN_PRICING,
    warnFraction: s.warnFraction ?? 0.8,
    noise: s.noise ?? {},
    maxBenchAttempts: s.maxBenchAttempts ?? 2,
    drainTimeoutMs: s.drainTimeoutMs ?? 30 * 60_000,
    confirmBaseline: s.confirmBaseline ?? true,
    fallbackHourlyUsd: s.fallbackHourlyUsd ?? 10,
    reaper: {
      maxExperimentLifetimeMs: s.reaper?.maxExperimentLifetimeMs ?? 2 * HOUR,
      maxBenchLifetimeMs: s.reaper?.maxBenchLifetimeMs ?? 2 * HOUR,
      runnerGraceMs: s.reaper?.runnerGraceMs ?? 30 * 60_000,
    },
    reportBuilder: s.reportBuilder,
    costLagMs: s.costLagMs ?? 24 * HOUR,
    onError: s.onError ?? (() => undefined),
    newId: s.newId,
  };
}
