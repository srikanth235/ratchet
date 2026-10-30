import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve, sep } from 'node:path';
import type { AgentAdapter, AgentEvent, AgentSession, AgentSpec, Clock } from '@ratchet/core';
import { systemClock } from './clock.js';
import { AgentAbortedError, AgentError } from './cli-agent.js';
import { execGit, type GitRunner } from './git.js';

/** Skip a step when a marker is already present (or absent) in a file: patches stay idempotent after a rebase onto the best. */
export interface StepCondition {
  file: string;
  contains: string;
  skipWhen: 'present' | 'absent';
}

export type ScriptStep =
  | { type: 'message'; text: string }
  | { type: 'tool'; name: string; input?: Record<string, unknown>; output?: string; ms?: number }
  | { type: 'usage'; inputTokens: number; outputTokens: number; costUsd?: number }
  /** `diff` is unified-diff text relative to the workdir root, applied with `git apply`. */
  | { type: 'patch'; name?: string; diff: string; skip?: StepCondition }
  /** Replace `find` (must occur exactly once) with `replace`. */
  | { type: 'edit'; path: string; find: string; replace: string }
  | { type: 'write'; path: string; content: string }
  | { type: 'delete'; path: string }
  /** Run git in the workdir, e.g. `reset --hard HEAD~1` to drop a rejected attempt. */
  | { type: 'git'; args: string[] }
  | { type: 'sleep'; ms: number }
  /** Crash the turn with an error (simulates a CLI failure). */
  | { type: 'fail'; message: string };

/** One agent turn: `start` plays attempt 0, each `resume` plays the next. */
export interface ScriptAttempt {
  name: string;
  steps: ScriptStep[];
}

export interface AgentScript {
  name: string;
  attempts: ScriptAttempt[];
}

export interface ScriptedAgentOptions {
  clock?: Clock;
  git?: GitRunner;
  /** Fired for every event; handy in tests. */
  onPlay?(attempt: ScriptAttempt, index: number): void;
}

/** Sonnet-class list prices, to give scripted usage a believable `costUsd`. */
export function priceUsage(inputTokens: number, outputTokens: number): number {
  return Math.round(((inputTokens * 3 + outputTokens * 15) / 1e6) * 10_000) / 10_000;
}

export class ScriptedAgent implements AgentAdapter {
  readonly kind = 'mock' as const;
  /** Messages received through `resume`, in order (verdict summaries, user messages). */
  readonly received: string[] = [];
  private readonly clock: Clock;
  private readonly git: GitRunner;
  private readonly sessionIds = new Set<string>();
  /** Next attempt to play. Shared by all sessions: a restarted agent keeps working through the same plan. */
  private cursor = 0;
  private sessions = 0;

  constructor(readonly script: AgentScript, private readonly options: ScriptedAgentOptions = {}) {
    this.clock = options.clock ?? systemClock;
    this.git = options.git ?? execGit;
  }

  async preflight(): Promise<{ ok: boolean; problems: string[] }> {
    return { ok: this.script.attempts.length > 0, problems: this.script.attempts.length ? [] : ['script has no attempts'] };
  }

