import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { SandboxEnvironment, SandboxInput } from '@ratchet/core';
import { hashEnvironmentFiles } from '@ratchet/core';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  BASE_MODULE_DIRNAME,
  EnvironmentRejectedError,
  GENERATED_INPUTS_FILE,
  GENERATED_OUTPUTS_FILE,
  assertDraftedEnvironment,
  environmentVars,
  materializeCustomEnvironment,
  materializeDraftedEnvironment,
  parseEnvironmentOutputs,
} from './environment.js';
import { TofuEnvironmentChecker } from './environment-check.js';
import { MockSandboxProvider } from './mock.js';
import { InfraOverrideError } from './override.js';
import type { ProcessResult, ProcessRunner, ProcessSpec } from './process.js';
import { TeardownBlockedError, TofuError, TofuSandboxProvider, parseTofuOutputs, type AwsSandboxOutputs } from './tofu.js';

const BASE_MODULE_DIR = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..', 'infra', 'modules', 'sandbox');
const MODULE_DIR = join(BASE_MODULE_DIR, '..', '..', 'aws');

const MAIN_TF = `module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "the service"
      instance_type = "c7i.2xlarge"
      user_data     = file("\${path.module}/api.sh")
    }
    db = {
      role           = "database"
      label          = "Postgres"
      instance_type  = "r7g.large"
      data_volume_gb = 20
    }
  }
}
`;

const env = (over: Partial<SandboxEnvironment> = {}, files = [{ path: 'main.tf', content: MAIN_TF }, { path: 'api.sh', content: '#!/bin/bash\n' }]): SandboxEnvironment => ({
  kind: 'drafted',
  scope: 'full',
  path: 'infra/ratchet',
  commit: 'abc123',
  hash: hashEnvironmentFiles(files),
  files,
  ...over,
});

let root: string;
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'ratchet-env-'));
});
afterEach(() => rm(root, { recursive: true, force: true }));

describe('materializeDraftedEnvironment', () => {
  it('writes the draft, a copy of the base module and the two generated files', async () => {
    const dest = await materializeDraftedEnvironment({ env: env(), baseModuleDir: BASE_MODULE_DIR, destDir: join(root, 'env') });
    const top = (await readdir(dest)).sort();
    expect(top).toEqual(['api.sh', 'main.tf', GENERATED_INPUTS_FILE, GENERATED_OUTPUTS_FILE, BASE_MODULE_DIRNAME].sort());
    expect((await readdir(join(dest, BASE_MODULE_DIRNAME))).sort()).toEqual(['main.tf', 'outputs.tf', 'variables.tf', 'versions.tf']);
    expect(await readFile(join(dest, 'main.tf'), 'utf8')).toBe(MAIN_TF);
    // the copy is the shipped module, byte for byte (the draft cannot supply its own)
    expect(await readFile(join(dest, BASE_MODULE_DIRNAME, 'main.tf'), 'utf8')).toBe(await readFile(join(BASE_MODULE_DIR, 'main.tf'), 'utf8'));
    const inputs = await readFile(join(dest, GENERATED_INPUTS_FILE), 'utf8');
    expect(inputs).toContain('variable "ratchet"');
    expect(inputs).toContain('provider "aws"');
    expect(inputs).toContain('region = var.ratchet.region');
    expect(await readFile(join(dest, GENERATED_OUTPUTS_FILE), 'utf8')).toContain('module.sandbox.environment');
  });

  it('starts from an empty directory every time (stale files of an earlier draft never survive)', async () => {
    const dest = join(root, 'env');
    await materializeDraftedEnvironment({ env: env(), baseModuleDir: BASE_MODULE_DIR, destDir: dest });
    const files = [{ path: 'main.tf', content: MAIN_TF }];
    await materializeDraftedEnvironment({ env: env({}, files), baseModuleDir: BASE_MODULE_DIR, destDir: dest }).catch(() => undefined);
    await materializeDraftedEnvironment({ env: env({}, [{ path: 'main.tf', content: MAIN_TF.replace('file("${path.module}/api.sh")', '""') }]), baseModuleDir: BASE_MODULE_DIR, destDir: dest });
    expect(await readdir(dest)).not.toContain('api.sh');
  });

  it('refuses a draft that fails the validator, before writing anything', async () => {
    const bad = env({}, [{ path: 'main.tf', content: `${MAIN_TF}\nresource "aws_internet_gateway" "igw" {\n  tags = module.sandbox.tags\n}\n` }]);
    const e = await materializeDraftedEnvironment({ env: bad, baseModuleDir: BASE_MODULE_DIR, destDir: join(root, 'env') }).catch((x) => x);
    expect(e).toBeInstanceOf(EnvironmentRejectedError);
    expect((e as EnvironmentRejectedError).problems.join()).toMatch(/Isolated: no internet on the measuring hosts/);
    await expect(stat(join(root, 'env'))).rejects.toThrow();
  });

  it('refuses a draft that does not use the base module, and unsafe paths', async () => {
    expect(() => assertDraftedEnvironment(env({}, [{ path: 'main.tf', content: 'locals {\n a = 1\n}\n' }]))).toThrow(/missing the base module/);
    const evil = env({}, [{ path: '../escape.tf', content: MAIN_TF }]);
    await expect(materializeDraftedEnvironment({ env: evil, baseModuleDir: BASE_MODULE_DIR, destDir: join(root, 'e2') })).rejects.toThrow(EnvironmentRejectedError);
  });

  it('refuses when the base module is not where it should be', async () => {
    await expect(materializeDraftedEnvironment({ env: env(), baseModuleDir: join(root, 'missing'), destDir: join(root, 'e3') })).rejects.toThrow();
    await expect(materializeDraftedEnvironment({ env: env(), baseModuleDir: root, destDir: join(root, 'e4') })).rejects.toThrow(/base module not found/);
  });
});

