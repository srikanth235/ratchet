import type { AgentId, BenchJob, BenchOutcome, BenchRunner, OptimizationId, RatchetConfig } from '@ratchet/core';
import { BenchCancelledError } from './errors.js';
import type { BenchProgress, ProgressBenchRunner } from './pipeline.js';

/**
 * Bench queue.
 *
 * - One job at a time per optimization (each optimization has its own pinned bench hardware, so
 *   different optimizations run in parallel).
 * - Within an optimization the next job is chosen round-robin over agents: each agent has its own
 *   FIFO; agents take turns in order of first arrival; an agent that still has work goes to the
 *   back of the rotation after being served. So an agent submitting 20 candidates delays another
 *   agent's single candidate by at most one job.
 * - Queued jobs are cancelled without ever running; a running job is aborted through its
 *   AbortSignal (the runner kills its process groups / terminates its instance).
 */

export interface BenchRequest {
  agentId: AgentId;
  job: BenchJob;
  config: RatchetConfig;
  baseline: { metrics: Record<string, number> } | null;
  /** Aborting this signal cancels the job (queued or running). */
  signal?: AbortSignal;
}

export interface BenchTicket {
  id: string;
  /** Resolves with the outcome; rejects with BenchCancelledError when cancelled, or the runner's error. */
  promise: Promise<BenchOutcome>;
  cancel(): void;
}

interface Ref {
  id: string;
  agentId: AgentId;
  job: BenchJob;
}

export type QueueEvent =
  | (Ref & { type: 'queued'; /** 0 = next to run */ position: number })
  | (Ref & { type: 'started' })
  | (Ref & { type: 'progress'; progress: BenchProgress })
  | (Ref & { type: 'done'; outcome: BenchOutcome })
  | (Ref & { type: 'failed'; error: string })
  | (Ref & { type: 'cancelled'; wasRunning: boolean });

/** Short UI label for an event: queued, checkout, setup, guardrails 1/2, bench 3/5, confirm 2/3, done, ... */
export function stateLabel(e: QueueEvent): string {
  switch (e.type) {
    case 'queued':
      return e.position === 0 ? 'queued (next)' : `queued (#${e.position + 1})`;
    case 'started':
      return 'starting';
    case 'progress': {
      const p = e.progress;
      switch (p.step) {
        case 'checkout':
        case 'setup':
        case 'evaluate':
          return p.step;
        case 'guardrails':
          return `guardrails ${p.index}/${p.total}`;
        case 'bench':
          return `${p.phase === 'confirm' ? 'confirm' : p.phase === 'baseline' ? 'baseline' : 'bench'} ${p.run}/${p.of}`;
      }
    }
    case 'done':
      return 'done';
    case 'failed':
      return 'failed';
    case 'cancelled':
      return 'cancelled';
  }
}

interface Entry extends Ref {
  req: BenchRequest;
  controller: AbortController;
  cancelled: boolean;
  settled: boolean;
  resolve(o: BenchOutcome): void;
  reject(e: unknown): void;
  detachExternal?: () => void;
}

interface Lane {
  optimizationId: OptimizationId;
  running?: Entry;
  perAgent: Map<AgentId, Entry[]>;
  rotation: AgentId[];
}

export interface BenchQueueOptions {
  onEvent?: (e: QueueEvent) => void;
  /** Id generator (tests). */
  newId?: () => string;
}

export class BenchQueue {
  private readonly lanes = new Map<OptimizationId, Lane>();
  private readonly listeners = new Set<(e: QueueEvent) => void>();
  private seq = 0;
  private closed = false;

  constructor(
    private readonly runner: BenchRunner,
    private readonly opts: BenchQueueOptions = {},
  ) {
    if (opts.onEvent) this.listeners.add(opts.onEvent);
  }

