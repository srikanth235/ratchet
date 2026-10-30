import type { AgentSession, AgentSpec, Clock } from '@ratchet/core';
import { nodeSpawner, runProcess, type RunProcessResult, type Spawner } from './process.js';
import { systemClock } from './clock.js';

/** The agent CLI failed (non-zero exit, error result, missing session). The turn did not complete. */
export class AgentError extends Error {
  constructor(message: string, readonly detail?: { exitCode?: number | null; stderr?: string }) {
    super(message);
    this.name = 'AgentError';
  }
}

/** The turn was cut short by the AbortSignal (the CLI's process group was killed). */
export class AgentAbortedError extends Error {
  constructor(readonly session?: AgentSession) {
    super('agent turn aborted');
    this.name = 'AgentAbortedError';
  }
}

export interface CliAgentOptions {
  /** Path or name of the CLI binary. */
  binary?: string;
  /** Base environment (default `process.env`); `AgentSpec.env` is layered on top. */
  env?: Record<string, string | undefined>;
  spawner?: Spawner;
  clock?: Clock;
  /** Extra CLI args appended before the prompt. */
  extraArgs?: string[];
  /** Grace between SIGTERM and SIGKILL on abort. */
  killGraceMs?: number;
}

export function truncate(text: string, max = 600): string {
  return text.length > max ? `${text.slice(0, max)}… [+${text.length - max} chars]` : text;
}

export function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

export function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0;
}

export function buildEnv(base: Record<string, string | undefined>, spec: Record<string, string | undefined> | undefined): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries({ ...base, ...spec })) if (v !== undefined) env[k] = v;
  return env;
}

/** Common plumbing for the CLI-backed adapters: spawn, feed lines to a parser, classify the outcome. */
export abstract class CliAgentBase {
  protected readonly binary: string;
  protected baseEnv: Record<string, string | undefined>;
  protected readonly spawner: Spawner;
  protected readonly clock: Clock;
  protected readonly extraArgs: string[];
  protected readonly killGraceMs: number | undefined;

  constructor(defaultBinary: string, options: CliAgentOptions) {
    this.binary = options.binary ?? defaultBinary;
    this.baseEnv = options.env ?? process.env;
    this.spawner = options.spawner ?? nodeSpawner;
    this.clock = options.clock ?? systemClock;
    this.extraArgs = options.extraArgs ?? [];
    this.killGraceMs = options.killGraceMs;
  }

  protected async run(
    args: string[],
    spec: AgentSpec,
    stdin: string | undefined,
    onLine: (line: string) => void,
    signal?: AbortSignal,
  ): Promise<RunProcessResult> {
    return runProcess(this.spawner, this.binary, args, {
      cwd: spec.workdir,
      env: buildEnv(this.baseEnv, spec.env),
      ...(stdin !== undefined ? { stdin } : {}),
      ...(signal ? { signal } : {}),
      ...(this.killGraceMs !== undefined ? { killGraceMs: this.killGraceMs } : {}),
      onStdoutLine: onLine,
    });
  }

  /** `<binary> --version` succeeds. */
  protected async probeBinary(): Promise<string | null> {
    let out = '';
    const res = await runProcess(this.spawner, this.binary, ['--version'], {
      cwd: process.cwd(),
      env: buildEnv(this.baseEnv, undefined),
      killGraceMs: 1000,
      onStdoutLine: (l) => (out += `${l}\n`),
    });
    if (res.signal?.startsWith('spawn-error')) return `cannot run '${this.binary}': ${res.signal.slice('spawn-error: '.length)}`;
    if (res.code !== 0) return `'${this.binary} --version' exited with ${res.code}: ${res.stderrTail || out.trim()}`;
    return null;
  }
}