describe('materializeCustomEnvironment (infra: in ratchet.yaml)', () => {
  const custom = (files: Array<{ path: string; content: string }>): SandboxEnvironment => ({ kind: 'custom', scope: 'full', path: 'ops/sandbox', commit: 'c', hash: 'h', files });

  it('writes the module and nothing else', async () => {
    const dest = await materializeCustomEnvironment({ env: custom([{ path: 'main.tf', content: 'resource "aws_s3_bucket" "a" {}\n' }, { path: 'sub/x.tf', content: '# x\n' }]), destDir: join(root, 'c') });
    expect((await readdir(dest)).sort()).toEqual(['main.tf', 'sub']);
  });

  it('refuses a module without .tf files or that can run commands here', async () => {
    await expect(materializeCustomEnvironment({ env: custom([{ path: 'README', content: 'x' }]), destDir: join(root, 'c1') })).rejects.toThrow(InfraOverrideError);
    await expect(materializeCustomEnvironment({ env: custom([{ path: 'main.tf', content: 'resource "null_resource" "x" {\n provisioner "local-exec" { command = "id" }\n}' }]), destDir: join(root, 'c2') })).rejects.toThrow(/can run commands on this machine/);
  });
});

describe('environmentVars / parseEnvironmentOutputs', () => {
  const out = parseTofuOutputs(
    JSON.stringify({
      artifacts_bucket: { value: 'ratchet-demo-artifacts' },
      sealed_bucket: { value: 'ratchet-demo-sealed' },
      agent_boundary_ref: { value: 'arn:aws:iam::1:policy/b' },
      eval_identity_ref: { value: { role_arn: 'arn:aws:iam::1:role/e', role_name: 'e', instance_profile_arn: 'arn:aws:iam::1:instance-profile/e', instance_profile_name: 'ratchet-demo-bench-runner' } },
      confirm_identity_ref: { value: { role_arn: 'arn:aws:iam::1:role/c', role_name: 'c', instance_profile_arn: 'arn:aws:iam::1:instance-profile/c', instance_profile_name: 'c' } },
      sealed_confirm_prefix: { value: 'holdout/' },
      network_ref: { value: { vpc_id: 'vpc-1', subnet_id: 'subnet-a', bench_subnet_id: 'subnet-b', security_group_ids: { agent_runner: 'sg-a', bench: 'sg-b' }, egress_proxy_url: null } },
      agent_role_path: { value: '/ratchet/agent/' },
      limits: { value: { max_instances_per_agent: 2, max_instance_lifetime_hours: 4 } },
    }),
    'us-east-1',
  )!;

  it('feeds the base module from the sandbox contract outputs: the bench subnet and group, the evaluation-only profile', () => {
    const v = environmentVars(out, env(), { sandboxId: 'ratchet-env-1234abcd', tags: { owner: 'me' }, imageVersion: '0.4.0' });
    expect(v.ratchet).toEqual({
      sandbox_id: 'ratchet-env-1234abcd',
      region: 'us-east-1',
      vpc_id: 'vpc-1',
      bench_subnet_id: 'subnet-b',
      bench_security_group_id: 'sg-b',
      artifacts_bucket: 'ratchet-demo-artifacts',
      eval_instance_profile_name: 'ratchet-demo-bench-runner',
      environment_hash: env().hash,
      machine_image_id: '',
      image_version: '0.4.0',
      tags: { owner: 'me' },
    });
    expect(JSON.stringify(v)).not.toMatch(/confirm|sealed|agent-runner|subnet-a|sg-a/); // never the agent side, never the holdout identity
  });

  it('reads the machines and the created resources (data sources excluded)', () => {
    const r = parseEnvironmentOutputs(
      env(),
      JSON.stringify({ environment: { value: { hash: 'h', machines: { api: { id: 'i-1', role: 'app', private_ip: '10.0.1.5' }, db: { id: 'i-2', role: 'database', private_ip: '10.0.1.6' } } } } }),
      'module.sandbox.aws_instance.machine["api"]\nmodule.sandbox.data.aws_ami.bench[0]\nmodule.sandbox.aws_instance.machine["db"]\n\n',
    );
    expect(r).toEqual({
      kind: 'drafted',
      scope: 'full',
      hash: env().hash,
      resources: ['module.sandbox.aws_instance.machine["api"]', 'module.sandbox.aws_instance.machine["db"]'],
      machines: { api: { id: 'i-1', role: 'app', privateIp: '10.0.1.5' }, db: { id: 'i-2', role: 'database', privateIp: '10.0.1.6' } },
    });
  });
});