  async start(spec: AgentSpec, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<AgentSession> {
    const session: AgentSession = { id: `mock-${this.script.name}-${++this.sessions}`, kind: 'mock' };
    this.sessionIds.add(session.id);
    this.emit(onEvent, { type: 'status', text: `scripted agent '${this.script.name}' started`, data: { sessionId: session.id } });
    await this.playNext(session, spec, onEvent, signal);
    return session;
  }

  async resume(
    session: AgentSession,
    spec: AgentSpec,
    message: string,
    onEvent: (e: AgentEvent) => void,
    signal?: AbortSignal,
  ): Promise<void> {
    if (!this.sessionIds.has(session.id)) throw new AgentError(`unknown scripted session ${session.id}`);
    this.received.push(message);
    await this.playNext(session, spec, onEvent, signal);
  }

  private emit(onEvent: (e: AgentEvent) => void, e: Omit<AgentEvent, 'at'>): void {
    onEvent({ at: this.clock.now(), ...e });
  }

  private async playNext(session: AgentSession, spec: AgentSpec, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<void> {
    const index = this.cursor++;
    const attempt = this.script.attempts[index];
    if (!attempt) {
      this.emit(onEvent, { type: 'message', text: 'I have run out of ideas worth trying; leaving the tree as it is.' });
      return;
    }
    this.options.onPlay?.(attempt, index);
    for (const step of attempt.steps) {
      if (signal?.aborted) {
        this.emit(onEvent, { type: 'status', text: 'scripted turn aborted' });
        throw new AgentAbortedError(session);
      }
      await this.playStep(step, spec, onEvent, signal);
    }
    if (signal?.aborted) throw new AgentAbortedError(session);
  }

  private inside(workdir: string, path: string): string {
    const full = resolve(workdir, path);
    if (full !== resolve(workdir) && !full.startsWith(resolve(workdir) + sep)) throw new AgentError(`path escapes the workdir: ${path}`);
    return full;
  }

  private async playStep(step: ScriptStep, spec: AgentSpec, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<void> {
    const tool = (name: string, text: string, isError = false, extra: Record<string, unknown> = {}) => {
      this.emit(onEvent, { type: 'tool', text: `${name}: ${text}`, data: { phase: 'call', name, ...extra } });
      this.emit(onEvent, { type: 'tool', text: isError ? 'failed' : 'ok', data: { phase: 'result', isError } });
    };
    switch (step.type) {
      case 'message':
        this.emit(onEvent, { type: 'message', text: step.text });
        return;
      case 'tool':
        this.emit(onEvent, { type: 'tool', text: `${step.name}: ${JSON.stringify(step.input ?? {}).slice(0, 200)}`, data: { phase: 'call', name: step.name, input: step.input } });
        if (step.ms) await this.clock.sleep(step.ms, signal);
        this.emit(onEvent, { type: 'tool', text: step.output ?? 'ok', data: { phase: 'result', isError: false } });
        return;
      case 'usage': {
        const costUsd = step.costUsd ?? priceUsage(step.inputTokens, step.outputTokens);
        this.emit(onEvent, { type: 'usage', usage: { inputTokens: step.inputTokens, outputTokens: step.outputTokens, costUsd } });
        return;
      }
      case 'sleep':
        await this.clock.sleep(step.ms, signal);
        return;
      case 'write': {
        const full = this.inside(spec.workdir, step.path);
        await mkdir(dirname(full), { recursive: true });
        await writeFile(full, step.content);
        tool('Write', step.path);
        return;
      }
      case 'delete':
        await rm(this.inside(spec.workdir, step.path), { force: true });
        tool('Delete', step.path);
        return;
      case 'edit': {
        const full = this.inside(spec.workdir, step.path);
        const text = await readFile(full, 'utf8').catch(() => null);
        const count = text === null ? 0 : text.split(step.find).length - 1;
        if (text === null || count !== 1) {
          tool('Edit', step.path, true);
          this.emit(onEvent, { type: 'error', text: `edit ${step.path}: expected exactly one match, found ${count}`, data: { fatal: false } });
          return;
        }
        await writeFile(full, text.replace(step.find, () => step.replace));
        tool('Edit', step.path);
        return;
      }
      case 'git': {
        const res = await this.git(step.args, { cwd: spec.workdir });
        tool('Bash', `git ${step.args.join(' ')}`, res.code !== 0);
        if (res.code !== 0) this.emit(onEvent, { type: 'error', text: res.stderr.trim(), data: { fatal: false } });
        return;
      }
      case 'patch': {
        if (step.skip) {
          const text = await readFile(this.inside(spec.workdir, step.skip.file), 'utf8').catch(() => '');
          if (text.includes(step.skip.contains) === (step.skip.skipWhen === 'present')) {
            this.emit(onEvent, { type: 'message', text: `${step.name ?? 'patch'} is already in the tree; skipping it.` });
            return;
          }
        }
        const dir = await mkdtemp(join(tmpdir(), 'ratchet-patch-'));
        try {
          const file = join(dir, 'change.patch');
          await writeFile(file, step.diff);
          const res = await this.git(['apply', '--whitespace=nowarn', file], { cwd: spec.workdir });
          tool('Edit', step.name ?? 'patch', res.code !== 0);
          if (res.code !== 0) {
            this.emit(onEvent, { type: 'error', text: `patch ${step.name ?? ''} does not apply: ${res.stderr.trim()}`, data: { fatal: false } });
          }
        } finally {
          await rm(dir, { recursive: true, force: true });
        }
        return;
      }
      case 'fail':
        this.emit(onEvent, { type: 'error', text: step.message, data: { fatal: true } });
        throw new AgentError(step.message);
    }
  }
}
