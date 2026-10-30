import { describe, it, expect } from 'vitest';
import { MockSandboxProvider, SANDBOX_RESOURCES } from './mock.js';
import { TeardownBlockedError } from './tofu.js';

const input = { sandboxId: 'sim', region: 'us-east-1', allowedInstanceTypes: ['c7i.large'], maxInstancesPerAgent: 4, maxInstanceLifetimeHours: 2 };

describe('MockSandboxProvider', () => {
  it('plans every sandbox resource with real addresses', async () => {
    const p = new MockSandboxProvider();
    const r = await p.plan(input);
    expect(r.changes).toHaveLength(SANDBOX_RESOURCES.length);
    expect(r.changes.every((c) => c.action === 'create')).toBe(true);
    expect(r.changes.map((c) => c.address)).toEqual(SANDBOX_RESOURCES.map((x) => x.address));
    expect(r.changes.map((c) => c.address)).toContain('aws_s3_bucket_policy.sealed_deny_agents');
    expect(r.summary).toBe(`Plan: ${SANDBOX_RESOURCES.length} to add, 0 to change, 0 to destroy.`);
  });

  it('apply produces contract outputs; outputs() and re-plan reflect it', async () => {
    const p = new MockSandboxProvider();
    expect(await p.outputs('sim')).toBeNull();
    const o = await p.apply(input);
    expect(o.artifactsBucket).toBe('ratchet-sim-artifacts');
    expect(o.agentRolePath).toBe('/ratchet/agent/');
    expect(o.limits).toEqual({ maxInstancesPerAgent: 4, maxInstanceLifetimeHours: 2 });
    expect(await p.outputs('sim')).toBe(o);
    expect(await p.apply(input)).toBe(o); // idempotent
    expect((await p.plan(input)).changes.every((c) => c.action === 'no-op')).toBe(true);
  });

  it('destroy honours the teardown guard like the real provider', async () => {
    let roles = ['r1'];
    const p = new MockSandboxProvider({ listRuntimeAgentRoles: async () => roles });
    await p.apply(input);
    await expect(p.destroy(input)).rejects.toBeInstanceOf(TeardownBlockedError);
    expect(await p.outputs('sim')).not.toBeNull();
    roles = [];
    await p.destroy(input);
    expect(await p.outputs('sim')).toBeNull();
  });

  it('validates the sandbox id', async () => {
    await expect(new MockSandboxProvider().plan({ ...input, sandboxId: 'BAD' })).rejects.toThrow();
  });
});