// ---- the provider -------------------------------------------------------------------------------------------------------------

const SBX_OUT = {
  artifacts_bucket: { value: 'ratchet-envsbx-artifacts' },
  sealed_bucket: { value: 'ratchet-envsbx-sealed' },
  agent_boundary_ref: { value: 'arn:aws:iam::123456789012:policy/ratchet/b' },
  eval_identity_ref: { value: { role_arn: 'arn:aws:iam::123456789012:role/ratchet/e', role_name: 'e', instance_profile_arn: 'arn:aws:iam::123456789012:instance-profile/ratchet/e', instance_profile_name: 'e' } },
  confirm_identity_ref: { value: { role_arn: 'arn:aws:iam::123456789012:role/ratchet/c', role_name: 'c', instance_profile_arn: 'arn:aws:iam::123456789012:instance-profile/ratchet/c', instance_profile_name: 'c' } },
  deps_identity_ref: { value: { role_arn: 'arn:aws:iam::123456789012:role/ratchet/d', role_name: 'd', instance_profile_arn: 'arn:aws:iam::123456789012:instance-profile/ratchet/d', instance_profile_name: 'd' } },
  sealed_confirm_prefix: { value: 'holdout/' },
  network_ref: { value: { vpc_id: 'vpc-0abc', subnet_id: 'subnet-0a', bench_subnet_id: 'subnet-0b', security_group_ids: { agent_runner: 'sg-0a', bench: 'sg-0b' }, egress_proxy_url: null } },
  agent_role_path: { value: '/ratchet/agent/' },
  limits: { value: { max_instances_per_agent: 2, max_instance_lifetime_hours: 4 } },
};
const ENV_OUT = { environment: { value: { hash: 'h', machines: { api: { id: 'i-0api', role: 'app', private_ip: '10.42.1.10' }, db: { id: 'i-0db', role: 'database', private_ip: '10.42.1.11' } } } } };

