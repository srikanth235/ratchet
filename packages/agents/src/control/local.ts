import { join } from 'node:path';
import type { AgentAdapter, AgentEvent, Clock, RunnerConfig, RunnerControl, RunnerHandle, RunnerMessage } from '@ratchet/core';
import { systemClock } from '../clock.js';
import type { GitRunner } from '../git.js';
import { MemoryInbox } from '../inbox.js';
import { runRunnerLoop, type RunnerLoopDeps, type RunnerLoopResult } from '../runner-loop.js';
import { prepareWorkdir } from '../workdir.js';

export interface LocalRunnerControlOptions {
  /** Each agent gets its own clone under `<rootDir>/<optimizationId>/<agentId>`. */
  rootDir: string;
  adapterFor(config: RunnerConfig, handle: RunnerHandle): AgentAdapter;
  clock?: Clock;
  git?: GitRunner;
  gitEnv?: Record<string, string>;
  /** Overrides for the loop (bestRef, maxIdleTurns, retryDelayMs, ...). */
  loop?: Partial<Omit<RunnerLoopDeps, 'config' | 'adapter' | 'workdir' | 'inbox' | 'emit' | 'clock' | 'signal'>>;
  /** Wait this long for a graceful stop before aborting the running turn. Default 10 s. */
  stopGraceMs?: number;
}

interface LocalRunner {
  inbox: MemoryInbox;
  events: AgentEvent[];
  abort: AbortController;
  done: Promise<RunnerLoopResult>;
  workdir: string;
}

const keyOf = (h: RunnerHandle) => `${h.optimizationId}/${h.agentId}`;

/** Simulation `RunnerControl`: runs the runner loop in this process, one clone per agent, in-memory inbox and event buffer. */
export class LocalRunnerControl implements RunnerControl {
  private readonly runners = new Map<string, LocalRunner>();
  private readonly clock: Clock;

  constructor(private readonly options: LocalRunnerControlOptions) {
    this.clock = options.clock ?? systemClock;
  }

  async start(handle: RunnerHandle, config: RunnerConfig): Promise<void> {
    const key = keyOf(handle);
    if (this.runners.has(key)) throw new Error(`runner ${key} already started`);
    const workdir = join(this.options.rootDir, handle.optimizationId, handle.agentId);
    const events: AgentEvent[] = [];
    const emit = (e: AgentEvent) => void events.push(e);
    const abort = new AbortController();
    const inbox = new MemoryInbox();
    const gitOpts = { ...(this.options.git ? { git: this.options.git } : {}), ...(this.options.gitEnv ? { env: this.options.gitEnv } : {}) };
    await prepareWorkdir({ repoUrl: config.repoUrl, branch: config.branch, baseRef: config.baseRef, dir: workdir, ...gitOpts });
    const done = runRunnerLoop({
      ...this.options.loop,
      ...(this.options.git ? { git: this.options.git } : {}),
      ...(this.options.gitEnv ? { gitEnv: this.options.gitEnv } : {}),
      config,
      adapter: this.options.adapterFor(config, handle),
      workdir,
      inbox,
      emit,
      clock: this.clock,
      signal: abort.signal,
    });
    this.runners.set(key, { inbox, events, abort, done, workdir });
  }

  private get(handle: RunnerHandle): LocalRunner {
    const runner = this.runners.get(keyOf(handle));
    if (!runner) throw new Error(`unknown runner ${keyOf(handle)}`);
    return runner;
  }

  async send(handle: RunnerHandle, message: RunnerMessage): Promise<void> {
    this.get(handle).inbox.push(message);
  }

  async poll(handle: RunnerHandle, cursor?: string): Promise<{ events: AgentEvent[]; cursor: string }> {
    const { events } = this.get(handle);
    const from = cursor ? Number(cursor) : 0;
    return { events: events.slice(from), cursor: String(events.length) };
  }

  async stop(handle: RunnerHandle): Promise<void> {
    const runner = this.get(handle);
    runner.inbox.push({ type: 'stop' });
    const timer = setTimeout(() => runner.abort.abort(), this.options.stopGraceMs ?? 10_000);
    try {
      await runner.done;
    } finally {
      clearTimeout(timer);
    }
  }

  /** Resolves with the loop's result when the runner finishes by itself or after `stop`. */
  result(handle: RunnerHandle): Promise<RunnerLoopResult> {
    return this.get(handle).done;
  }

  workdirOf(handle: RunnerHandle): string {
    return this.get(handle).workdir;
  }

  async stopAll(): Promise<void> {
    await Promise.all([...this.runners.values()].map(async (r) => {
      r.inbox.push({ type: 'stop' });
      const timer = setTimeout(() => r.abort.abort(), this.options.stopGraceMs ?? 10_000);
      await r.done.finally(() => clearTimeout(timer));
    }));
  }
}
