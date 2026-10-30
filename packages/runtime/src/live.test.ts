import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { openLedger, type Ledger, type SandboxInput } from '@ratchet/core';
import type { AwsSandboxOutputs, ProcessResult, ProcessRunner, ProcessSpec } from '@ratchet/aws';
import { FakeSpawner, type HarnessInfo } from '@ratchet/agents';
import { createHybridRuntime, createLiveRuntime, createLiveRuntimeFor, resolveLiveImages, type HybridOptions, type LiveOptions, type LiveRuntime } from './live.js';

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

describe('createHybridRuntime (agents on this machine, evaluator in the cloud)', () => {
  const ok = (kind: HarnessInfo['kind'], over: Partial<HarnessInfo> = {}): HarnessInfo => ({ kind, installed: true, version: '1.0.0', path: `/usr/local/bin/${kind === 'codex' ? 'codex' : 'claude'}`, auth: 'signed-in', hint: '', ...over });
  const hybrid = async (ledger: Ledger, over: Partial<HybridOptions> = {}) => {
    const stateRoot = mkdtempSync(join(tmpdir(), 'ratchet-hybrid-'));
    roots.push(stateRoot);
    const rt = await createHybridRuntime({ ledger, region: 'us-east-1', sandbox: null, sandboxId: 'sbx', tofuBinary: '/opt/tofu', moduleDir: '/repo/infra/aws', stateRoot, tofuRunner: new FakeTofu(), harnesses: [ok('claude-code'), ok('codex')], loginPath: '/usr/local/bin:/usr/bin', ...over });
    runtimes.push(rt);
    return { rt, stateRoot };
  };

  it('defaults to local agents and an invisible sandbox: no apply step, no runner AMI needed', async () => {
    const { rt } = await hybrid(openLedger(':memory:'));
    expect(rt.agentsWhere).toBe('local');
    expect(rt.localRunners).not.toBeNull();
    expect(rt.harnesses.map((h) => h.kind)).toEqual(['claude-code', 'codex']);
    expect(rt.missing.join(' ')).not.toMatch(/apply the sandbox|runner AMI/);
    expect(rt.missing).toEqual([expect.stringMatching(/GitHub/), expect.stringMatching(/bench AMI/)]);
  });

  it('is ready once GitHub and the bench image are set', async () => {
    const { rt } = await hybrid(openLedger(':memory:'), { githubRepo: 'acme/api', githubToken: 'ghp_FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE12', benchImageRef: 'ami-0fedcba9876543210' });
    expect(rt.missing).toEqual([]);
    expect(rt.repoUrl).toBe('https://github.com/acme/api.git');
  });

  it('without an applied sandbox a draft can be created: the sandbox is made at the first start, so only GitHub is missing', async () => {
    const { rt } = await hybrid(openLedger(':memory:'));
    await expect(rt.engine.createOptimization({ repoUrl: 'x', baseRef: 'main', agents: ['codex'], budgetUsd: 1, timeLimitSeconds: 60, plateauEvals: 1 })).rejects.toMatchObject({
      code: 'not_configured',
      message: expect.stringMatching(/GitHub is not ready/), // not `sandbox_not_applied`
    });
  });

  it('runs the detection probes when no harnesses are given, and says what is missing when none is usable', async () => {
    const root = mkdtempSync(join(tmpdir(), 'ratchet-hybrid-bin-'));
    roots.push(root);
    const spawner = new FakeSpawner(() => ({ exitCode: 127, stderr: 'not found' })).spawner;
    const { rt } = await hybrid(openLedger(':memory:'), { harnesses: undefined, loginPath: undefined, detect: { spawner, env: { PATH: root, HOME: root, SHELL: '/bin/sh' }, home: root, platform: 'linux', timeoutMs: 200, shellTimeoutMs: 200 } });
    expect(rt.harnesses.map((h) => [h.kind, h.installed])).toEqual([['claude-code', false], ['codex', false]]);
    expect(rt.missing.join(' ')).toMatch(/install and sign in to Claude Code or Codex/);
  });

  it('a signed-out harness does not count as usable; a signed-in one does', async () => {
    const out = await hybrid(openLedger(':memory:'), { harnesses: [ok('claude-code', { auth: 'signed-out' })] });
    expect(out.rt.missing.join(' ')).toMatch(/sign in to Claude Code or Codex/);
    const usable = await hybrid(openLedger(':memory:'), { harnesses: [ok('claude-code', { auth: 'signed-out' }), ok('codex', { auth: 'api-key' })] });
    expect(usable.rt.missing.join(' ')).not.toMatch(/sign in to Claude Code/);
  });

  it('agentsWhere cloud keeps the SSM path: the runner AMI is required again', async () => {
    const { rt } = await hybrid(openLedger(':memory:'), { agentsWhere: 'cloud' });
    expect(rt.agentsWhere).toBe('cloud');
    expect(rt.missing.join(' ')).toMatch(/runner AMI/);
  });

  it('createLiveRuntime without localAgents is unchanged (cloud, no local control) and refuses agentsWhere local without them', () => {
    const ledger = openLedger(':memory:');
    const { rt } = build(ledger);
    expect(rt.agentsWhere).toBe('cloud');
    expect(rt.localRunners).toBeNull();
    expect(rt.harnesses).toEqual([]);
    const stateRoot = mkdtempSync(join(tmpdir(), 'ratchet-live-'));
    roots.push(stateRoot);
    expect(() => createLiveRuntime({ ledger, region: 'us-east-1', sandbox: null, sandboxId: 'sbx', tofuBinary: 't', moduleDir: 'm', stateRoot, agentsWhere: 'local' })).toThrow(/no localAgents/);
  });
});