class Runner implements ProcessRunner {
  constructor(private readonly stateRoot: string) {}
  calls: ProcessSpec[] = [];
  sandboxOutputs: unknown = SBX_OUT;
  failEnvOn: string | undefined;
  async run(spec: ProcessSpec): Promise<ProcessResult> {
    this.calls.push(spec);
    const sub = spec.args[0]!;
    const inEnv = spec.cwd.endsWith('/env');
    if (inEnv && this.failEnvOn === sub) return { exitCode: 1, stdout: '', stderr: 'Error: nope' };
    if (sub === 'apply') await writeFile(join(this.stateRoot, 'envsbx', inEnv ? 'env.tfstate' : 'terraform.tfstate'), '{}'); // what tofu does
    if (sub === 'plan') return { exitCode: 2, stdout: '', stderr: '' };
    if (sub === 'show') return { exitCode: 0, stdout: JSON.stringify({ resource_changes: [{ address: 'aws_vpc.sandbox', mode: 'managed', type: 'aws_vpc', change: { actions: ['create'] } }] }), stderr: '' };
    if (sub === 'output') return { exitCode: 0, stdout: JSON.stringify(inEnv ? ENV_OUT : this.sandboxOutputs), stderr: '' };
    if (sub === 'state') return { exitCode: 0, stdout: 'module.sandbox.aws_instance.machine["api"]\nmodule.sandbox.aws_instance.machine["db"]\nmodule.sandbox.aws_ebs_volume.data["db"]\n', stderr: '' };
    return { exitCode: 0, stdout: '', stderr: '' };
  }
  /** `<where>:<subcommand>` per call, `env` for the drafted stage, `sbx` for the standard module, `custom` for a custom module. */
  trace(): string[] {
    return this.calls.map((c) => `${c.cwd.endsWith('/env') ? 'env' : c.cwd.endsWith('/custom-module') ? 'custom' : c.cwd === MODULE_DIR ? 'sbx' : c.cwd}:${c.args[0]}`);
  }
}

const input = (environment?: SandboxEnvironment): SandboxInput => ({
  sandboxId: 'envsbx',
  region: 'us-east-1',
  allowedInstanceTypes: ['c7i.large'],
  maxInstancesPerAgent: 2,
  maxInstanceLifetimeHours: 4,
  tags: { 'ratchet:managed': 'true' },
  ...(environment ? { environment } : {}),
});

