import { describe, expect, it } from 'vitest';
import type { SandboxInput } from '@ratchet/core';
import { EngineError } from './errors.js';
import { makeHarness } from './testing/harness.js';

const SBX = 'ratchet-sandbox';
const input: SandboxInput = { sandboxId: SBX, region: 'us-east-1', allowedInstanceTypes: ['c7i.xlarge'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 };
const draft = (h: ReturnType<typeof makeHarness>, extra: { sandboxId?: string } = {}) =>
  h.engine.createOptimization({ repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 3, ...extra });

describe('sandbox lifecycle', () => {
  it('plans, applies and records planned -> applying -> applied with the outputs in the ledger', async () => {
    const h = makeHarness({ mode: 'live' });
    expect((await h.engine.sandboxState(SBX)).status).toBe('absent');

    const plan = await h.engine.planSandbox(input);
    expect(plan).toMatchObject({ summary: 'Plan: 1 to add, 0 to change, 0 to destroy.' });
    expect(plan.changes[0]).toMatchObject({ action: 'create', address: 'aws_s3_bucket.artifacts' });
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'planned', provider: 'aws', region: 'us-east-1', outputs: null });

    const statusDuringApply: string[] = [];
    h.sandbox.onApply = () => statusDuringApply.push(h.ledger.getSandbox(SBX)!.status);
    const out = await h.engine.applySandbox(input);
    expect(statusDuringApply).toEqual(['applying']);
    expect(out.artifactsBucket).toBe('ratchet-ratchet-sandbox-artifacts');

    const state = await h.engine.sandboxState(SBX);
    expect(state).toMatchObject({ status: 'applied', accountId: '123456789012', outputs: out });
    expect(state.appliedAt).not.toBeNull();
    expect(h.ledger.getSandbox(SBX)!.outputs).toMatchObject({ artifactsBucket: 'ratchet-ratchet-sandbox-artifacts', network: { subnetId: 'subnet-1' } });
    // a re-plan of an applied sandbox shows nothing to do
    expect((await h.engine.planSandbox(input)).changes.every((c) => c.action === 'no-op')).toBe(true);
  });

  it('records a failed apply and recovers on retry', async () => {
    const h = makeHarness({ mode: 'live' });
    h.sandbox.failApply = new Error('tofu apply failed (exit 1)');
    await expect(h.engine.applySandbox(input)).rejects.toThrow('tofu apply failed');
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'failed', lastError: 'tofu apply failed (exit 1)' });
    h.sandbox.failApply = null;
    await h.engine.applySandbox(input);
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
  });

  it('marks an apply that a crash interrupted as failed when the engine restarts', async () => {
    const h = makeHarness({ mode: 'live' });
    h.ledger.recordSandbox({ id: SBX, provider: 'aws', region: 'us-east-1', status: 'applying' });
    h.restart();
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'failed' });
    expect((await h.engine.sandboxState(SBX)).lastError).toMatch(/interrupted/);
    await h.engine.applySandbox(input);
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
  });

  it('destroys with the input it was applied with: applied -> destroying -> destroyed, outputs cleared', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    const seen: string[] = [];
    const orig = h.sandbox.destroy.bind(h.sandbox);
    h.sandbox.destroy = async (i) => {
      seen.push(h.ledger.getSandbox(SBX)!.status);
      expect(i).toEqual(input);
      return orig(i);
    };
    await h.engine.destroySandbox(SBX);
    expect(seen).toEqual(['destroying']);
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'destroyed', outputs: null });
    expect(h.sandbox.applied.has(SBX)).toBe(false);
  });

  it('a destroy that fails inside the provider leaves the sandbox failed, and can be retried', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    h.sandbox.failDestroy = new Error('tofu destroy failed');
    await expect(h.engine.destroySandbox(SBX)).rejects.toThrow('tofu destroy failed');
    expect(await h.engine.sandboxState(SBX)).toMatchObject({ status: 'failed', lastError: 'tofu destroy failed' });
    h.sandbox.failDestroy = null;
    await h.engine.destroySandbox(SBX);
    expect((await h.engine.sandboxState(SBX)).status).toBe('destroyed');
  });

  it('keeps the ledger state across an engine restart', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    h.restart();
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
    await h.engine.destroySandbox(SBX); // the input survives in the engine's kv table
    expect((await h.engine.sandboxState(SBX)).status).toBe('destroyed');
  });
});

