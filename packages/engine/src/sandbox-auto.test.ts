import { describe, expect, it } from 'vitest';
import type { AgentKind, EngineEvent, RatchetConfig } from '@ratchet/core';
import { regionOf } from './sandbox-auto.js';
import { makeHarness, TICK_MS, type Harness, type HarnessOptions } from './testing/harness.js';

const MIN = 60_000;
const SBX = 'ratchet-sandbox'; // live mode, default region

const live = (over: HarnessOptions = {}): Harness =>
  makeHarness({ mode: 'live', ...over, settings: { sandbox: { auto: true }, ...over.settings } });

/** Create + start another optimization (the harness's own `begin` always uses the id `opt-test`). */
async function run(h: Harness, id: string, agents: AgentKind[] = ['codex'], where?: 'local' | 'cloud'): Promise<string> {
  h.bench.set('main-sha', 'baseline', { samples: [100, 102, 98], metrics: { peak_rss: 500 } });
  h.bench.set('main-sha', 'confirm', { samples: [100, 100, 100] });
  await h.engine.createOptimization({ id, repoUrl: h.git.remoteUrl(), baseRef: 'main', agents, budgetUsd: 100, timeLimitSeconds: 8 * 3600, plateauEvals: 50, ...(where ? { agentsWhere: where } : {}) });
  await h.engine.start(id);
  return id;
}

const sandboxEvents = (h: Harness): Array<{ phase: string; sandboxId: string }> =>
  (h.events as EngineEvent[]).flatMap((e) => (e.type === 'sandbox' ? [{ phase: e.phase, sandboxId: e.sandboxId }] : []));

const applies = (h: Harness): string[] => h.log.filter((l) => l.startsWith('sandbox.apply'));
const destroys = (h: Harness): string[] => h.log.filter((l) => l.startsWith('sandbox.destroy'));

/** Advance the clock and tick (the engine's `run()` loop does this every tickIntervalMs). */
async function pass(h: Harness, ms: number): Promise<void> {
  h.clock.advance(ms);
  await h.engine.tick();
  await h.engine.settle();
}

describe('regionOf', () => {
  const cfg = (environment?: unknown) => ({ ...(environment === undefined ? {} : { environment }) }) as unknown as RatchetConfig;
  it('reads environment.region defensively and falls back to the setting default', () => {
    expect(regionOf(cfg({ region: 'eu-west-1' }), 'us-east-1')).toBe('eu-west-1');
    expect(regionOf(cfg({ region: '  ' }), 'us-east-1')).toBe('us-east-1');
    expect(regionOf(cfg({}), 'us-east-1')).toBe('us-east-1');
    expect(regionOf(cfg(), 'us-east-2')).toBe('us-east-2');
    expect(regionOf(cfg({ region: 42 }), 'us-east-1')).toBe('us-east-1');
  });
});

