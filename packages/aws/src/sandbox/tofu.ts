import { mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type {
  PlanChange,
  SandboxInput,
  SandboxOutputs,
  SandboxProvider,
} from '@ratchet/core';
import { assertValidSandboxId } from '../util.js';
import { NodeProcessRunner, type ProcessResult, type ProcessRunner } from './process.js';

/** Contract outputs plus the AWS-specific detail the compute layer needs. */
export interface AwsSandboxOutputs extends SandboxOutputs {
  region: string;
  vpcId: string;
  evalIdentity: { roleArn: string; roleName: string; instanceProfileArn: string; instanceProfileName: string };
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
      return this.makePlan(ctx);
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
      return out;
    });
  }

  async destroy(input: SandboxInput): Promise<void> {
    return this.locked(input.sandboxId, async () => {
      const roles = await this.opts.listRuntimeAgentRoles(input);
      if (roles.length > 0) throw new TeardownBlockedError(roles);
      const ctx = await this.prepare(input);
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
      return this.readOutputs({ dir, statePath, varsPath, planPath: '', input: { sandboxId, region } as SandboxInput });
    });
  }

  // ---- internals ----------------------------------------------------------

  private async prepare(input: SandboxInput): Promise<Ctx> {
    assertValidSandboxId(input.sandboxId);
    if (input.allowedInstanceTypes.length === 0) throw new Error('allowedInstanceTypes must not be empty');
    const dir = this.workDir(input.sandboxId);
    await mkdir(dir, { recursive: true });
    const varsPath = join(dir, 'vars.tfvars.json');
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
    return { input, dir, varsPath, statePath: join(dir, 'terraform.tfstate'), planPath: join(dir, 'plan.tfplan') };
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
    return parseTofuOutputs(res.stdout, (ctx.input as Partial<SandboxInput>).region);
  }

  private async tofu(ctx: Ctx, args: string[], okCodes: number[] = [0]): Promise<ProcessResult> {
    const result = await this.runner.run({
      command: this.opts.tofuBinary,
      args,
      cwd: this.opts.moduleDir,
      env: {
        ...this.opts.env,
        TF_DATA_DIR: join(ctx.dir, '.terraform-data'),
        TF_IN_AUTOMATION: '1',
        TF_INPUT: '0',
        CHECKPOINT_DISABLE: '1',
      },
    });
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
  const eval_ = get<{ role_arn: string; role_name: string; instance_profile_arn: string; instance_profile_name: string }>('eval_identity_ref');
  const net = get<{ vpc_id: string; subnet_id: string; security_group_ids: { agent_runner: string; bench: string } }>('network_ref');
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
    },
    region: region ?? '',
    vpcId: net.vpc_id,
    evalIdentity: {
      roleArn: eval_.role_arn,
      roleName: eval_.role_name,
      instanceProfileArn: eval_.instance_profile_arn,
      instanceProfileName: eval_.instance_profile_name,
    },
    limits: { maxInstancesPerAgent: limits.max_instances_per_agent, maxInstanceLifetimeHours: limits.max_instance_lifetime_hours },
  };
}