describe('optimizations and the sandbox', () => {
  it('live mode refuses to create or start an optimization without an applied sandbox', async () => {
    const h = makeHarness({ mode: 'live' });
    await expect(draft(h)).rejects.toMatchObject({ code: 'sandbox_not_applied' });
    await expect(draft(h, { sandboxId: SBX })).rejects.toMatchObject({ code: 'sandbox_not_applied' });
    await h.engine.planSandbox(input); // planned is not applied
    await expect(draft(h, { sandboxId: SBX })).rejects.toThrow(/planned/);
    expect(h.ledger.listOptimizations()).toHaveLength(0);

    await h.engine.applySandbox(input);
    const id = await draft(h, { sandboxId: SBX });
    expect(h.ledger.getOptimization(id)!.sandboxId).toBe(SBX);

    // The sandbox disappears between create and start: start refuses and nothing is launched.
    await h.engine.destroySandbox(SBX);
    await expect(h.engine.start(id)).rejects.toMatchObject({ code: 'sandbox_not_applied' });
    expect(h.ledger.getOptimization(id)!.status).toBe('draft');
    expect(h.log.filter((l) => l.startsWith('compute.'))).toEqual([]);
  });

  it('live mode links the sandbox to the optimization it runs in', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    const { id } = await h.begin({ sandboxId: SBX });
    expect(h.ledger.getOptimization(id)).toMatchObject({ sandboxId: SBX, status: 'running' });
    expect((await h.engine.sandboxState(SBX)).optimizations).toEqual([{ id, status: 'running' }]);
  });

  it('simulation does not need a sandbox, and links one only once it is applied', async () => {
    const h = makeHarness();
    const a = await draft(h, { sandboxId: SBX });
    expect(h.ledger.getOptimization(a)!.sandboxId).toBeNull();
    await h.engine.applySandbox(input);
    const b = await h.engine.createOptimization({ id: 'opt-b', repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 10, timeLimitSeconds: 600, plateauEvals: 3, sandboxId: SBX });
    expect(h.ledger.getOptimization(b)!.sandboxId).toBe(SBX);
    await expect(draft(h, { sandboxId: 'Not Valid' })).rejects.toMatchObject({ code: 'invalid_id' });
  });

  it('refuses to destroy while an optimization in the sandbox is running, then allows it after teardown', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    const { id } = await h.begin({ sandboxId: SBX });
    const err = await h.engine.destroySandbox(SBX).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(EngineError);
    expect(err).toMatchObject({ code: 'sandbox_in_use' });
    expect((err as Error).message).toContain(id);
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
    expect(h.log).not.toContain(`sandbox.destroy:${SBX}`);

    await h.engine.stop(id);
    // stopped, but its agent identities still exist: teardown first (the existing orphan check)
    await expect(h.engine.destroySandbox(SBX)).rejects.toMatchObject({ code: 'orphans' });
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');

    await h.engine.teardown(id);
    await h.engine.destroySandbox(SBX);
    expect((await h.engine.sandboxState(SBX)).status).toBe('destroyed');
  });

  it('refuses to destroy while a tagged instance is still alive', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    const { id, agents } = await h.begin({ sandboxId: SBX });
    await h.engine.stop(id);
    const stuck = h.compute.launchExperiment(id, agents.codex!);
    h.compute.stubborn.add(stuck.id);
    await h.engine.teardown(id).catch(() => undefined); // leaves the orphan
    const err = await h.engine.destroySandbox(SBX).catch((e: unknown) => e);
    expect(err).toMatchObject({ code: 'orphans' });
    expect((err as Error).message).toContain(stuck.id);
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
    h.compute.stubborn.clear();
    await h.engine.teardown(id);
    await h.engine.destroySandbox(SBX);
    expect((await h.engine.sandboxState(SBX)).status).toBe('destroyed');
  });

  it('fails closed when the orphan check itself cannot run', async () => {
    const h = makeHarness({ mode: 'live' });
    await h.engine.applySandbox(input);
    h.compute.listTagged = async () => {
      throw new Error('ec2 throttled');
    };
    await expect(h.engine.destroySandbox(SBX)).rejects.toThrow('ec2 throttled');
    expect((await h.engine.sandboxState(SBX)).status).toBe('applied');
  });

  it('treats a compute that does not exist yet (no sandbox outputs) as having no orphans', async () => {
    const h = makeHarness({ mode: 'live' });
    h.sandbox.failApply = new Error('boom');
    await h.engine.applySandbox(input).catch(() => undefined);
    h.compute.listTagged = async () => {
      throw new EngineError('not_configured', 'apply the sandbox first');
    };
    h.sandbox.failApply = null;
    await h.engine.destroySandbox(SBX);
    expect((await h.engine.sandboxState(SBX)).status).toBe('destroyed');
  });
});
