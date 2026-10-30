import type { BenchJob, BenchOutcome, BenchRunner, RatchetConfig } from '@ratchet/core';
import { describe, expect, it } from 'vitest';
import { BenchCancelledError } from './errors.js';
import type { BenchProgress } from './pipeline.js';
import { BenchQueue, stateLabel, type QueueEvent } from './queue.js';
import { makeConfig } from './testkit/repo.js';

const config = makeConfig();
const outcome = (v = 1): BenchOutcome => ({ runs: [], samples: [v], guardrails: [], startedAt: 0, finishedAt: 1 });

interface Call {
  job: BenchJob;
  signal: AbortSignal | undefined;
  progress: ((p: BenchProgress) => void) | undefined;
  resolve(o?: BenchOutcome): void;
  reject(e: unknown): void;
}

/** A runner whose jobs finish when the test says so. Aborting rejects the job like a real runner. */
class ControlledRunner implements BenchRunner {
  calls: Call[] = [];
  run(job: BenchJob, _c: RatchetConfig, _b: unknown, signal?: AbortSignal, progress?: (p: BenchProgress) => void): Promise<BenchOutcome> {
    return new Promise((resolve, reject) => {
      const call: Call = { job, signal, progress, resolve: (o) => resolve(o ?? outcome()), reject };
      signal?.addEventListener('abort', () => reject(Object.assign(new Error('aborted'), { name: 'AbortError' })), { once: true });
      this.calls.push(call);
    });
  }
  get started(): string[] {
    return this.calls.map((c) => c.job.candidateId ?? c.job.sha);
  }
}

const j = (id: string, optimizationId = 'opt1', phase: BenchJob['phase'] = 'bench'): BenchJob => ({ optimizationId, candidateId: id, sha: id, phase, repeats: 2 });
const tick = () => new Promise((r) => setTimeout(r, 5));

function setup() {
  const runner = new ControlledRunner();
  const events: QueueEvent[] = [];
  const queue = new BenchQueue(runner, { onEvent: (e) => events.push(e) });
  const submit = (agentId: string, id: string, opt = 'opt1') => queue.submit({ agentId, job: j(id, opt), config, baseline: null });
  return { runner, events, queue, submit };
}