describe('images found by tag, region from the repo', () => {
  let calls = 0;
  const fake = (images: unknown[]) => ({ send: async () => (calls++, { Images: images }) }) as never;
  const base = { ledger: undefined as unknown as Ledger, region: 'us-east-1', sandbox: null, sandboxId: 'sbx', tofuBinary: '/opt/tofu', moduleDir: '/m', stateRoot: '/tmp/x' };
  const tagged = (id: string, kind: string, v: string) => ({ ImageId: id, State: 'available', Architecture: 'x86_64', CreationDate: '2026-10-20T10:00:00.000Z', Tags: [{ Key: 'ratchet:image', Value: kind }, { Key: 'ratchet:image_version', Value: v }] });

  it('resolveLiveImages fills both AMIs from the tags for the app version; an explicit ref wins', async () => {
    const ec2 = fake([tagged('ami-0000000000000001', 'runner', '0.4.0-1'), tagged('ami-0000000000000002', 'bench', '0.4.0-1')]);
    const r = await resolveLiveImages({ ...base, appVersion: '0.4.0' }, { ec2 });
    expect([r.runnerImageRef, r.benchImageRef]).toEqual(['ami-0000000000000001', 'ami-0000000000000002']);
    const o = await resolveLiveImages({ ...base, appVersion: '0.4.0', benchImageRef: 'ami-0fedcba9876543210' }, { ec2 });
    expect([o.runnerImageRef, o.benchImageRef]).toEqual(['ami-0000000000000001', 'ami-0fedcba9876543210']);
    const none = await resolveLiveImages({ ...base });
    expect(none.runnerImageRef).toBeUndefined(); // no app version: nothing looked up
  });

  it('createLiveRuntimeFor uses environment.region of the repo, then the setting; the lookup is skipped before a sandbox exists', async () => {
    calls = 0;
    const ledger = openLedger(':memory:');
    const stateRoot = mkdtempSync(join(tmpdir(), 'ratchet-live-'));
    roots.push(stateRoot);
    const rt = await createLiveRuntimeFor({ ...base, ledger, stateRoot, tofuRunner: new FakeTofu(), appVersion: '0.4.0', region: 'us-west-2' }, { environment: { region: 'eu-west-1' } });
    runtimes.push(rt);
    expect(calls).toBe(0);
    await expect(createLiveRuntimeFor({ ...base, ledger, stateRoot }, { environment: { region: 'mars-1' } })).rejects.toThrow(/not an AWS region name/);
  });
});
