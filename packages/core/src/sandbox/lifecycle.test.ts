import { describe, expect, it } from 'vitest';
import { openLedger } from '../ledger/db.js';
import type { SandboxInput, SandboxOutputs, SandboxProvider } from '../ports/index.js';
import { SandboxLifecycle, SandboxLifecycleError } from './lifecycle.js';

const input: SandboxInput = { sandboxId: 'sbx-1', region: 'us-east-1', allowedInstanceTypes: ['c7i.large'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 };
const OUTPUTS: SandboxOutputs = {
  artifactsBucket: 'a',
  sealedBucket: 's',
  agentBoundaryRef: 'arn:aws:iam::123456789012:policy/b',
  evalIdentityRef: 'arn:aws:iam::123456789012:instance-profile/e',
  agentRolePath: '/ratchet/agent/',
  network: { subnetId: 'subnet-1', agentRunnerSecurityGroupId: 'sg-1', benchSecurityGroupId: 'sg-2' },
};

class Provider implements SandboxProvider {
  calls: string[] = [];
  failApply: Error | null = null;
  failDestroy: Error | null = null;
  inputs: SandboxInput[] = [];
  async plan() {
    this.calls.push('plan');
    return { changes: [{ action: 'create' as const, address: 'aws_s3_bucket.a', type: 'aws_s3_bucket' }], summary: 'Plan: 1 to add, 0 to change, 0 to destroy.' };
  }
  async apply(i: SandboxInput) {
    this.calls.push('apply');
    this.inputs.push(i);
    if (this.failApply) throw this.failApply;
    return OUTPUTS;
  }
  async destroy(i: SandboxInput) {
    this.calls.push('destroy');
    this.inputs.push(i);
    if (this.failDestroy) throw this.failDestroy;
  }
  async outputs() {
    return null;
  }
}

function setup(blockers?: (id: string) => string[]) {
  const ledger = openLedger(':memory:');
  const provider = new Provider();
  const stored = new Map<string, SandboxInput>();
  const lifecycle = new SandboxLifecycle({
    ledger,
    provider,
    kind: 'mock',
    ...(blockers ? { blockers } : {}),
    inputs: { get: (id) => stored.get(id), set: (id, i) => void stored.set(id, i) },
  });
  return { ledger, provider, lifecycle };
}

const addOptimization = (ledger: ReturnType<typeof openLedger>, id: string, status: 'draft' | 'running' | 'stopped') =>
  ledger.createOptimization({ id, repo: 'r', baseRef: 'main', metric: 'm', direction: 'minimize', minGain: 0.1, budgetUsd: 10, timeLimitS: 60, configYaml: 'x', status, sandboxId: 'sbx-1' });

describe('SandboxLifecycle', () => {
  it('reports an absent sandbox, then walks planned -> applying -> applied with outputs and account', async () => {
    const { ledger, lifecycle, provider } = setup();
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'absent', outputs: null });
    const plan = await lifecycle.plan(input);
    expect(plan.summary).toMatch(/1 to add/);
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'planned', provider: 'mock', region: 'us-east-1' });

    const seen: string[] = [];
    const orig = provider.apply.bind(provider);
    provider.apply = async (i) => {
      seen.push(ledger.getSandbox('sbx-1')!.status);
      return orig(i);
    };
    const out = await lifecycle.apply(input);
    expect(out).toEqual(OUTPUTS);
    expect(seen).toEqual(['applying']);
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'applied', accountId: '123456789012', outputs: OUTPUTS });
    expect(lifecycle.state('sbx-1').appliedAt).not.toBeNull();
  });

  it('records a failed apply, keeps the error, and can be retried', async () => {
    const { lifecycle, provider } = setup();
    provider.failApply = new Error('tofu apply failed');
    await expect(lifecycle.apply(input)).rejects.toThrow('tofu apply failed');
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'failed', lastError: 'tofu apply failed' });
    provider.failApply = null;
    await lifecycle.apply(input);
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'applied', lastError: null });
  });

  it('destroys with the input it was applied with, clears outputs, and is idempotent', async () => {
    const { lifecycle, provider } = setup();
    await lifecycle.apply({ ...input, maxInstancesPerAgent: 3 });
    await lifecycle.destroy('sbx-1');
    expect(provider.inputs.at(-1)).toMatchObject({ maxInstancesPerAgent: 3 });
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'destroyed', outputs: null });
    expect(lifecycle.state('sbx-1').destroyedAt).not.toBeNull();
    await lifecycle.destroy('sbx-1');
    expect(provider.calls.filter((c) => c === 'destroy')).toHaveLength(1);
    // a destroyed sandbox can be applied again
    await lifecycle.apply(input);
    expect(lifecycle.state('sbx-1').status).toBe('applied');
  });

  it('refuses to destroy something never applied, or unknown', async () => {
    const { lifecycle } = setup();
    await expect(lifecycle.destroy('nope')).rejects.toMatchObject({ code: 'not_found' });
    await lifecycle.plan(input);
    await expect(lifecycle.destroy('sbx-1')).rejects.toMatchObject({ code: 'invalid_state' });
  });

  it('refuses to destroy while an optimization in the sandbox is running, and changes nothing', async () => {
    const { ledger, lifecycle, provider } = setup();
    await lifecycle.apply(input);
    addOptimization(ledger, 'opt-1', 'running');
    const err = await lifecycle.destroy('sbx-1').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SandboxLifecycleError);
    expect(err).toMatchObject({ code: 'sandbox_in_use' });
    expect((err as Error).message).toContain('opt-1');
    expect(provider.calls).not.toContain('destroy');
    expect(lifecycle.state('sbx-1').status).toBe('applied');
    ledger.setOptimizationStatus('opt-1', 'stopped');
    await lifecycle.destroy('sbx-1');
    expect(lifecycle.state('sbx-1').status).toBe('destroyed');
  });

  it('refuses while the orphan check reports leftovers, and runs it fail-closed', async () => {
    let orphans = ['i-0abc running'];
    const { lifecycle, provider } = setup(() => orphans);
    await lifecycle.apply(input);
    await expect(lifecycle.destroy('sbx-1')).rejects.toMatchObject({ code: 'orphans' });
    expect(provider.calls).not.toContain('destroy');
    expect(lifecycle.state('sbx-1').status).toBe('applied');
    orphans = [];
    await lifecycle.destroy('sbx-1');
    expect(lifecycle.state('sbx-1').status).toBe('destroyed');

    const ledger = openLedger(':memory:');
    const l2 = new SandboxLifecycle({ ledger, provider: new Provider(), kind: 'mock', blockers: () => { throw new Error('listTagged failed'); } });
    await l2.apply(input);
    await expect(l2.destroy('sbx-1')).rejects.toThrow('listTagged failed');
    expect(l2.state('sbx-1').status).toBe('applied');
  });

  it("restores the previous status when the provider's own teardown guard refuses", async () => {
    const { lifecycle, provider } = setup();
    await lifecycle.apply(input);
    provider.failDestroy = Object.assign(new Error('refusing to destroy sandbox: 1 runtime agent role(s) still exist'), { name: 'TeardownBlockedError' });
    await expect(lifecycle.destroy('sbx-1')).rejects.toMatchObject({ code: 'orphans' });
    expect(lifecycle.state('sbx-1').status).toBe('applied');
  });

  it('marks a destroy that failed inside the provider as failed (state unknown) and allows a retry', async () => {
    const { lifecycle, provider } = setup();
    await lifecycle.apply(input);
    provider.failDestroy = new Error('tofu destroy failed');
    await expect(lifecycle.destroy('sbx-1')).rejects.toThrow('tofu destroy failed');
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'failed', lastError: 'tofu destroy failed' });
    provider.failDestroy = null;
    await lifecycle.destroy('sbx-1');
    expect(lifecycle.state('sbx-1').status).toBe('destroyed');
  });

  it('rejects a second concurrent operation on the same sandbox', async () => {
    const { lifecycle, provider } = setup();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const orig = provider.apply.bind(provider);
    provider.apply = async (i) => (await gate, orig(i));
    const first = lifecycle.apply(input);
    await expect(lifecycle.apply(input)).rejects.toMatchObject({ code: 'invalid_state' });
    release();
    await first;
    expect(lifecycle.state('sbx-1').status).toBe('applied');
  });

  it('recover() marks interrupted operations as failed', async () => {
    const { ledger, lifecycle } = setup();
    ledger.recordSandbox({ id: 'sbx-1', provider: 'mock', region: 'us-east-1', status: 'applying' });
    ledger.recordSandbox({ id: 'sbx-2', provider: 'mock', region: 'us-east-1', status: 'destroying' });
    ledger.recordSandbox({ id: 'sbx-3', provider: 'mock', region: 'us-east-1', status: 'applied' });
    expect(lifecycle.recover().sort()).toEqual(['sbx-1', 'sbx-2']);
    expect(lifecycle.state('sbx-1')).toMatchObject({ status: 'failed' });
    expect(lifecycle.state('sbx-1').lastError).toMatch(/interrupted/);
    expect(lifecycle.state('sbx-3').status).toBe('applied');
  });
});
