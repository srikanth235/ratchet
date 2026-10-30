import { MockSandboxProvider } from './mock.js';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, readFile, rm, writeFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { SandboxInput } from '@ratchet/core';
import type { ProcessResult, ProcessRunner, ProcessSpec } from './process.js';
import { TeardownBlockedError, TofuError, TofuSandboxProvider, mapActions, parseTofuOutputs, summarizePlan } from './tofu.js';

const input: SandboxInput = {
  sandboxId: 'demo',
  region: 'us-east-1',
  allowedInstanceTypes: ['c7i.large', 'c7i.2xlarge'],
  maxInstancesPerAgent: 4,
  maxInstanceLifetimeHours: 2,
  tags: { owner: 'me' },
};

const PLAN_JSON = JSON.stringify({
  resource_changes: [
    { address: 'aws_s3_bucket.artifacts', mode: 'managed', type: 'aws_s3_bucket', change: { actions: ['create'] } },
    { address: 'aws_s3_bucket.sealed', mode: 'managed', type: 'aws_s3_bucket', change: { actions: ['update'] } },
    { address: 'aws_iam_role.bench_runner', mode: 'managed', type: 'aws_iam_role', change: { actions: ['delete', 'create'] } },
    { address: 'aws_security_group.bench', mode: 'managed', type: 'aws_security_group', change: { actions: ['delete'] } },
    { address: 'aws_iam_policy.agent_boundary', mode: 'managed', type: 'aws_iam_policy', change: { actions: ['no-op'] } },
    { address: 'data.aws_availability_zones.available', mode: 'data', type: 'aws_availability_zones', change: { actions: ['read'] } },
  ],
});

const OUTPUTS_JSON = JSON.stringify({
  artifacts_bucket: { value: 'ratchet-demo-artifacts' },
  sealed_bucket: { value: 'ratchet-demo-sealed' },
  agent_boundary_ref: { value: 'arn:aws:iam::1:policy/ratchet/ratchet-demo-agent-boundary' },
  eval_identity_ref: {
    value: {
      role_arn: 'arn:aws:iam::1:role/ratchet/bench/ratchet-demo-bench-runner',
      role_name: 'ratchet-demo-bench-runner',
      instance_profile_arn: 'arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-bench-runner',
      instance_profile_name: 'ratchet-demo-bench-runner',
    },
  },
  confirm_identity_ref: {
    value: {
      role_arn: 'arn:aws:iam::1:role/ratchet/bench/ratchet-demo-confirm-runner',
      role_name: 'ratchet-demo-confirm-runner',
      instance_profile_arn: 'arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-confirm-runner',
      instance_profile_name: 'ratchet-demo-confirm-runner',
    },
  },
  deps_identity_ref: {
    value: {
      role_arn: 'arn:aws:iam::1:role/ratchet/bench/ratchet-demo-deps-runner',
      role_name: 'ratchet-demo-deps-runner',
      instance_profile_arn: 'arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-deps-runner',
      instance_profile_name: 'ratchet-demo-deps-runner',
    },
  },
  sealed_confirm_prefix: { value: 'holdout/' },
  network_ref: {
    value: {
      vpc_id: 'vpc-1',
      subnet_id: 'subnet-1',
      bench_subnet_id: 'subnet-bench',
      security_group_ids: { agent_runner: 'sg-r', bench: 'sg-b' },
      egress_proxy_url: null,
    },
  },
  agent_role_path: { value: '/ratchet/agent/' },
  limits: { value: { max_instances_per_agent: 4, max_instance_lifetime_hours: 2 } },
});

class FakeRunner implements ProcessRunner {
  calls: ProcessSpec[] = [];
  planExit = 2;
  failOn: string | undefined;
  inflight = 0;
  maxInflight = 0;
  async run(spec: ProcessSpec): Promise<ProcessResult> {
    this.calls.push(spec);
    this.inflight++;
    this.maxInflight = Math.max(this.maxInflight, this.inflight);
    await new Promise((r) => setTimeout(r, 1));
    this.inflight--;
    const sub = spec.args[0]!;
    if (this.failOn === sub) return { exitCode: 1, stdout: '', stderr: 'Error: boom' };
    switch (sub) {
      case 'plan':
        return { exitCode: this.planExit, stdout: '', stderr: '' };
      case 'show':
        return { exitCode: 0, stdout: PLAN_JSON, stderr: '' };
      case 'output':
        return { exitCode: 0, stdout: OUTPUTS_JSON, stderr: '' };
      default:
        return { exitCode: 0, stdout: '', stderr: '' };
    }
  }
  subs() {
    return this.calls.map((c) => c.args[0]);
  }
}

let root: string;
let runner: FakeRunner;
let roles: string[];
let tofu: TofuSandboxProvider;

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'ratchet-tofu-'));
  runner = new FakeRunner();
  roles = [];
  tofu = new TofuSandboxProvider({
    tofuBinary: '/opt/tofu',
    moduleDir: '/repo/infra/aws',
    stateRoot: root,
    runner,
    env: { AWS_PROFILE: 'sandbox' },
    listRuntimeAgentRoles: async () => roles,
  });
});
afterEach(() => rm(root, { recursive: true, force: true }));

