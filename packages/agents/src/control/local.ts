import { rm } from 'node:fs/promises';
import { join } from 'node:path';
import type { AgentAdapter, AgentEvent, Clock, RunnerConfig, RunnerControl, RunnerHandle, RunnerMessage } from '@ratchet/core';
import { systemClock } from '../clock.js';
import type { Git, GitRunner } from '../git.js';
import { MemoryInbox } from '../inbox.js';
import { runRunnerLoop, type RunnerLoopDeps, type RunnerLoopResult } from '../runner-loop.js';
import { addWorktree, ensureLocalClone, prepareWorkdir, removeWorktree } from '../workdir.js';

export interface LocalRunnerControlOptions {
  /** Each agent works under `<rootDir>/<optimizationId>/`: `<agentId>` (a clone, or a worktree of `clone/`). */
  rootDir: string;
  adapterFor(config: RunnerConfig, handle: RunnerHandle): AgentAdapter;
  clock?: Clock;
  git?: GitRunner;
  gitEnv?: Record<string, string>;
  /** Overrides for the loop (bestRef, maxIdleTurns, retryDelayMs, ...). */
  loop?: Partial<Omit<RunnerLoopDeps, 'config' | 'adapter' | 'workdir' | 'inbox' | 'emit' | 'clock' | 'signal'>>;
  /** Wait this long for a graceful stop before aborting the running turn. Default 10 s. */
  stopGraceMs?: number;
  /**
   * `clone` (default, simulation): a full clone per agent.
   * `worktree` (the app's "This machine" mode): one local clone per optimization under `<rootDir>/<optimizationId>/clone`, and a
   * `git worktree` of it per agent. The worktree is removed when the agent is stopped; the clone when the optimization is torn
   * down (`release`).
   */
  workdirs?: 'clone' | 'worktree';
  /** Extra environment for the agent processes of one agent (passed to the loop as `agentEnv`). */
  agentEnvFor?(config: RunnerConfig, handle: RunnerHandle): Record<string, string> | undefined;
}

interface LocalRunner {
  inbox: MemoryInbox;
  events: AgentEvent[];
  abort: AbortController;
  done: Promise<RunnerLoopResult>;
  workdir: string;
  stopped: boolean;
  /** Worktree mode: the clone this runner's worktree belongs to. */
  clone?: Git;
}

const keyOf = (h: RunnerHandle) => `${h.optimizationId}/${h.agentId}`;

/** Serializes async work (git writes lock the shared clone's metadata). */
class Queue {
  private tail: Promise<unknown> = Promise.resolve();
  run<T>(fn: () => Promise<T>): Promise<T> {
    const p = this.tail.then(fn, fn);
    this.tail = p.catch(() => undefined);
    return p;
  }
}

/**
 * `RunnerControl` that runs the runner loop in this process: in-memory inbox and event buffer, one working directory per agent.
 *
 * Used with `ScriptedAgent` in simulation and with the real Claude Code / Codex adapters for local agents (`workdirs:
 * 'worktree'`, the user's own CLI login). Agents do not survive the process: when the app restarts, `poll` reports each lost
 * agent as failed so the engine can end the run instead of waiting for events that will never come.
 */
export class LocalRunnerControl implements RunnerControl {
  private readonly runners = new Map<string, LocalRunner>();
  private readonly clones = new Map<string, Promise<Git>>();
  private readonly queues = new Map<string, Queue>();
  private readonly clock: Clock;
  private readonly mode: 'clone' | 'worktree';

  constructor(private readonly options: LocalRunnerControlOptions) {
    this.clock = options.clock ?? systemClock;
    this.mode = options.workdirs ?? 'clone';
  }

  private queue(optimizationId: string): Queue {
    let q = this.queues.get(optimizationId);
    if (!q) this.queues.set(optimizationId, (q = new Queue()));
    return q;
  }

  private gitOptions() {
    return { ...(this.options.git ? { git: this.options.git } : {}), ...(this.options.gitEnv ? { env: this.options.gitEnv } : {}) };
  }

  private cloneDir(optimizationId: string): string {
    return join(this.options.rootDir, optimizationId, 'clone');
  }

  private cloneFor(optimizationId: string, repoUrl: string): Promise<Git> {
    let clone = this.clones.get(optimizationId);
    if (!clone) {
      clone = ensureLocalClone({ repoUrl, dir: this.cloneDir(optimizationId), ...this.gitOptions() });
      this.clones.set(optimizationId, clone);
      clone.catch(() => this.clones.delete(optimizationId)); // a failed clone is retried by the next start
    }
    return clone;
  }