describe('TofuSandboxProvider with a drafted environment (two stages)', () => {
  let runner: Runner;
  let provider: TofuSandboxProvider;
  beforeEach(() => {
    runner = new Runner(root);
    provider = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner, listRuntimeAgentRoles: async () => [], imageVersion: '0.4.0', env: { AWS_PROFILE: 'p' } });
  });

  it('applies the standard sandbox first, then the environment on top of it, and reports what it created', async () => {
    const out = await provider.apply(input(env()));
    expect(runner.trace()).toEqual(['sbx:init', 'sbx:plan', 'sbx:show', 'sbx:apply', 'sbx:output', 'env:init', 'env:plan', 'env:apply', 'env:output', 'env:state']);
    expect(out.environment).toEqual({
      kind: 'drafted',
      scope: 'full',
      hash: env().hash,
      resources: ['module.sandbox.aws_instance.machine["api"]', 'module.sandbox.aws_instance.machine["db"]', 'module.sandbox.aws_ebs_volume.data["db"]'],
      machines: { api: { id: 'i-0api', role: 'app', privateIp: '10.42.1.10' }, db: { id: 'i-0db', role: 'database', privateIp: '10.42.1.11' } },
    });
    // the second stage has its own state, data dir and variables; the standard sandbox's files are untouched
    const dir = join(root, 'envsbx');
    const planEnv = runner.calls.find((c) => c.args[0] === 'plan' && c.cwd.endsWith('/env'))!;
    expect(planEnv.args).toEqual(expect.arrayContaining([`-state=${dir}/env.tfstate`, `-var-file=${dir}/env.tfvars.json`, '-detailed-exitcode']));
    expect(planEnv.env['TF_DATA_DIR']).toBe(`${dir}/.terraform-data-env`);
    const sbxPlan = runner.calls.find((c) => c.args[0] === 'plan' && c.cwd === MODULE_DIR)!;
    expect(sbxPlan.args).toContain(`-state=${dir}/terraform.tfstate`);
    expect(sbxPlan.env['TF_DATA_DIR']).toBe(`${dir}/.terraform-data`);
    const vars = JSON.parse(await readFile(join(dir, 'env.tfvars.json'), 'utf8')) as { ratchet: Record<string, unknown> };
    expect(vars.ratchet).toMatchObject({ bench_subnet_id: 'subnet-0b', bench_security_group_id: 'sg-0b', eval_instance_profile_name: 'e', artifacts_bucket: 'ratchet-envsbx-artifacts', image_version: '0.4.0', environment_hash: env().hash });
    // the module that was applied is the draft on top of the shipped base module
    expect(await readdir(join(dir, 'env'))).toEqual(expect.arrayContaining(['main.tf', 'api.sh', GENERATED_INPUTS_FILE, GENERATED_OUTPUTS_FILE, BASE_MODULE_DIRNAME]));
    expect(JSON.parse(await readFile(join(dir, 'env-meta.json'), 'utf8')).hash).toBe(env().hash);
  });

  it('applies the environment saved plan, never -auto-approve', async () => {
    await provider.apply(input(env()));
    const apply = runner.calls.find((c) => c.args[0] === 'apply' && c.cwd.endsWith('/env'))!;
    expect(apply.args).not.toContain('-auto-approve');
    expect(apply.args.at(-1)).toBe(join(root, 'envsbx', 'env.tfplan'));
  });

  it('refuses a draft that fails the contract BEFORE running any tofu command', async () => {
    const bad = env({}, [{ path: 'main.tf', content: `${MAIN_TF}\nresource "aws_iam_role" "r" {\n  tags = module.sandbox.tags\n}\n` }]);
    await expect(provider.apply(input(bad))).rejects.toThrow(EnvironmentRejectedError);
    await expect(provider.plan(input(bad))).rejects.toThrow(/Results sealed/);
    expect(runner.calls).toEqual([]);
  });

  it('a failed environment stage leaves the sandbox applied (the lifecycle marks it failed) and destroy still removes both stages', async () => {
    runner.failEnvOn = 'apply';
    await expect(provider.apply(input(env()))).rejects.toThrow(TofuError);
    runner.failEnvOn = undefined;
    // the stage wrote its state before failing: simulate that
    await writeFile(join(root, 'envsbx', 'env.tfstate'), '{}');
    runner.calls = [];
    await provider.destroy(input());
    expect(runner.trace()).toEqual(['env:init', 'env:destroy', 'sbx:init', 'sbx:destroy']);
  });

  it('destroy removes the environment first, then the sandbox, from the files that were applied', async () => {
    await provider.apply(input(env()));
    runner.calls = [];
    await provider.destroy(input()); // the caller passes no environment: the stored copy is used
    expect(runner.trace()).toEqual(['env:init', 'env:destroy', 'sbx:init', 'sbx:destroy']);
    const d = runner.calls.find((c) => c.args[0] === 'destroy' && c.cwd.endsWith('/env'))!;
    expect(d.args).toEqual(expect.arrayContaining(['-auto-approve', `-state=${join(root, 'envsbx', 'env.tfstate')}`, `-var-file=${join(root, 'envsbx', 'env.tfvars.json')}`]));
    await expect(stat(join(root, 'envsbx', 'env.tfstate'))).rejects.toThrow(); // gone, so a second destroy skips the stage
  });

  it('keeps the runtime-roles guard: nothing is destroyed while agent roles remain', async () => {
    await provider.apply(input(env()));
    runner.calls = [];
    const guarded = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner, listRuntimeAgentRoles: async () => ['role-1'] });
    await expect(guarded.destroy(input())).rejects.toThrow(TeardownBlockedError);
    expect(runner.calls).toEqual([]);
  });

  it('outputs() of an applied environment sandbox includes the environment', async () => {
    await provider.apply(input(env()));
    const o = await provider.outputs('envsbx');
    expect(o?.environment?.machines['api']).toEqual({ id: 'i-0api', role: 'app', privateIp: '10.42.1.10' });
    expect(o?.artifactsBucket).toBe('ratchet-envsbx-artifacts');
  });

  it('plan lists the environment machines next to the sandbox changes (before the sandbox exists the stage cannot be planned for real)', async () => {
    const p = await provider.plan(input(env()));
    expect(runner.trace()).toEqual(['sbx:init', 'sbx:plan', 'sbx:show']);
    expect(p.changes.map((c) => c.address)).toEqual(['aws_vpc.sandbox', 'module.sandbox.aws_instance.machine["api"]', 'module.sandbox.aws_instance.machine["db"]', 'module.sandbox.aws_ebs_volume.data["db"]']);
    expect(p.summary).toMatch(/includes the environment: 2 machines/);
  });

  it('without an environment nothing changes: the standard module only', async () => {
    const out = await provider.apply(input());
    expect(runner.trace()).toEqual(['sbx:init', 'sbx:plan', 'sbx:show', 'sbx:apply', 'sbx:output']);
    expect(out.environment).toBeUndefined();
  });
});

