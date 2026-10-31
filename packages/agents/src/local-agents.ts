/**
 * Local agents: the "This machine" mode. The real Claude Code / Codex adapters, driven by the same runner loop as in the cloud,
 * but using the CLIs that are installed and signed in on this machine (see `detect.ts`), each agent in its own git worktree.
 */
import type { AgentAdapter, Clock, RunnerConfig } from '@ratchet/core';
import { ClaudeCodeAdapter, type ClaudeCodeOptions } from './claude-code.js';
import { CodexAdapter, type CodexOptions } from './codex.js';
import { LocalRunnerControl, type LocalRunnerControlOptions } from './control/local.js';
import { localAgentEnv, type HarnessInfo, type HarnessKind } from './detect.js';
import type { Spawner } from './process.js';

export interface LocalAdapterOptions {
  /** Result of `detectHarnesses()`. */
  harnesses: HarnessInfo[];
  /** The login-shell PATH (`resolveLoginPath().path`), so the CLIs find node, git and the user's tools. */
  loginPath: string;
  /** Environment to derive the agents' from. Default `process.env`. Credentials are stripped, see `localAgentEnv`. */
  baseEnv?: Record<string, string | undefined>;
  spawner?: Spawner;
  clock?: Clock;
  claude?: Partial<ClaudeCodeOptions>;
  codex?: Partial<CodexOptions>;
  /** Builds the adapter for `kind: 'mock'` (a demo with scripted agents). Without it a mock agent is an error. */
  mock?: (config: RunnerConfig) => AgentAdapter;
}

/** Why an agent of `kind` cannot run on this machine, or null when it can. */
export function localAgentProblem(harnesses: HarnessInfo[], kind: HarnessKind): string | null {
  const info = harnesses.find((h) => h.kind === kind);
  const label = kind === 'claude-code' ? 'Claude Code' : 'Codex';
  if (!info) return `${label} was not probed on this machine`;
  if (!info.installed || !info.path) return info.hint;
  if (info.auth === 'signed-out') return info.hint;
  return null;
}

/**
 * `adapterFor` for `LocalRunnerControl`. Each adapter runs the binary found by detection with the user's login: no API key is
 * passed unless the harness reported `api-key` auth and the key is set. Throws (so the launch fails with the hint) when the
 * harness is missing or signed out.
 */
export function localAdapterFactory(opts: LocalAdapterOptions): (config: RunnerConfig) => AgentAdapter {
  const baseEnv = opts.baseEnv ?? process.env;
  return (config) => {
    if (config.kind === 'mock') {
      if (!opts.mock) throw new Error('mock agents are not available in this runtime');
      return opts.mock(config);
    }
    const problem = localAgentProblem(opts.harnesses, config.kind);
    if (problem) throw new Error(`cannot run ${config.kind} on this machine: ${problem}`);
    const info = opts.harnesses.find((h) => h.kind === config.kind)!;
    const env = localAgentEnv(info, baseEnv, opts.loginPath);
    const common = { binary: info.path!, env, allowSubscriptionAuth: true, ...(opts.spawner ? { spawner: opts.spawner } : {}), ...(opts.clock ? { clock: opts.clock } : {}) };
    return config.kind === 'claude-code' ? new ClaudeCodeAdapter({ ...common, ...opts.claude }) : new CodexAdapter({ ...common, ...opts.codex });
  };
}

/** The adapter of one agent kind (the user's own login), for callers that need an agent outside a runner loop, such as the environment setup agent. */
export function localAdapterFor(opts: LocalAdapterOptions, kind: HarnessKind): AgentAdapter {
  return localAdapterFactory(opts)({ kind } as RunnerConfig);
}

export interface LocalAgentControlOptions extends LocalAdapterOptions {
  /** `<rootDir>/<optimizationId>/{clone,<agentId>}`. */
  rootDir: string;
  /** Git credentials for the clone and the pushes (e.g. `gitAuthEnv(repoUrl, token)`). Never given to the agent processes. */
  gitEnv?: Record<string, string>;
  loop?: LocalRunnerControlOptions['loop'];
  stopGraceMs?: number;
}

/** A `LocalRunnerControl` that runs the real adapters, one git worktree per agent. */
export function createLocalAgentControl(opts: LocalAgentControlOptions): LocalRunnerControl {
  return new LocalRunnerControl({
    rootDir: opts.rootDir,
    workdirs: 'worktree',
    adapterFor: localAdapterFactory(opts),
    ...(opts.clock ? { clock: opts.clock } : {}),
    ...(opts.gitEnv ? { gitEnv: opts.gitEnv } : {}),
    ...(opts.loop ? { loop: opts.loop } : {}),
    ...(opts.stopGraceMs !== undefined ? { stopGraceMs: opts.stopGraceMs } : {}),
  });
}
