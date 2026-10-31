import { mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { validateEnvironment } from '@ratchet/core';
import type {
  PlanChange,
  SandboxEnvironment,
  SandboxEnvironmentOutputs,
  SandboxInput,
  SandboxOutputs,
  SandboxProvider,
} from '@ratchet/core';
import { assertValidSandboxId } from '../util.js';
import {
  assertDraftedEnvironment,
  environmentVars,
  materializeCustomEnvironment,
  materializeDraftedEnvironment,
  parseEnvironmentOutputs,
} from './environment.js';
import { assertSandboxContract, type ContractCheckOptions } from './override.js';
import { NodeProcessRunner, type ProcessResult, type ProcessRunner } from './process.js';

export interface AwsIdentityRef {
  roleArn: string;
  roleName: string;
  instanceProfileArn: string;
  instanceProfileName: string;
}

/** Contract outputs plus the AWS-specific detail the compute layer needs. */
export interface AwsSandboxOutputs extends SandboxOutputs {
  region: string;
  vpcId: string;
  /** Bench runner for baseline/bench phases (`eval_identity_ref`): NO sealed access. */
  evalIdentity: AwsIdentityRef;
  /** Confirm runner (`confirm_identity_ref`): the only identity that reads the sealed bucket. Use only for confirm runs. */
  confirmIdentity: AwsIdentityRef;
  /**
   * Dependency-snapshot builder (`deps_identity_ref`): runs in the agent subnet, reads only the staged base tree, writes only
   * `bench-deps/`, no sealed access. Absent in sandboxes applied before dependency snapshots existed (re-apply to get it).
   */
  depsIdentity?: AwsIdentityRef;
  /** Key prefix of the confirm traces in the sealed bucket. */
  sealedConfirmPrefix: string;
  network: SandboxOutputs['network'] & {
    /** Private bench subnet: no route to the internet. */
    benchSubnetId: string;
    /** Null when the egress proxy is disabled. */
    egressProxyUrl: string | null;
  };
  limits: { maxInstancesPerAgent: number; maxInstanceLifetimeHours: number };
}

export interface TofuSandboxProviderOptions {
  /** Path to the bundled, pinned `tofu` binary. */
  tofuBinary: string;
  /** Directory holding the OpenTofu module (`infra/aws`). */
  moduleDir: string;
  /** Root for per-sandbox working dirs: `<stateRoot>/<sandboxId>/`. */
  stateRoot: string;
  /**
   * Teardown guard (infra/README.md): returns the runtime-created agent roles
   * that still exist for this sandbox. `destroy` refuses while any remain,
   * because the boundary policy cannot be deleted while roles reference it.
   */
  listRuntimeAgentRoles(input: SandboxInput): Promise<string[]>;
  runner?: ProcessRunner;
  /** Extra env for every tofu invocation (e.g. AWS_PROFILE). */
  env?: Record<string, string>;
  /**
   * Validate the module's outputs against the sandbox contract (infra/contract.md) after every apply and when outputs are read,
   * failing with a `ContractViolationError` that lists every problem. Off for the module that ships with the app (its own tests
   * pin the contract); ON for a custom module (`infra:` in ratchet.yaml, see override.ts).
   */
  validateContract?: boolean | ContractCheckOptions;
  /**
   * Directory of the base module drafted environments are built on (`infra/modules/sandbox`). Default: `../modules/sandbox`
   * next to `moduleDir`.
   */
  baseModuleDir?: string;
  /** Bench image for the machines of a drafted environment: an explicit AMI id, or (else) the app version to look it up by tag. */
  machineImageId?: string;
  imageVersion?: string;
}

export class TofuError extends Error {
  constructor(
    readonly args: string[],
    readonly result: ProcessResult,
  ) {
    super(`tofu ${args[0]} failed (exit ${result.exitCode}): ${result.stderr.trim().slice(-2000) || result.stdout.trim().slice(-500)}`);
    this.name = 'TofuError';
  }
}

export class TeardownBlockedError extends Error {
  constructor(readonly roles: string[]) {
    super(
      `refusing to destroy sandbox: ${roles.length} runtime agent role(s) still exist (${roles.join(', ')}). ` +
        'Teardown order: kill switch -> terminate tagged instances -> delete parameters -> delete agent roles/profiles -> tofu destroy.',
    );
    this.name = 'TeardownBlockedError';
  }
}

const COMMON = ['-input=false', '-no-color'];

export class TofuSandboxProvider implements SandboxProvider {
  private readonly runner: ProcessRunner;
  private readonly locks = new Map<string, Promise<unknown>>();

  constructor(private readonly opts: TofuSandboxProviderOptions) {
    this.runner = opts.runner ?? new NodeProcessRunner();
  }

  /** `<stateRoot>/<sandboxId>`: state, plan file, tfvars and TF_DATA_DIR for one sandbox. */
  workDir(sandboxId: string): string {
    assertValidSandboxId(sandboxId);
    return join(this.opts.stateRoot, sandboxId);
  }

  async plan(input: SandboxInput): Promise<{ changes: PlanChange[]; summary: string }> {
    return this.locked(input.sandboxId, async () => {
      const ctx = await this.prepare(input);
      await this.init(ctx);
      const base = await this.makePlan(ctx);
      return ctx.env?.kind === 'drafted' ? this.withEnvironmentPlan(ctx, base) : base;
    });
  }

  async apply(input: SandboxInput): Promise<AwsSandboxOutputs> {
    return this.locked(input.sandboxId, async () => {
      const ctx = await this.prepare(input);
      await this.init(ctx);
      await this.makePlan(ctx);
      await this.tofu(ctx, ['apply', ...COMMON, `-state=${ctx.statePath}`, ctx.planPath]);
      await rm(ctx.planPath, { force: true });
      const out = await this.readOutputs(ctx);
      if (!out) throw new Error('tofu apply succeeded but produced no contract outputs');
      if (ctx.env?.kind === 'drafted') out.environment = await this.applyEnvironment(ctx, out);
      return out;
    });
  }

  async destroy(input: SandboxInput): Promise<void> {
    return this.locked(input.sandboxId, async () => {
      const roles = await this.opts.listRuntimeAgentRoles(input);
      if (roles.length > 0) throw new TeardownBlockedError(roles);
      const ctx = await this.prepare(input);
      // The drafted stage goes first: it lives inside the sandbox's network and identities.
      if (await this.hasEnvironmentState(ctx.dir)) await this.destroyEnvironment(ctx);
      await this.init(ctx);
      await this.tofu(ctx, ['destroy', ...COMMON, '-auto-approve', `-state=${ctx.statePath}`, `-var-file=${ctx.varsPath}`]);
      await rm(ctx.planPath, { force: true });
    });
  }

  async outputs(sandboxId: string): Promise<AwsSandboxOutputs | null> {
    return this.locked(sandboxId, async () => {
      const dir = this.workDir(sandboxId);
      const statePath = join(dir, 'terraform.tfstate');
      try {
        await stat(statePath);
      } catch {
        return null;
      }
      const varsPath = join(dir, 'vars.tfvars.json');
      let region = '';
      try {
        region = (JSON.parse(await readFile(varsPath, 'utf8')) as { region?: string }).region ?? '';
      } catch {
        /* vars file missing: region unknown */
      }
      const ctx: Ctx = { dir, statePath, varsPath, planPath: '', input: { sandboxId, region } as SandboxInput, moduleCwd: this.opts.moduleDir };
      if (await exists(join(dir, CUSTOM_DIR))) {
        ctx.moduleCwd = join(dir, CUSTOM_DIR);
        ctx.custom = true;
      }
      const out = await this.readOutputs(ctx);
      if (out && (await this.hasEnvironmentState(dir))) {
        const meta = await this.readEnvMeta(dir);
        if (meta) out.environment = await this.readEnvironmentOutputs(ctx, meta);
      }
      return out;
    });
  }

  // ---- internals ----------------------------------------------------------

  private async prepare(input: SandboxInput): Promise<Ctx> {
    assertValidSandboxId(input.sandboxId);
    if (input.allowedInstanceTypes.length === 0) throw new Error('allowedInstanceTypes must not be empty');
    const dir = this.workDir(input.sandboxId);
    await mkdir(dir, { recursive: true });
    const varsPath = join(dir, 'vars.tfvars.json');
    const env = input.environment;
    let moduleCwd = this.opts.moduleDir;
    let custom = false;
    if (env?.kind === 'custom') {
      // A custom module replaces the standard one; it is written where only this sandbox reads it, and its outputs are checked.
      moduleCwd = await materializeCustomEnvironment({ env, destDir: join(dir, CUSTOM_DIR) });
      custom = true;
    } else if (!env && (await exists(join(dir, CUSTOM_DIR)))) {
      moduleCwd = join(dir, CUSTOM_DIR); // destroy / outputs of a sandbox that was applied with one
      custom = true;
    } else if (env?.kind === 'drafted') {
      assertDraftedEnvironment(env); // refuse before anything is created
    }
    // Contract inputs only; no secrets ever reach OpenTofu (contract invariant 11).
    await writeFile(
      varsPath,
      JSON.stringify(
        {
          sandbox_id: input.sandboxId,
          region: input.region,
          allowed_instance_types: input.allowedInstanceTypes,
          max_instances_per_agent: input.maxInstancesPerAgent,
          max_instance_lifetime_hours: input.maxInstanceLifetimeHours,
          tags: input.tags ?? {},
        },
        null,
        2,
      ),
    );
    return { input, dir, varsPath, statePath: join(dir, 'terraform.tfstate'), planPath: join(dir, 'plan.tfplan'), moduleCwd, custom, ...(env ? { env } : {}) };
  }

  // ---- the drafted environment stage (docs/SIMPLIFIED_MODEL.md, "Dynamic environments") ----------------------------------

  private envPaths(dir: string): { root: string; state: string; vars: string; meta: string; dataDir: string; plan: string } {
    return { root: join(dir, ENV_DIR), state: join(dir, 'env.tfstate'), vars: join(dir, 'env.tfvars.json'), meta: join(dir, 'env-meta.json'), dataDir: join(dir, '.terraform-data-env'), plan: join(dir, 'env.tfplan') };
  }

  private async hasEnvironmentState(dir: string): Promise<boolean> {
    return exists(this.envPaths(dir).state);
  }

  private async readEnvMeta(dir: string): Promise<SandboxEnvironment | undefined> {
    try {
      return JSON.parse(await readFile(this.envPaths(dir).meta, 'utf8')) as SandboxEnvironment;
    } catch {
      return undefined;
    }
  }

  private envTofu(ctx: Ctx, args: string[], okCodes: number[] = [0]): Promise<ProcessResult> {
    const p = this.envPaths(ctx.dir);
    return this.run(p.root, { ...this.baseEnv(), TF_DATA_DIR: p.dataDir }, args, okCodes);
  }

  /** Materialize the drafted module next to the sandbox's state, write its variables (when the sandbox outputs are known) and initialise it. */
  private async prepareEnvironment(ctx: Ctx, env: SandboxEnvironment, out: AwsSandboxOutputs | null): Promise<void> {
    const p = this.envPaths(ctx.dir);
    const baseModuleDir = this.opts.baseModuleDir ?? join(dirname(this.opts.moduleDir), 'modules', 'sandbox');
    await materializeDraftedEnvironment({ env, baseModuleDir, destDir: p.root });
    await writeFile(p.meta, JSON.stringify(env, null, 2));
    if (out) {
      const vars = environmentVars(out, env, {
        sandboxId: ctx.input.sandboxId,
        ...(ctx.input.tags ? { tags: ctx.input.tags } : {}),
        ...(this.opts.machineImageId ? { machineImageId: this.opts.machineImageId } : {}),
        ...(this.opts.imageVersion ? { imageVersion: this.opts.imageVersion } : {}),
      });
      await writeFile(p.vars, JSON.stringify(vars, null, 2));
    }
    await this.envTofu(ctx, ['init', ...COMMON, '-backend=false']);
  }

  private async applyEnvironment(ctx: Ctx, out: AwsSandboxOutputs): Promise<SandboxEnvironmentOutputs> {
    const env = ctx.env!;
    const p = this.envPaths(ctx.dir);
    await this.prepareEnvironment(ctx, env, out);
    await this.envTofu(ctx, ['plan', ...COMMON, '-detailed-exitcode', `-out=${p.plan}`, `-state=${p.state}`, `-var-file=${p.vars}`], [0, 2]);
    await this.envTofu(ctx, ['apply', ...COMMON, `-state=${p.state}`, p.plan]);
    await rm(p.plan, { force: true });
    return this.readEnvironmentOutputs(ctx, env);
  }

  private async readEnvironmentOutputs(ctx: Ctx, env: SandboxEnvironment): Promise<SandboxEnvironmentOutputs> {
    const p = this.envPaths(ctx.dir);
    const outputs = await this.envTofu(ctx, ['output', '-json', '-no-color', `-state=${p.state}`]);
    const list = await this.envTofu(ctx, ['state', 'list', `-state=${p.state}`]);
    return parseEnvironmentOutputs(env, outputs.stdout, list.stdout);
  }

  private async destroyEnvironment(ctx: Ctx): Promise<void> {
    const p = this.envPaths(ctx.dir);
    // Destroy with the files that were applied (the stored copy), whatever the caller passes now.
    const env = (await this.readEnvMeta(ctx.dir)) ?? ctx.env;
    if (!env) throw new Error('the environment stage has state but no record of its files; cannot destroy it safely');
    if (!(await exists(p.vars))) throw new Error('the environment stage has state but no variables file; re-apply the sandbox to converge it');
    await this.prepareEnvironment(ctx, env, null);
    await this.envTofu(ctx, ['destroy', ...COMMON, '-auto-approve', `-state=${p.state}`, `-var-file=${p.vars}`]);
    await rm(p.state, { force: true });
    await rm(`${p.state}.backup`, { force: true });
  }

  /**
   * `plan` for a drafted environment: the standard sandbox's plan plus the environment's machines, listed from the draft (they
   * cannot be planned for real without the sandbox's network; the apply plans the stage with the real outputs).
   */
  private withEnvironmentPlan(ctx: Ctx, base: { changes: PlanChange[]; summary: string }): { changes: PlanChange[]; summary: string } {
    const env = ctx.env!;
    const v = validateEnvironment({ scope: env.scope, moduleOnly: true, files: env.files.map((f) => ({ path: `${env.path}/${f.path}`, content: f.content })) });
    const added: PlanChange[] = v.machines.flatMap((m) => [
      { action: 'create' as const, address: `module.sandbox.aws_instance.machine["${m.key ?? m.name}"]`, type: 'aws_instance' },
      ...((m.dataVolumeGb ?? 0) > 0 ? [{ action: 'create' as const, address: `module.sandbox.aws_ebs_volume.data["${m.key ?? m.name}"]`, type: 'aws_ebs_volume' }] : []),
    ]);
    const changes = [...base.changes, ...added];
    const n = (a: PlanChange['action']) => changes.filter((c) => c.action === a).length;
    return {
      changes,
      summary: `Plan: ${n('create') + n('replace')} to add, ${n('update')} to change, ${n('delete') + n('replace')} to destroy (includes the environment: ${v.machines.length} machine${v.machines.length === 1 ? '' : 's'}).`,
    };
  }

  private async init(ctx: Ctx): Promise<void> {
    await this.tofu(ctx, ['init', ...COMMON, '-backend=false']);
  }

  private async makePlan(ctx: Ctx): Promise<{ changes: PlanChange[]; summary: string }> {
    // -detailed-exitcode: 0 = no changes, 2 = changes present; both are success.
    await this.tofu(
      ctx,
      ['plan', ...COMMON, '-detailed-exitcode', `-out=${ctx.planPath}`, `-state=${ctx.statePath}`, `-var-file=${ctx.varsPath}`],
      [0, 2],
    );
    const shown = await this.tofu(ctx, ['show', '-json', '-no-color', ctx.planPath]);
    return summarizePlan(shown.stdout);
  }

  private async readOutputs(ctx: Ctx): Promise<AwsSandboxOutputs | null> {
    const res = await this.tofu(ctx, ['output', '-json', '-no-color', `-state=${ctx.statePath}`]);
    const v = ctx.custom ? (this.opts.validateContract ?? true) : this.opts.validateContract;
    if (v && Object.keys(JSON.parse(res.stdout) as object).length > 0) assertSandboxContract(res.stdout, v === true ? {} : v);
    return parseTofuOutputs(res.stdout, (ctx.input as Partial<SandboxInput>).region);
  }

  private baseEnv(): Record<string, string> {
    return { ...this.opts.env, TF_IN_AUTOMATION: '1', TF_INPUT: '0', CHECKPOINT_DISABLE: '1' };
  }

  private async tofu(ctx: Ctx, args: string[], okCodes: number[] = [0]): Promise<ProcessResult> {
    return this.run(ctx.moduleCwd, { ...this.baseEnv(), TF_DATA_DIR: join(ctx.dir, '.terraform-data') }, args, okCodes);
  }

  private async run(cwd: string, env: Record<string, string>, args: string[], okCodes: number[]): Promise<ProcessResult> {
    const result = await this.runner.run({ command: this.opts.tofuBinary, args, cwd, env });
    if (!okCodes.includes(result.exitCode)) throw new TofuError(args, result);
    return result;
  }

  /** Serialize operations per sandbox so two tofu runs never share a state file. */
  private locked<T>(sandboxId: string, fn: () => Promise<T>): Promise<T> {
    assertValidSandboxId(sandboxId);
    const prev = this.locks.get(sandboxId) ?? Promise.resolve();
    const next = prev.then(fn, fn);
    const tail = next.catch(() => undefined);
    this.locks.set(sandboxId, tail);
    void tail.then(() => {
      if (this.locks.get(sandboxId) === tail) this.locks.delete(sandboxId);
    });
    return next;
  }
}

interface Ctx {
  input: SandboxInput;
  dir: string;
  varsPath: string;
  statePath: string;
  planPath: string;
  /** Where tofu runs for the sandbox's own stage: `infra/aws`, or the materialized custom module. */
  moduleCwd: string;
  /** The module is a custom one (`infra:`): its outputs are validated against the contract. */
  custom?: boolean;
  env?: SandboxEnvironment;
}

/** Subdirectories of `<stateRoot>/<sandboxId>` that hold a materialized environment. */
const CUSTOM_DIR = 'custom-module';
const ENV_DIR = 'env';

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

interface TfResourceChange {
  address: string;
  mode?: string;
  type: string;
  change: { actions: string[] };
}

export function mapActions(actions: string[]): PlanChange['action'] {
  const a = [...actions];
  if (a.length === 2 && a.includes('create') && a.includes('delete')) return 'replace';
  if (a.length === 1) {
    switch (a[0]) {
      case 'create':
        return 'create';
      case 'update':
        return 'update';
      case 'delete':
        return 'delete';
      default:
        return 'no-op'; // no-op, read
    }
  }
  return 'update';
}

/** `tofu show -json <plan>` -> PlanChange[] (managed resources only). */
export function summarizePlan(showJson: string): { changes: PlanChange[]; summary: string } {
  const doc = JSON.parse(showJson) as { resource_changes?: TfResourceChange[] };
  const changes: PlanChange[] = (doc.resource_changes ?? [])
    .filter((r) => (r.mode ?? 'managed') === 'managed')
    .map((r) => ({ action: mapActions(r.change.actions), address: r.address, type: r.type }));
  const n = (...as: PlanChange['action'][]) => changes.filter((c) => as.includes(c.action)).length;
  const replace = n('replace');
  const summary = `Plan: ${n('create') + replace} to add, ${n('update')} to change, ${n('delete') + replace} to destroy.`;
  return { changes, summary };
}

type OutputDoc = Record<string, { value: unknown }>;

/** `tofu output -json` -> contract outputs (infra/contract.md). Null when nothing is applied. */
export function parseTofuOutputs(json: string, region?: string): AwsSandboxOutputs | null {
  const doc = JSON.parse(json) as OutputDoc;
  if (Object.keys(doc).length === 0) return null;
  const get = <T>(k: string): T => {
    const o = doc[k];
    if (!o) throw new Error(`tofu output "${k}" missing (infra/contract.md)`);
    return o.value as T;
  };
  type RawIdentity = { role_arn: string; role_name: string; instance_profile_arn: string; instance_profile_name: string };
  const identity = (r: RawIdentity): AwsIdentityRef => ({
    roleArn: r.role_arn,
    roleName: r.role_name,
    instanceProfileArn: r.instance_profile_arn,
    instanceProfileName: r.instance_profile_name,
  });
  const eval_ = get<RawIdentity>('eval_identity_ref');
  const confirm = get<RawIdentity>('confirm_identity_ref');
  // Optional: sandboxes applied before dependency snapshots have no such output.
  const deps = doc.deps_identity_ref ? (doc.deps_identity_ref.value as RawIdentity) : undefined;
  const net = get<{
    vpc_id: string;
    subnet_id: string;
    bench_subnet_id: string;
    security_group_ids: { agent_runner: string; bench: string };
    egress_proxy_url: string | null;
  }>('network_ref');
  const limits = get<{ max_instances_per_agent: number; max_instance_lifetime_hours: number }>('limits');
  return {
    artifactsBucket: get('artifacts_bucket'),
    sealedBucket: get('sealed_bucket'),
    agentBoundaryRef: get('agent_boundary_ref'),
    // The port carries a single string; the bench launcher needs the instance profile.
    evalIdentityRef: eval_.instance_profile_arn,
    agentRolePath: get('agent_role_path'),
    network: {
      subnetId: net.subnet_id,
      agentRunnerSecurityGroupId: net.security_group_ids.agent_runner,
      benchSecurityGroupId: net.security_group_ids.bench,
      benchSubnetId: net.bench_subnet_id,
      egressProxyUrl: net.egress_proxy_url ?? null,
    },
    region: region ?? '',
    vpcId: net.vpc_id,
    evalIdentity: identity(eval_),
    confirmIdentity: identity(confirm),
    ...(deps ? { depsIdentity: identity(deps) } : {}),
    sealedConfirmPrefix: get<string>('sealed_confirm_prefix'),
    limits: { maxInstancesPerAgent: limits.max_instances_per_agent, maxInstanceLifetimeHours: limits.max_instance_lifetime_hours },
  };
}