describe('TofuSandboxProvider with a custom module (infra:)', () => {
  const custom = (files: Array<{ path: string; content: string }>): SandboxEnvironment => ({ kind: 'custom', scope: 'full', path: 'ops/sandbox', commit: 'c', hash: 'h', files });
  const files = [{ path: 'main.tf', content: 'resource "aws_s3_bucket" "a" {}\n' }];

  it('runs tofu in the materialized custom module instead of infra/aws, and validates its outputs against the contract', async () => {
    const runner = new Runner(root);
    const provider = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner, listRuntimeAgentRoles: async () => [] });
    await provider.apply(input(custom(files)));
    expect(new Set(runner.trace().map((t) => t.split(':')[0]))).toEqual(new Set(['custom']));
    expect(runner.calls[0]!.cwd).toBe(join(root, 'envsbx', 'custom-module'));
    expect(await readFile(join(root, 'envsbx', 'custom-module', 'main.tf'), 'utf8')).toContain('aws_s3_bucket');
  });

  it('fails apply with the list of contract problems even though validateContract was not set (a custom module is always checked)', async () => {
    const runner = new Runner(root);
    const { confirm_identity_ref: _c, ...rest } = SBX_OUT;
    runner.sandboxOutputs = rest;
    const provider = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner, listRuntimeAgentRoles: async () => [] });
    await expect(provider.apply(input(custom(files)))).rejects.toThrow(/does not satisfy the sandbox contract[\s\S]*confirm_identity_ref/);
    // ... while the module that ships with the app is not re-checked on every apply
    const shipped = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner: new Runner(root), listRuntimeAgentRoles: async () => [] });
    await expect(shipped.apply({ ...input(), sandboxId: 'plain' })).resolves.toBeDefined();
  });

  it('destroys and reads outputs from the same custom module', async () => {
    const runner = new Runner(root);
    const provider = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner, listRuntimeAgentRoles: async () => [] });
    await provider.apply(input(custom(files)));
    runner.calls = [];
    await provider.destroy(input());
    expect(runner.trace()).toEqual(['custom:init', 'custom:destroy']);
  });

  it('refuses a module that can run commands before running tofu', async () => {
    const runner = new Runner(root);
    const provider = new TofuSandboxProvider({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, stateRoot: root, runner, listRuntimeAgentRoles: async () => [] });
    await expect(provider.apply(input(custom([{ path: 'main.tf', content: 'data "external" "x" { program = ["sh"] }' }])))).rejects.toThrow(/can run commands/);
    expect(runner.calls).toEqual([]);
  });
});

describe('MockSandboxProvider with a drafted environment', () => {
  it('applies it like the real provider (same refusal), reports the machines and plans them', async () => {
    const mock = new MockSandboxProvider();
    const out: AwsSandboxOutputs = await mock.apply(input(env()));
    expect(Object.keys(out.environment!.machines)).toEqual(['api', 'db']);
    expect(out.environment!.resources).toContain('module.sandbox.aws_ebs_volume.data["db"]');
    const plan = await new MockSandboxProvider().plan(input(env()));
    expect(plan.changes.some((c) => c.address === 'module.sandbox.aws_instance.machine["api"]' && c.type === 'aws_instance')).toBe(true);
    await expect(new MockSandboxProvider().apply(input(env({}, [{ path: 'main.tf', content: 'locals {}\n' }])))).rejects.toThrow(EnvironmentRejectedError);
  });
});