describe('automatic sandbox: creation', () => {
  it('is created at the first run start, not before, from ratchet.yaml and the default region', async () => {
    const h = live();
    const id = await h.engine.createOptimization({ repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 3 });
    expect(applies(h)).toEqual([]); // a draft needs no environment
    expect(h.ledger.getOptimization(id)!.sandboxId).toBeNull();

    h.bench.set('main-sha', 'baseline', { samples: [100, 100, 100] });
    h.bench.set('main-sha', 'confirm', { samples: [100, 100, 100] });
    let seen: { region: string; types: string[] } | undefined;
    h.sandbox.onApply = (i) => (seen = { region: i.region, types: i.allowedInstanceTypes });
    await h.engine.start(id);
    expect(applies(h)).toEqual([`sandbox.apply:${SBX}`]);
    expect(seen).toEqual({ region: 'us-east-1', types: ['c7i.xlarge', 'c7i.2xlarge'] }); // agents' type + the evaluator's hardware
    expect(h.ledger.getOptimization(id)).toMatchObject({ sandboxId: SBX, status: 'running' });
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'applied', region: 'us-east-1' });
    expect(sandboxEvents(h).map((e) => e.phase)).toEqual(['creating', 'ready']);
  });

  it('uses the configured default region and id scheme; other regions get their own sandbox', async () => {
    const h = live({ settings: { sandbox: { auto: true, defaultRegion: 'eu-west-1' } } });
    await run(h, 'run-a');
    expect(h.ledger.getOptimization('run-a')!.sandboxId).toBe(SBX);
    expect(h.ledger.getSandbox(SBX)!.region).toBe('eu-west-1');

    const named = live({ settings: { sandbox: { auto: true, sandboxId: (r) => `box-${r}`, input: ({ sandboxId, region }) => ({ sandboxId, region, allowedInstanceTypes: ['m7i.large'], maxInstancesPerAgent: 1, maxInstanceLifetimeHours: 1 }) } } });
    let types: string[] = [];
    named.sandbox.onApply = (i) => (types = i.allowedInstanceTypes);
    await run(named, 'run-b');
    expect(named.ledger.getOptimization('run-b')!.sandboxId).toBe('box-us-east-1');
    expect(types).toEqual(['m7i.large']);
  });

  it('simulation ids differ from live ones', async () => {
    const h = makeHarness({ mode: 'simulation', settings: { sandbox: { auto: true } } });
    await run(h, 'run-a');
    expect(h.ledger.getOptimization('run-a')!.sandboxId).toBe('ratchet-sim');
  });

  it('is reused while runs are active: concurrent first starts apply once', async () => {
    const h = live();
    await Promise.all([run(h, 'run-a'), run(h, 'run-b'), run(h, 'run-c')]);
    expect(applies(h)).toHaveLength(1);
    expect(new Set(['run-a', 'run-b', 'run-c'].map((i) => h.ledger.getOptimization(i)!.sandboxId))).toEqual(new Set([SBX]));
    await run(h, 'run-d');
    expect(applies(h)).toHaveLength(1);
  });

  it('a failed creation leaves the optimization a draft with a clear error, and a retry works', async () => {
    const h = live();
    h.sandbox.failApply = new Error('tofu apply failed (exit 1)');
    h.bench.set('main-sha', 'baseline', { samples: [100, 100, 100] });
    h.bench.set('main-sha', 'confirm', { samples: [100, 100, 100] });
    const id = await h.engine.createOptimization({ repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 3 });
    const err = await h.engine.start(id).then(() => null, (e: unknown) => e as Error & { code: string });
    expect(err).toMatchObject({ code: 'sandbox_unavailable' });
    expect(err!.message).toMatch(/could not create the environment in us-east-1: tofu apply failed/);
    expect(h.ledger.getOptimization(id)!.status).toBe('draft');
    expect(h.engine.runActive()).toBe(false);
    expect(sandboxEvents(h).map((e) => e.phase)).toEqual(['creating', 'failed']);
    h.sandbox.failApply = null;
    await h.engine.start(id);
    expect(h.ledger.getOptimization(id)!.status).toBe('running');
  });

  it('an auto sandbox is off unless asked for: the explicit rules still apply', async () => {
    const h = makeHarness({ mode: 'live' });
    await expect(h.engine.createOptimization({ repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 3 })).rejects.toMatchObject({ code: 'sandbox_not_applied' });
  });
});