describe('BenchQueue', () => {
  it('runs one job at a time per optimization, FIFO', async () => {
    const { runner, submit } = setup();
    const t1 = submit('A', 'a1');
    const t2 = submit('A', 'a2');
    submit('A', 'a3');
    await tick();
    expect(runner.started).toEqual(['a1']);
    runner.calls[0]!.resolve(outcome(11));
    await expect(t1.promise).resolves.toMatchObject({ samples: [11] });
    await tick();
    expect(runner.started).toEqual(['a1', 'a2']);
    runner.calls[1]!.resolve();
    await t2.promise;
    await tick();
    expect(runner.started).toEqual(['a1', 'a2', 'a3']);
  });

  it('runs different optimizations in parallel', async () => {
    const { runner, submit } = setup();
    submit('A', 'x1', 'optX');
    submit('B', 'y1', 'optY');
    submit('A', 'x2', 'optX');
    await tick();
    expect(runner.started).toEqual(['x1', 'y1']);
  });

  it('is fair across agents: round-robin, so one agent cannot starve another', async () => {
    const { runner, submit } = setup();
    submit('A', 'a1');
    for (const id of ['a2', 'a3', 'a4', 'a5']) submit('A', id);
    submit('B', 'b1');
    submit('C', 'c1');
    submit('C', 'c2');
    const order: string[] = [];
    // drain: finish whatever runs, one at a time
    for (let i = 0; i < 8; i++) {
      await tick();
      const call = runner.calls[i]!;
      order.push(call.job.candidateId!);
      call.resolve();
    }
    // A's backlog does not delay B and C by more than one job each
    expect(order).toEqual(['a1', 'a2', 'b1', 'c1', 'a3', 'c2', 'a4', 'a5']);
  });

  it('serves a late agent after at most one more job of the busy agent', async () => {
    const { runner, submit } = setup();
    for (const id of ['a1', 'a2', 'a3', 'a4']) submit('A', id);
    await tick();
    submit('B', 'b1');
    runner.calls[0]!.resolve();
    await tick();
    expect(runner.started).toEqual(['a1', 'a2']);
    runner.calls[1]!.resolve();
    await tick();
    expect(runner.started).toEqual(['a1', 'a2', 'b1']);
  });

  it('cancels a queued job without running it', async () => {
    const { runner, events, submit } = setup();
    submit('A', 'a1');
    const t2 = submit('A', 'a2');
    const t3 = submit('B', 'b1');
    await tick();
    t2.cancel();
    await expect(t2.promise).rejects.toBeInstanceOf(BenchCancelledError);
    runner.calls[0]!.resolve();
    await tick();
    expect(runner.started).toEqual(['a1', 'b1']);
    expect(events.filter((e) => e.type === 'cancelled')).toMatchObject([{ id: t2.id, wasRunning: false }]);
    runner.calls[1]!.resolve();
    await t3.promise;
  });

  it('aborts a running job and moves on to the next', async () => {
    const { runner, events, submit } = setup();
    const t1 = submit('A', 'a1');
    submit('B', 'b1');
    await tick();
    expect(runner.calls[0]!.signal?.aborted).toBe(false);
    t1.cancel();
    expect(runner.calls[0]!.signal?.aborted).toBe(true);
    await expect(t1.promise).rejects.toBeInstanceOf(BenchCancelledError);
    await tick();
    expect(runner.started).toEqual(['a1', 'b1']);
    expect(events.find((e) => e.type === 'cancelled')).toMatchObject({ wasRunning: true });
    expect(events.some((e) => e.type === 'failed')).toBe(false);
  });

  it('cancelAgent, cancelOptimization and close', async () => {
    const { runner, queue, submit } = setup();
    const a1 = submit('A', 'a1');
    const a2 = submit('A', 'a2');
    const b1 = submit('B', 'b1');
    const x1 = submit('A', 'x1', 'optX');
    await tick();
    expect(queue.cancelAgent('A', 'opt1')).toBe(2); // running a1 + queued a2
    await expect(a1.promise).rejects.toBeInstanceOf(BenchCancelledError);
    await expect(a2.promise).rejects.toBeInstanceOf(BenchCancelledError);
    await tick();
    expect(runner.started).toEqual(['a1', 'x1', 'b1']);
    expect(queue.cancelOptimization('optX')).toBe(1);
    await expect(x1.promise).rejects.toBeInstanceOf(BenchCancelledError);
    queue.close();
    await expect(b1.promise).rejects.toBeInstanceOf(BenchCancelledError);
    expect(() => submit('A', 'late')).toThrow(/closed/);
    expect(queue.cancel('nope')).toBe(false);
  });

  it('honours an external AbortSignal, even one already aborted', async () => {
    const { runner, submit, queue } = setup();
    const ac = new AbortController();
    submit('A', 'a1');
    const t2 = queue.submit({ agentId: 'A', job: j('a2'), config, baseline: null, signal: ac.signal });
    ac.abort();
    await expect(t2.promise).rejects.toBeInstanceOf(BenchCancelledError);
    const pre = new AbortController();
    pre.abort();
    const t3 = queue.submit({ agentId: 'A', job: j('a3'), config, baseline: null, signal: pre.signal });
    await expect(t3.promise).rejects.toBeInstanceOf(BenchCancelledError);
    runner.calls[0]!.resolve();
    await tick();
    expect(runner.started).toEqual(['a1']);
  });

  it('emits queued / started / progress / done events with UI labels', async () => {
    const { runner, events, submit } = setup();
    submit('A', 'a1');
    await tick();
    const p = runner.calls[0]!.progress!;
    p({ step: 'deps' });
    p({ step: 'checkout' });
    p({ step: 'guardrails', index: 1, total: 2, name: 'pnpm test' });
    p({ step: 'bench', run: 2, of: 5, phase: 'bench' });
    p({ step: 'bench', run: 1, of: 3, phase: 'confirm' });
    runner.calls[0]!.resolve();
    await tick();
    expect(events.map(stateLabel)).toEqual(['queued (next)', 'starting', 'dependencies', 'checkout', 'guardrails 1/2', 'bench 2/5', 'confirm 1/3', 'done']);
  });

  it('reports a failing runner as a failed event and keeps going', async () => {
    const { runner, events, submit } = setup();
    const t1 = submit('A', 'a1');
    const t2 = submit('A', 'a2');
    await tick();
    runner.calls[0]!.reject(new Error('instance exploded'));
    await expect(t1.promise).rejects.toThrow('instance exploded');
    await tick();
    expect(events.find((e) => e.type === 'failed')).toMatchObject({ error: 'instance exploded' });
    expect(runner.started).toEqual(['a1', 'a2']);
    runner.calls[1]!.resolve();
    await t2.promise;
  });

  it('snapshot lists the running job and the waiting ones in service order', async () => {
    const { queue, submit } = setup();
    submit('A', 'a1');
    submit('A', 'a2');
    submit('A', 'a3');
    submit('B', 'b1');
    await tick();
    const [lane] = queue.snapshot();
    expect(lane!.running?.job.candidateId).toBe('a1');
    expect(lane!.queued.map((q) => [q.job.candidateId, q.position])).toEqual([['a2', 0], ['b1', 1], ['a3', 2]]);
  });

  it('survives a throwing listener', async () => {
    const runner = new ControlledRunner();
    const queue = new BenchQueue(runner, { onEvent: () => { throw new Error('ui bug'); } });
    const t = queue.submit({ agentId: 'A', job: j('a1'), config, baseline: null });
    await tick();
    runner.calls[0]!.resolve();
    await expect(t.promise).resolves.toBeDefined();
  });
});