describe('TofuEnvironmentChecker (tofu validate + plan in the user account)', () => {
  const files = [{ path: 'infra/ratchet/main.tf', content: MAIN_TF }, { path: 'infra/ratchet/api.sh', content: '#!/bin/bash\n' }];
  const fake = (over: (spec: ProcessSpec) => Partial<ProcessResult> | undefined = () => undefined) => {
    const calls: ProcessSpec[] = [];
    const runner: ProcessRunner = {
      run: async (spec) => {
        calls.push(spec);
        return { exitCode: 0, stdout: spec.args[0] === 'plan' ? 'Plan: 2 to add, 0 to change, 0 to destroy.' : '', stderr: '', ...over(spec) };
      },
    };
    return { calls, runner };
  };
  const checker = (runner: ProcessRunner) => new TofuEnvironmentChecker({ tofuBinary: '/opt/tofu', moduleDir: MODULE_DIR, workRoot: root, runner, imageVersion: '0.4.0' });

  it('runs init, validate and a no-refresh plan in the selected profile and region', async () => {
    const { calls, runner } = fake();
    const r = await checker(runner).plan({ scope: 'full', files, region: 'eu-west-1', awsProfile: 'acme-prod' });
    expect(r).toEqual({ ok: true, detail: 'tofu validate and plan succeeded (2 resources to add) in eu-west-1' });
    expect(calls.map((c) => c.args[0])).toEqual(['init', 'validate', 'plan']);
    expect(calls[2]!.args).toEqual(expect.arrayContaining(['-refresh=false', '-lock=false', '-detailed-exitcode']));
    expect(calls[2]!.env).toMatchObject({ AWS_PROFILE: 'acme-prod', AWS_REGION: 'eu-west-1' });
    expect(calls[0]!.args).toContain('-backend=false');
    await expect(stat(calls[0]!.cwd)).rejects.toThrow(); // the scratch directory is removed
  });

  it.each(['init', 'validate', 'plan'])('reports a failing %s with the reason and does not claim a plan', async (sub) => {
    const { runner } = fake((s) => (s.args[0] === sub ? { exitCode: 1, stderr: `Error: ${sub} exploded` } : undefined));
    const r = await checker(runner).plan({ scope: 'full', files, region: 'us-east-1', awsProfile: 'p' });
    expect(r.ok).toBe(false);
    expect(r.detail).toContain(`tofu ${sub} failed`);
    expect(r.detail).toContain(`${sub} exploded`);
  });

  it('rejects a draft that violates the contract without calling tofu', async () => {
    const { calls, runner } = fake();
    const r = await checker(runner).plan({ scope: 'full', files: [{ path: 'infra/ratchet/main.tf', content: 'locals {}\n' }], region: 'us-east-1' });
    expect(r.ok).toBe(false);
    expect(r.detail).toMatch(/missing the base module/);
    expect(calls).toEqual([]);
  });
});

// Opt-in: the real OpenTofu binary validates what Ratchet materializes (downloads the AWS provider on first run).
//   RATCHET_TOFU_BIN=/path/to/tofu pnpm --filter @ratchet/aws test
const REAL_TOFU = process.env['RATCHET_TOFU_BIN'];
describe.skipIf(!REAL_TOFU || !existsSync(REAL_TOFU))('a materialized drafted environment with the real tofu', () => {
  it('passes tofu init and tofu validate (the generated provider, the variable, the base module and the draft fit together)', async () => {
    const dest = await materializeDraftedEnvironment({ env: env(), baseModuleDir: BASE_MODULE_DIR, destDir: join(root, 'real') });
    const run = (...args: string[]) => execFileSync(REAL_TOFU!, args, { cwd: dest, encoding: 'utf8', env: { ...process.env, TF_DATA_DIR: join(root, '.data'), CHECKPOINT_DISABLE: '1' } });
    run('init', '-backend=false', '-input=false', '-no-color');
    expect(run('validate', '-no-color')).toContain('valid');
  }, 240_000);
});