describe('plan', () => {
  it('init -> plan -out -> show -json, with per-sandbox dir and vars file', async () => {
    const r = await tofu.plan(input);
    expect(runner.subs()).toEqual(['init', 'plan', 'show']);
    expect(runner.calls.every((c) => c.command === '/opt/tofu' && c.cwd === '/repo/infra/aws')).toBe(true);
    const dir = join(root, 'demo');
    const plan = runner.calls[1]!;
    expect(plan.args).toEqual(
      expect.arrayContaining([
        '-detailed-exitcode',
        `-out=${dir}/plan.tfplan`,
        `-state=${dir}/terraform.tfstate`,
        `-var-file=${dir}/vars.tfvars.json`,
      ]),
    );
    expect(runner.calls[0]!.args).toContain('-backend=false');
    expect(runner.calls[2]!.args).toEqual(['show', '-json', '-no-color', `${dir}/plan.tfplan`]);
    expect(plan.env).toMatchObject({ AWS_PROFILE: 'sandbox', TF_DATA_DIR: `${dir}/.terraform-data`, TF_IN_AUTOMATION: '1' });

    expect(JSON.parse(await readFile(join(dir, 'vars.tfvars.json'), 'utf8'))).toEqual({
      sandbox_id: 'demo',
      region: 'us-east-1',
      allowed_instance_types: ['c7i.large', 'c7i.2xlarge'],
      max_instances_per_agent: 4,
      max_instance_lifetime_hours: 2,
      tags: { owner: 'me' },
    });
    expect(r.changes).toEqual([
      { action: 'create', address: 'aws_s3_bucket.artifacts', type: 'aws_s3_bucket' },
      { action: 'update', address: 'aws_s3_bucket.sealed', type: 'aws_s3_bucket' },
      { action: 'replace', address: 'aws_iam_role.bench_runner', type: 'aws_iam_role' },
      { action: 'delete', address: 'aws_security_group.bench', type: 'aws_security_group' },
      { action: 'no-op', address: 'aws_iam_policy.agent_boundary', type: 'aws_iam_policy' },
    ]);
    expect(r.summary).toBe('Plan: 2 to add, 1 to change, 2 to destroy.');
  });

  it('exit code 0 (no changes) is fine; 1 is an error with stderr', async () => {
    runner.planExit = 0;
    await expect(tofu.plan(input)).resolves.toBeTruthy();
    runner.planExit = 1;
    const e = await tofu.plan(input).catch((x) => x);
    expect(e).toBeInstanceOf(TofuError);
    expect(e.message).toContain('tofu plan failed (exit 1)');
  });

  it('validates the sandbox id and instance types', async () => {
    await expect(tofu.plan({ ...input, sandboxId: '../evil' })).rejects.toThrow(/invalid sandbox id/);
    await expect(tofu.plan({ ...input, allowedInstanceTypes: [] })).rejects.toThrow(/must not be empty/);
    expect(runner.calls).toHaveLength(0);
  });

  it.each(['*', '?', '/', '..', 'a/b', 'a*', 'a?', '../x', 'a', 'Demo', 'x'.repeat(33)])('rejects sandbox id %j at every provider entry point', async (bad) => {
    const i = { ...input, sandboxId: bad };
    await expect(tofu.plan(i)).rejects.toThrow(/invalid sandbox id/);
    await expect(tofu.apply(i)).rejects.toThrow(/invalid sandbox id/);
    await expect(tofu.destroy(i)).rejects.toThrow(/invalid sandbox id/);
    await expect(tofu.outputs(bad)).rejects.toThrow(/invalid sandbox id/);
    expect(() => tofu.workDir(bad)).toThrow(/invalid sandbox id/);
    await expect(new MockSandboxProvider().plan(i)).rejects.toThrow(/invalid sandbox id/);
    await expect(new MockSandboxProvider().apply(i)).rejects.toThrow(/invalid sandbox id/);
    expect(runner.calls).toHaveLength(0);
  });

  it('does not put anything secret-shaped in the vars file', async () => {
    await tofu.plan(input);
    expect(await readFile(join(root, 'demo', 'vars.tfvars.json'), 'utf8')).not.toMatch(/key|token|secret/i);
  });
});