  async start(handle: RunnerHandle, config: RunnerConfig): Promise<void> {
    const key = keyOf(handle);
    if (this.runners.has(key)) throw new Error(`runner ${key} already started`);
    const workdir = join(this.options.rootDir, handle.optimizationId, handle.agentId);
    const events: AgentEvent[] = [];
    const emit = (e: AgentEvent) => void events.push(e);
    const abort = new AbortController();
    const inbox = new MemoryInbox();
    const gitOpts = this.gitOptions();

    let clone: Git | undefined;
    const refs: Pick<RunnerLoopDeps, 'acceptedRef' | 'bestTrackingRef'> = {};
    if (this.mode === 'worktree') {
      clone = await this.cloneFor(handle.optimizationId, config.repoUrl);
      await this.queue(handle.optimizationId).run(() =>
        addWorktree({ clone: clone!, dir: workdir, branch: config.branch, baseRef: config.baseRef, identity: { name: `${config.agentId} (ratchet)`, email: `${config.agentId}@ratchet.local` }, ...gitOpts }),
      );
      // Refs outside HEAD are shared by every worktree of the clone: give each agent its own.
      refs.acceptedRef = `refs/ratchet/${handle.agentId}/accepted`;
      refs.bestTrackingRef = `refs/remotes/origin/ratchet-best/${handle.agentId}`;
    } else {
      await prepareWorkdir({ repoUrl: config.repoUrl, branch: config.branch, baseRef: config.baseRef, dir: workdir, ...gitOpts });
    }

    const agentEnv = this.options.agentEnvFor?.(config, handle);
    const done = runRunnerLoop({
      ...this.options.loop,
      ...refs,
      ...(this.options.git ? { git: this.options.git } : {}),
      ...(this.options.gitEnv ? { gitEnv: this.options.gitEnv } : {}),
      ...(agentEnv ? { agentEnv } : {}),
      config,
      adapter: this.options.adapterFor(config, handle),
      workdir,
      inbox,
      emit,
      clock: this.clock,
      signal: abort.signal,
    });
    this.runners.set(key, { inbox, events, abort, done, workdir, stopped: false, ...(clone ? { clone } : {}) });
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
    const runner = this.runners.get(keyOf(handle));
    if (!runner) {
      // The process restarted since this agent was launched: its loop is gone. Say so once, so the engine can end it.
      if (cursor === 'lost') return { events: [], cursor };
      const lost: AgentEvent = {
        at: this.clock.now(),
        type: 'status',
        text: 'local agent lost: the app was restarted while it was running',
        data: { state: 'failed', reason: 'lost' },
      };
      return { events: [lost], cursor: 'lost' };
    }
    const { events } = runner;
    const from = cursor ? Number(cursor) : 0;
    return { events: events.slice(from), cursor: String(events.length) };
  }

  private async finish(runner: LocalRunner): Promise<void> {
    runner.inbox.push({ type: 'stop' });
    const timer = setTimeout(() => runner.abort.abort(), this.options.stopGraceMs ?? 10_000);
    try {
      await runner.done;
    } finally {
      clearTimeout(timer);
    }
  }

  /** Worktree mode: remove the agent's worktree once its loop has ended. The branch it pushed stays on the remote. */
  private async cleanup(handle: RunnerHandle, runner: LocalRunner): Promise<void> {
    if (!runner.clone || runner.stopped) return;
    runner.stopped = true;
    const { clone, workdir } = runner;
    await this.queue(handle.optimizationId).run(() => removeWorktree(clone, workdir));
  }

  async stop(handle: RunnerHandle): Promise<void> {
    const runner = this.get(handle);
    await this.finish(runner);
    await this.cleanup(handle, runner);
  }

  /** Resolves with the loop's result when the runner finishes by itself or after `stop`. */
  result(handle: RunnerHandle): Promise<RunnerLoopResult> {
    return this.get(handle).done;
  }

  workdirOf(handle: RunnerHandle): string {
    return this.get(handle).workdir;
  }

  async stopAll(): Promise<void> {
    await Promise.all(
      [...this.runners.entries()].map(async ([key, r]) => {
        await this.finish(r);
        const [optimizationId, agentId] = key.split('/') as [string, string];
        await this.cleanup({ optimizationId, agentId, instanceId: '' }, r);
      }),
    );
  }

  /**
   * Teardown of one optimization: stop its agents, remove their worktrees and delete the local clone. Idempotent; the commits
   * the agents pushed are on the remote and are not touched.
   */
  async release(optimizationId: string): Promise<void> {
    const mine = [...this.runners.entries()].filter(([k]) => k.startsWith(`${optimizationId}/`));
    await Promise.all(
      mine.map(async ([key, r]) => {
        await this.finish(r);
        await this.cleanup({ optimizationId, agentId: key.slice(optimizationId.length + 1), instanceId: '' }, r);
      }),
    );
    if (this.mode === 'worktree') {
      this.clones.delete(optimizationId);
      await rm(this.cloneDir(optimizationId), { recursive: true, force: true });
    }
    await rm(join(this.options.rootDir, optimizationId), { recursive: true, force: true }).catch(() => undefined);
  }
}