describe('automatic sandbox: idle window and teardown', () => {
  it('is not destroyed while a run is active, however long it takes', async () => {
    const h = live();
    await run(h, 'run-a');
    await pass(h, 5 * 60 * MIN);
    expect(destroys(h)).toEqual([]);
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
    const act = await h.engine.sandboxActivity(SBX);
    expect(act).toMatchObject({ auto: true, activeRuns: ['run-a'], idleSince: null, destroyAt: null, lastReceipt: null });
  });

  it('is destroyed after the last run ends plus the idle window (default 30 min), with the ended runs torn down first', async () => {
    const h = live();
    await run(h, 'run-a', ['claude-code', 'codex'], 'cloud');
    await pass(h, 10 * MIN); // some runner time to bill
    await h.engine.stop('run-a');
    expect(h.compute.identities.size).toBe(2);

    await pass(h, 29 * MIN);
    expect(destroys(h)).toEqual([]); // still inside the window
    const waiting = await h.engine.sandboxActivity(SBX);
    expect(waiting.activeRuns).toEqual([]);
    expect(waiting.destroyAt).toBe(waiting.idleSince! + 30 * MIN);
    expect(await h.engine.teardownReceipt('run-a')).toBeNull();

    await pass(h, 2 * MIN);
    expect(destroys(h)).toEqual([`sandbox.destroy:${SBX}`]);
    // the ended run's cloud resources go first, then the sandbox itself
    expect(h.log.indexOf(`sandbox.destroy:${SBX}`)).toBeGreaterThan(h.log.lastIndexOf('compute.deleteIdentity:run-a-claude-code'));
    expect(h.ledger.getOptimization('run-a')!.status).toBe('completed'); // the engine's own teardown ran
    expect([...h.compute.identities.values()].every((i) => i.deleted)).toBe(true);
    expect(h.sandbox.applied.size).toBe(0);
    expect(h.compute.live()).toEqual([]);
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'destroyed', outputs: null });
    expect(sandboxEvents(h).map((e) => e.phase)).toEqual(['creating', 'ready', 'idle', 'destroying', 'destroyed']);
  });

  it('records a receipt: what was removed, what it cost, nothing left running', async () => {
    const h = live({ hourly: 2 });
    await run(h, 'run-a', ['codex'], 'cloud');
    await pass(h, 60 * MIN);
    await h.engine.stop('run-a');
    const spend = h.ledger.totalSpend('run-a');
    expect(spend).toBeGreaterThan(1); // an hour of a $2/h runner
    await pass(h, 31 * MIN);

    const receipt = await h.engine.teardownReceipt('run-a');
    expect(receipt).toMatchObject({
      sandboxId: SBX,
      region: 'us-east-1',
      trigger: 'idle',
      nothingRunning: true,
      orphans: [],
      errors: [],
      runs: [{ optimizationId: 'run-a', status: 'completed' }],
    });
    expect(receipt!.spendUsd).toBeCloseTo(h.ledger.totalSpend('run-a'), 6);
    expect(receipt!.spendUsd).toBeGreaterThanOrEqual(spend);
    expect(receipt!.spendByCategory['agent_runner']).toBeGreaterThan(1);
    expect(receipt!.removed.instances).toHaveLength(1);
    expect(receipt!.removed.agentIdentities).toEqual(['run-a-codex']);
    expect(receipt!.removed.secrets).toEqual(['run-a-codex']);
    expect(receipt!.removed.resources.join('\n')).toMatch(/ratchet-ratchet-sandbox-artifacts.*\n.*sealed/);
    expect(receipt!.destroyedAt).toBe(h.clock.now());
    expect(receipt!.createdAt).not.toBeNull();
    expect(receipt!.summary).toMatch(/removed after 1 run, \$\d+\.\d\d spent.*Nothing is left running/);
    expect((await h.engine.sandboxActivity(SBX)).lastReceipt).toEqual(receipt);
  });

  it('honours a configured idle window', async () => {
    const h = live({ settings: { sandbox: { auto: true, idleMs: 5 * MIN } } });
    await run(h, 'run-a');
    await h.engine.stop('run-a');
    await pass(h, 4 * MIN);
    expect(destroys(h)).toEqual([]);
    await pass(h, 2 * MIN);
    expect(destroys(h)).toHaveLength(1);
  });

  it('a run that starts inside the idle window reuses the sandbox and stops the clock', async () => {
    const h = live();
    await run(h, 'run-a');
    await h.engine.stop('run-a');
    await pass(h, 25 * MIN);
    await run(h, 'run-b');
    expect(applies(h)).toHaveLength(1);
    await pass(h, 60 * MIN);
    expect(destroys(h)).toEqual([]);
    expect((await h.engine.sandboxActivity(SBX)).activeRuns).toEqual(['run-b']);
    // the window restarts from the end of run-b, not from run-a
    await h.engine.stop('run-b');
    await pass(h, 29 * MIN);
    expect(destroys(h)).toEqual([]);
    await pass(h, 2 * MIN);
    expect(destroys(h)).toHaveLength(1);
    const receipt = (await h.engine.teardownReceipt('run-a'))!;
    expect(receipt.runs.map((r) => r.optimizationId).sort()).toEqual(['run-a', 'run-b']);
    expect(await h.engine.teardownReceipt('run-b')).toEqual(receipt);
  });

  it('a run after the teardown creates a fresh sandbox', async () => {
    const h = live();
    await run(h, 'run-a');
    await h.engine.stop('run-a');
    await pass(h, 31 * MIN);
    expect(h.sandbox.applied.size).toBe(0);
    await run(h, 'run-b');
    expect(applies(h)).toHaveLength(2);
    expect(h.ledger.getOptimization('run-b')!.sandboxId).toBe(SBX);
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
    expect(await h.engine.teardownReceipt('run-b')).toBeNull(); // its sandbox is alive again
  });

  it('keeps the idle clock across an app restart (a sandbox that outlived the app goes on the first tick)', async () => {
    const h = live();
    await run(h, 'run-a');
    await h.engine.stop('run-a');
    await pass(h, 10 * MIN);
    h.restart();
    await h.engine.ready();
    await pass(h, 10 * MIN);
    expect(destroys(h)).toEqual([]); // 20 min of idleness in total
    h.restart();
    await pass(h, 11 * MIN);
    expect(destroys(h)).toHaveLength(1);
    // closed for a day: the first tick after launch removes it
    const g = live();
    await run(g, 'run-a');
    await g.engine.stop('run-a');
    g.clock.advance(24 * 60 * MIN);
    g.restart();
    await g.engine.tick();
    expect(destroys(g)).toHaveLength(1);
  });

  it('a blocked destroy (an instance that will not terminate) is reported in the receipt path, not hidden, and retried later', async () => {
    const h = live({ settings: { sandbox: { auto: true, retryMs: 10 * MIN }, onError: () => {} } });
    await run(h, 'run-a', ['codex'], 'cloud');
    const runner = h.compute.live('run-a')[0]!;
    h.compute.stubborn.add(runner.id);
    await h.engine.stop('run-a');
    await pass(h, 31 * MIN);
    expect(destroys(h)).toEqual([]); // the orphan check refused
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
    expect(await h.engine.teardownReceipt('run-a')).toBeNull();
    expect(h.engine.errors.some((e) => /auto destroy|teardown/.test(e.context))).toBe(true);

    h.compute.stubborn.delete(runner.id);
    await pass(h, 2 * MIN);
    expect(destroys(h)).toEqual([]); // retry back-off
    await pass(h, 10 * MIN);
    expect(destroys(h)).toHaveLength(1);
    expect((await h.engine.teardownReceipt('run-a'))!.nothingRunning).toBe(true);
  });

  it('a provider failure during the destroy leaves the sandbox failed (infrastructure of unknown state) and is retried', async () => {
    const h = live({ settings: { sandbox: { auto: true, retryMs: 5 * MIN }, onError: () => {} } });
    await run(h, 'run-a');
    await h.engine.stop('run-a');
    h.sandbox.failDestroy = new Error('tofu destroy failed');
    await pass(h, 31 * MIN);
    expect((await h.engine.sandboxState(SBX)).status).toBe('failed');
    expect(sandboxEvents(h).at(-1)!.phase).toBe('failed');
    expect(await h.engine.teardownReceipt('run-a')).toBeNull();
    h.sandbox.failDestroy = null;
    await pass(h, 6 * MIN);
    expect(destroys(h)).toHaveLength(2);
    expect((await h.engine.sandboxState(SBX)).status).toBe('destroyed');
    expect(await h.engine.teardownReceipt('run-a')).toMatchObject({ nothingRunning: true });
  });

  it('an explicitly applied sandbox is never destroyed automatically', async () => {
    const h = live();
    await h.engine.applySandbox({ sandboxId: SBX, region: 'us-east-1', allowedInstanceTypes: ['c7i.xlarge'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 });
    await run(h, 'run-a');
    expect(applies(h)).toHaveLength(1); // reused, not re-applied
    await h.engine.stop('run-a');
    await pass(h, 120 * MIN);
    expect(destroys(h)).toEqual([]);
    expect((await h.engine.sandboxActivity(SBX)).auto).toBe(false);
  });

  it('an explicit apply or destroy takes an auto sandbox out of automatic management', async () => {
    const h = live();
    await run(h, 'run-a');
    await h.engine.stop('run-a');
    await h.engine.teardown('run-a');
    await h.engine.applySandbox({ sandboxId: SBX, region: 'us-east-1', allowedInstanceTypes: ['c7i.xlarge'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 });
    await pass(h, 120 * MIN);
    expect(destroys(h)).toEqual([]);
    await h.engine.destroySandbox(SBX);
    expect(destroys(h)).toHaveLength(1);
  });

  it('is off by default: nothing is destroyed behind an explicit user', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox({ sandboxId: SBX, region: 'us-east-1', allowedInstanceTypes: ['c7i.xlarge'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 });
    const { id } = await h.begin({ sandboxId: SBX });
    await h.engine.stop(id);
    await pass(h, 600 * MIN);
    expect(destroys(h)).toEqual([]);
  });
});

