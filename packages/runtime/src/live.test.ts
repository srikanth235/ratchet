import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { openLedger, type Ledger, type SandboxInput } from '@ratchet/core';
import type { AwsSandboxOutputs, ProcessResult, ProcessRunner, ProcessSpec } from '@ratchet/aws';
import { createLiveRuntime, type LiveOptions, type LiveRuntime } from './live.js';

const ACCT = '123456789012';
const identity = (n: string) => ({
  role_arn: `arn:aws:iam::${ACCT}:role/ratchet/bench/${n}`,
  role_name: n,
  instance_profile_arn: `arn:aws:iam::${ACCT}:instance-profile/ratchet/bench/${n}`,
  instance_profile_name: n,
});
const OUTPUTS_JSON = JSON.stringify({
  artifacts_bucket: { value: 'ratchet-sbx-artifacts' },
  sealed_bucket: { value: 'ratchet-sbx-sealed' },
  agent_boundary_ref: { value: `arn:aws:iam::${ACCT}:policy/ratchet/boundary` },
  eval_identity_ref: { value: identity('bench-runner') },
  confirm_identity_ref: { value: identity('confirm-runner') },
  sealed_confirm_prefix: { value: 'holdout/' },
  network_ref: {
    value: { vpc_id: 'vpc-1', subnet_id: 'subnet-1', bench_subnet_id: 'subnet-bench', security_group_ids: { agent_runner: 'sg-r', bench: 'sg-b' }, egress_proxy_url: null },
  },
  agent_role_path: { value: '/ratchet/agent/' },
  limits: { value: { max_instances_per_agent: 2, max_instance_lifetime_hours: 4 } },
});
const PLAN_JSON = JSON.stringify({
  resource_changes: [
    { address: 'aws_s3_bucket.artifacts', mode: 'managed', type: 'aws_s3_bucket', change: { actions: ['create'] } },
    { address: 'aws_vpc.sandbox', mode: 'managed', type: 'aws_vpc', change: { actions: ['create'] } },
  ],
});

class FakeTofu implements ProcessRunner {
  calls: ProcessSpec[] = [];
  async run(spec: ProcessSpec): Promise<ProcessResult> {
    this.calls.push(spec);
    const sub = spec.args[0]!;
    if (sub === 'plan') return { exitCode: 2, stdout: '', stderr: '' };
    if (sub === 'show') return { exitCode: 0, stdout: PLAN_JSON, stderr: '' };
    if (sub === 'output') return { exitCode: 0, stdout: OUTPUTS_JSON, stderr: '' };
    return { exitCode: 0, stdout: '', stderr: '' };
  }
  subs = () => this.calls.map((c) => c.args[0]);
}

const input: SandboxInput = { sandboxId: 'sbx', region: 'us-east-1', allowedInstanceTypes: ['c7i.xlarge'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 };
const roots: string[] = [];
const runtimes: LiveRuntime[] = [];
afterEach(async () => {
  await Promise.all(runtimes.splice(0).map((r) => r.dispose()));
  for (const r of roots.splice(0)) rmSync(r, { recursive: true, force: true });
});

function build(ledger: Ledger, over: Partial<LiveOptions> = {}) {
  const stateRoot = mkdtempSync(join(tmpdir(), 'ratchet-live-'));
  roots.push(stateRoot);
  const tofu = new FakeTofu();
  const rt = createLiveRuntime({ ledger, region: 'us-east-1', sandbox: null, sandboxId: 'sbx', tofuBinary: '/opt/tofu', moduleDir: '/repo/infra/aws', stateRoot, tofuRunner: tofu, awsProfile: 'sandbox', ...over });
  runtimes.push(rt);
  return { rt, tofu, stateRoot };
}

describe('createLiveRuntime', () => {
  it('before the first apply: sandbox plan/apply/destroy work, everything else says what is missing', async () => {
    const ledger = openLedger(':memory:');
    const { rt, tofu } = build(ledger);
    expect(rt.engine.mode()).toBe('live');
    expect(rt.repoUrl).toBeNull();
    expect(rt.missing).toEqual(expect.arrayContaining([expect.stringMatching(/apply the sandbox/), expect.stringMatching(/GitHub/), expect.stringMatching(/runner AMI/), expect.stringMatching(/bench AMI/)]));

    const plan = await rt.engine.planSandbox(input);
    expect(plan.changes.map((c) => c.address)).toEqual(['aws_s3_bucket.artifacts', 'aws_vpc.sandbox']);
    expect(plan.summary).toBe('Plan: 2 to add, 0 to change, 0 to destroy.');
    expect(ledger.getSandbox('sbx')).toMatchObject({ status: 'planned', provider: 'aws' });

    // tofu runs with the chosen profile
    expect(tofu.calls.every((c) => c.env.AWS_PROFILE === 'sandbox')).toBe(true);

    // no optimization can start without an applied sandbox
    await expect(rt.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 1, timeLimitSeconds: 60, plateauEvals: 1, sandboxId: 'sbx' })).rejects.toMatchObject({ code: 'sandbox_not_applied' });

    const out = await rt.engine.applySandbox(input);
    expect(out.artifactsBucket).toBe('ratchet-sbx-artifacts');
    expect(tofu.subs()).toContain('apply');
    const row = ledger.getSandbox('sbx')!;
    expect(row).toMatchObject({ status: 'applied', accountId: ACCT });
    expect((row.outputs as unknown as AwsSandboxOutputs).network.benchSubnetId).toBe('subnet-bench');

    // applied, but GitHub is not configured: a clear error from the unready port, nothing is created
    await expect(rt.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 1, timeLimitSeconds: 60, plateauEvals: 1, sandboxId: 'sbx' })).rejects.toMatchObject({
      code: 'not_configured',
      message: expect.stringMatching(/GitHub is not ready: set the GitHub repository and token/),
    });
    expect(ledger.listOptimizations()).toHaveLength(0);

    // destroy: the compute port does not exist yet (this runtime was built before the apply), which means no orphans
    await rt.engine.destroySandbox('sbx');
    expect(tofu.subs()).toContain('destroy');
    expect(ledger.getSandbox('sbx')).toMatchObject({ status: 'destroyed', outputs: null });
  });

  it('rebuilt from the ledger outputs and a full configuration, nothing is missing', async () => {
    const ledger = openLedger(':memory:');
    const first = build(ledger);
    await first.rt.engine.applySandbox(input);
    const outputs = ledger.getSandbox('sbx')!.outputs as unknown as AwsSandboxOutputs;

    const { rt } = build(ledger, {
      sandbox: outputs,
      githubRepo: 'acme/api',
      githubToken: 'ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE12',
      runnerImageRef: 'ami-0123456789abcdef0',
      benchImageRef: 'ami-0fedcba9876543210',
      secrets: (_kind, name) => `value-of-${name}`,
    });
    expect(rt.missing).toEqual([]);
    expect(rt.repoUrl).toBe('https://github.com/acme/api.git');
    expect((await rt.engine.sandboxState('sbx')).status).toBe('applied');
  });

  it('reports only what is still missing', () => {
    const ledger = openLedger(':memory:');
    const { rt } = build(ledger, { githubRepo: 'acme/api', githubToken: 'ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE12', runnerImageRef: 'ami-0123456789abcdef0' });
    expect(rt.missing).toHaveLength(2);
    expect(rt.missing.join(' ')).toMatch(/apply the sandbox/);
    expect(rt.missing.join(' ')).toMatch(/bench AMI/);
    expect(rt.repoUrl).toBe('https://github.com/acme/api.git');
  });
});