  subscribe(listener: (e: QueueEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  submit(req: BenchRequest): BenchTicket {
    if (this.closed) throw new Error('BenchQueue is closed');
    const id = this.opts.newId ? this.opts.newId() : `bench-${++this.seq}`;
    let resolve!: (o: BenchOutcome) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<BenchOutcome>((res, rej) => ((resolve = res), (reject = rej)));
    promise.catch(() => undefined); // callers that only watch events must not trigger unhandled rejections
    const entry: Entry = {
      id,
      agentId: req.agentId,
      job: req.job,
      req,
      controller: new AbortController(),
      cancelled: false,
      settled: false,
      resolve,
      reject,
    };
    if (req.signal) {
      const onAbort = () => this.cancelEntry(entry);
      if (req.signal.aborted) entry.cancelled = true;
      else {
        req.signal.addEventListener('abort', onAbort, { once: true });
        entry.detachExternal = () => req.signal?.removeEventListener('abort', onAbort);
      }
    }

    const lane = this.lane(req.job.optimizationId);
    let q = lane.perAgent.get(entry.agentId);
    if (!q) lane.perAgent.set(entry.agentId, (q = []));
    q.push(entry);
    if (!lane.rotation.includes(entry.agentId)) lane.rotation.push(entry.agentId);
    this.emit({ type: 'queued', id, agentId: entry.agentId, job: entry.job, position: this.order(lane).findIndex((e) => e === entry) });
    if (entry.cancelled) this.cancelEntry(entry);
    else this.pump(lane);
    return { id, promise, cancel: () => this.cancelEntry(entry) };
  }

  /** Cancel one job by id. Returns false when it is unknown or already finished. */
  cancel(id: string): boolean {
    for (const lane of this.lanes.values()) {
      const e = lane.running?.id === id ? lane.running : [...lane.perAgent.values()].flat().find((x) => x.id === id);
      if (e) return this.cancelEntry(e);
    }
    return false;
  }

  /** Cancel everything queued or running for an agent (optionally within one optimization). */
  cancelAgent(agentId: AgentId, optimizationId?: OptimizationId): number {
    return this.cancelWhere((e) => e.agentId === agentId && (optimizationId === undefined || e.job.optimizationId === optimizationId));
  }

  cancelOptimization(optimizationId: OptimizationId): number {
    return this.cancelWhere((e) => e.job.optimizationId === optimizationId);
  }

  /** Cancel everything and refuse new submissions. */
  close(): void {
    this.closed = true;
    this.cancelWhere(() => true);
  }

  /** What is running and what is waiting, in the order jobs will be served. */
  snapshot(): { optimizationId: OptimizationId; running?: Ref; queued: Array<Ref & { position: number }> }[] {
    return [...this.lanes.values()].map((lane) => ({
      optimizationId: lane.optimizationId,
      ...(lane.running ? { running: ref(lane.running) } : {}),
      queued: this.order(lane).map((e, position) => ({ ...ref(e), position })),
    }));
  }

  // ------------------------------------------------------------------ internals

  private lane(id: OptimizationId): Lane {
    let l = this.lanes.get(id);
    if (!l) this.lanes.set(id, (l = { optimizationId: id, perAgent: new Map(), rotation: [] }));
    return l;
  }

  /** The order in which queued entries will run (simulates the rotation without mutating it). */
  private order(lane: Lane): Entry[] {
    const rotation = [...lane.rotation];
    const queues = new Map([...lane.perAgent].map(([k, v]) => [k, [...v]]));
    const out: Entry[] = [];
    while (rotation.length > 0) {
      const agent = rotation.shift()!;
      const q = queues.get(agent);
      const next = q?.shift();
      if (!next) continue;
      out.push(next);
      if (q!.length > 0) rotation.push(agent);
    }
    return out;
  }

  private takeNext(lane: Lane): Entry | undefined {
    while (lane.rotation.length > 0) {
      const agent = lane.rotation.shift()!;
      const q = lane.perAgent.get(agent);
      const next = q?.shift();
      if (q && q.length > 0) lane.rotation.push(agent);
      else lane.perAgent.delete(agent);
      if (next) return next;
    }
    return undefined;
  }

  private pump(lane: Lane): void {
    if (lane.running) return;
    const entry = this.takeNext(lane);
    if (!entry) return;
    lane.running = entry;
    void this.execute(lane, entry);
  }

  private async execute(lane: Lane, entry: Entry): Promise<void> {
    this.emit({ type: 'started', ...ref(entry) });
    try {
      // our runners take an optional 5th progress callback; a plain BenchRunner just ignores it
      const outcome = await (this.runner as ProgressBenchRunner).run(
        entry.job,
        entry.req.config,
        entry.req.baseline,
        entry.controller.signal,
        (p) => this.emit({ type: 'progress', ...ref(entry), progress: p }),
      );
      if (entry.cancelled) this.settle(entry, 'cancelled', undefined, true);
      else {
        this.emit({ type: 'done', ...ref(entry), outcome });
        this.settle(entry, 'ok', outcome);
      }
    } catch (err) {
      if (entry.cancelled) this.settle(entry, 'cancelled', undefined, true);
      else {
        this.emit({ type: 'failed', ...ref(entry), error: err instanceof Error ? err.message : String(err) });
        this.settle(entry, 'error', err);
      }
    } finally {
      lane.running = undefined;
      this.pump(lane);
    }
  }

  private cancelWhere(pred: (e: Entry) => boolean): number {
    const targets: Entry[] = [];
    for (const lane of this.lanes.values()) {
      if (lane.running && pred(lane.running)) targets.push(lane.running);
      for (const q of lane.perAgent.values()) targets.push(...q.filter(pred));
    }
    for (const t of targets) this.cancelEntry(t);
    return targets.length;
  }

  private cancelEntry(entry: Entry): boolean {
    if (entry.settled || (entry.cancelled && !this.isQueued(entry))) return false;
    entry.cancelled = true;
    const lane = this.lane(entry.job.optimizationId);
    if (lane.running === entry) {
      entry.controller.abort(); // execute() settles once the runner stops
      return true;
    }
    const q = lane.perAgent.get(entry.agentId);
    if (q) {
      const i = q.indexOf(entry);
      if (i >= 0) q.splice(i, 1);
      if (q.length === 0) {
        lane.perAgent.delete(entry.agentId);
        lane.rotation = lane.rotation.filter((a) => a !== entry.agentId);
      }
    }
    this.settle(entry, 'cancelled', undefined, false);
    return true;
  }

  private isQueued(entry: Entry): boolean {
    const lane = this.lanes.get(entry.job.optimizationId);
    return !!lane?.perAgent.get(entry.agentId)?.includes(entry);
  }

  private settle(entry: Entry, kind: 'ok' | 'error' | 'cancelled', value?: unknown, wasRunning = false): void {
    if (entry.settled) return;
    entry.settled = true;
    entry.detachExternal?.();
    if (kind === 'ok') entry.resolve(value as BenchOutcome);
    else if (kind === 'error') entry.reject(value);
    else {
      this.emit({ type: 'cancelled', ...ref(entry), wasRunning });
      entry.reject(new BenchCancelledError());
    }
  }

  private emit(e: QueueEvent): void {
    for (const l of this.listeners) {
      try {
        l(e);
      } catch {
        // a broken UI listener must not break the queue
      }
    }
  }
}

function ref(e: Ref): Ref {
  return { id: e.id, agentId: e.agentId, job: e.job };
}