describe('apply', () => {
  it('plans, applies the saved plan, and maps outputs per the contract', async () => {
    const out = await tofu.apply(input);
    expect(runner.subs()).toEqual(['init', 'plan', 'show', 'apply', 'output']);
    const apply = runner.calls[3]!;
    expect(apply.args[apply.args.length - 1]).toBe(join(root, 'demo', 'plan.tfplan'));
    expect(apply.args).not.toContain('-auto-approve'); // saved plan only
    expect(out).toMatchObject({
      artifactsBucket: 'ratchet-demo-artifacts',
      sealedBucket: 'ratchet-demo-sealed',
      agentBoundaryRef: 'arn:aws:iam::1:policy/ratchet/ratchet-demo-agent-boundary',
      evalIdentityRef: 'arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-bench-runner',
      agentRolePath: '/ratchet/agent/',
      network: { subnetId: 'subnet-1', benchSubnetId: 'subnet-bench', agentRunnerSecurityGroupId: 'sg-r', benchSecurityGroupId: 'sg-b', egressProxyUrl: null },
      sealedConfirmPrefix: 'holdout/',
      region: 'us-east-1',
      vpcId: 'vpc-1',
      limits: { maxInstancesPerAgent: 4, maxInstanceLifetimeHours: 2 },
    });
    expect(out.evalIdentity.roleName).toBe('ratchet-demo-bench-runner');
    // two identities: eval_identity_ref has NO sealed access, confirm_identity_ref does
    expect(out.confirmIdentity.instanceProfileArn).toBe('arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-confirm-runner');
    expect(out.confirmIdentity.roleArn).not.toBe(out.evalIdentity.roleArn);
    // ... and a third for dependency snapshots: it writes bench-deps/ and has no sealed access
    expect(out.depsIdentity?.instanceProfileArn).toBe('arn:aws:iam::1:instance-profile/ratchet/bench/ratchet-demo-deps-runner');
    expect(new Set([out.evalIdentity.roleArn, out.confirmIdentity.roleArn, out.depsIdentity?.roleArn]).size).toBe(3);
  });

  it('parses outputs of a sandbox applied before dependency snapshots (no deps_identity_ref): the identity is simply absent', () => {
    const doc = JSON.parse(OUTPUTS_JSON) as Record<string, unknown>;
    delete doc.deps_identity_ref;
    const out = parseTofuOutputs(JSON.stringify(doc), 'us-east-1')!;
    expect(out.depsIdentity).toBeUndefined();
    expect(out.confirmIdentity.roleName).toBe('ratchet-demo-confirm-runner');
  });

  it('stops after a failed apply', async () => {
    runner.failOn = 'apply';
    await expect(tofu.apply(input)).rejects.toThrow(/tofu apply failed/);
    expect(runner.subs()).not.toContain('output');
  });

  it('serializes concurrent operations on the same sandbox', async () => {
    await Promise.all([tofu.plan(input), tofu.plan(input), tofu.apply(input)]);
    expect(runner.maxInflight).toBe(1);
  });

  it('does not block different sandboxes on each other', async () => {
    await Promise.all([tofu.plan(input), tofu.plan({ ...input, sandboxId: 'other' })]);
    expect(runner.maxInflight).toBe(2);
  });
});

describe('destroy', () => {
  it('refuses while runtime agent roles exist; runs nothing', async () => {
    roles = ['ratchet-agent-opt1-a1'];
    const e = await tofu.destroy(input).catch((x) => x);
    expect(e).toBeInstanceOf(TeardownBlockedError);
    expect(e.roles).toEqual(['ratchet-agent-opt1-a1']);
    expect(e.message).toContain('kill switch');
    expect(runner.calls).toHaveLength(0);
  });

  it('destroys once the roles are gone', async () => {
    await tofu.destroy(input);
    expect(runner.subs()).toEqual(['init', 'destroy']);
    expect(runner.calls[1]!.args).toEqual(
      expect.arrayContaining(['-auto-approve', `-state=${join(root, 'demo')}/terraform.tfstate`, `-var-file=${join(root, 'demo')}/vars.tfvars.json`]),
    );
  });

  it('surfaces tofu failures', async () => {
    runner.failOn = 'destroy';
    await expect(tofu.destroy(input)).rejects.toBeInstanceOf(TofuError);
  });
});

describe('outputs', () => {
  it('is null for an unknown sandbox without spawning tofu', async () => {
    expect(await tofu.outputs('nope')).toBeNull();
    expect(runner.calls).toHaveLength(0);
  });

  it('reads outputs (and region) for an applied sandbox', async () => {
    await tofu.apply(input);
    runner.calls = [];
    await mkdir(join(root, 'demo'), { recursive: true });
    await writeFile(join(root, 'demo', 'terraform.tfstate'), '{}');
    const out = await tofu.outputs('demo');
    expect(out?.region).toBe('us-east-1');
    expect(out?.artifactsBucket).toBe('ratchet-demo-artifacts');
    expect(runner.subs()).toEqual(['output']);
  });
});

describe('pure parsers', () => {
  it('mapActions', () => {
    expect(mapActions(['create', 'delete'])).toBe('replace');
    expect(mapActions(['read'])).toBe('no-op');
    expect(mapActions(['update'])).toBe('update');
  });
  it('summarizePlan tolerates an empty plan', () => {
    expect(summarizePlan('{}')).toEqual({ changes: [], summary: 'Plan: 0 to add, 0 to change, 0 to destroy.' });
  });
  it('parseTofuOutputs: empty -> null, missing output -> clear error', () => {
    expect(parseTofuOutputs('{}')).toBeNull();
    expect(() => parseTofuOutputs(JSON.stringify({ artifacts_bucket: { value: 'x' } }))).toThrow(/contract/);
  });
});