describe('endSandbox (manual)', () => {
  it('refuses while a run is active, then tears down on demand with a receipt', async () => {
    const h = live();
    await run(h, 'run-a');
    await expect(h.engine.endSandbox(SBX)).rejects.toMatchObject({ code: 'sandbox_in_use' });
    await h.engine.stop('run-a');
    const receipt = await h.engine.endSandbox(SBX);
    expect(receipt).toMatchObject({ trigger: 'manual', nothingRunning: true });
    expect(destroys(h)).toHaveLength(1); // no waiting for the idle window
    expect(await h.engine.teardownReceipt('run-a')).toEqual(receipt);
    await expect(h.engine.endSandbox(SBX)).rejects.toMatchObject({ code: 'invalid_state' });
    await expect(h.engine.endSandbox('nope')).rejects.toMatchObject({ code: 'not_found' });
  });
});

describe('run-active (sleep prevention)', () => {
  const flips = (h: Harness): Array<[boolean, string[]]> =>
    (h.events as EngineEvent[]).flatMap((e) => (e.type === 'run-active' ? [[e.active, e.optimizationIds] as [boolean, string[]]] : []));

  it('flips true when a run starts (even while its environment is still being created) and false when the last one ends', async () => {
    const h = live();
    expect(h.engine.runActive()).toBe(false);
    let duringApply: boolean | undefined;
    h.sandbox.onApply = () => (duringApply = h.engine.runActive());
    await run(h, 'run-a');
    expect(duringApply).toBe(true);
    expect(h.engine.runActive()).toBe(true);
    await run(h, 'run-b');
    expect(flips(h)).toEqual([[true, ['run-a']]]); // only transitions are emitted
    await h.engine.stop('run-a');
    expect(h.engine.runActive()).toBe(true);
    await h.engine.stop('run-b');
    expect(h.engine.runActive()).toBe(false);
    expect(flips(h).map(([a]) => a)).toEqual([true, false]);
  });

  it('works without the automatic sandbox, and a failed start does not leave it stuck', async () => {
    const h = makeHarness();
    const { id } = await h.begin();
    expect(h.engine.runActive()).toBe(true);
    await h.engine.stop(id);
    expect(h.engine.runActive()).toBe(false);

    const bad = makeHarness();
    const id2 = await bad.engine.createOptimization({ repoUrl: bad.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 3 });
    await expect(bad.engine.start(id2)).rejects.toThrow(/no scripted bench result/); // the baseline blows up
    expect(bad.engine.runActive()).toBe(false);
  });

  it('is true again after a restart with a run in flight', async () => {
    const h = makeHarness();
    await h.begin();
    h.restart();
    await h.engine.ready();
    expect(h.engine.runActive()).toBe(true);
  });
});

describe('TICK constant sanity', () => {
  it('one engine tick is a minute in the harness (the windows above are multiples of it)', () => {
    expect(TICK_MS).toBe(MIN);
  });
});
