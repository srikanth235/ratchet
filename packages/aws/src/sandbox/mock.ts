import { validateEnvironment } from '@ratchet/core';
import type { PlanChange, SandboxEnvironment, SandboxEnvironmentOutputs, SandboxInput, SandboxProvider } from '@ratchet/core';
import { assertValidSandboxId } from '../util.js';
import { assertDraftedEnvironment } from './environment.js';
import { TeardownBlockedError, type AwsSandboxOutputs } from './tofu.js';

/** The headline resources of infra/aws (docs/V0_ARCHITECTURE.md §7), with real addresses. */
export const SANDBOX_RESOURCES: ReadonlyArray<{ address: string; type: string }> = [
  { address: 'aws_s3_bucket.artifacts', type: 'aws_s3_bucket' },
  { address: 'aws_s3_bucket.sealed', type: 'aws_s3_bucket' },
  { address: 'aws_s3_bucket_policy.sealed_deny_agents', type: 'aws_s3_bucket_policy' },
  { address: 'aws_iam_policy.agent_boundary', type: 'aws_iam_policy' },
  { address: 'aws_iam_role.bench_runner', type: 'aws_iam_role' },
  { address: 'aws_iam_instance_profile.bench_runner', type: 'aws_iam_instance_profile' },
  { address: 'aws_iam_role.confirm_runner', type: 'aws_iam_role' },
  { address: 'aws_iam_instance_profile.confirm_runner', type: 'aws_iam_instance_profile' },
  { address: 'aws_iam_role.deps_runner', type: 'aws_iam_role' },
  { address: 'aws_iam_instance_profile.deps_runner', type: 'aws_iam_instance_profile' },
  { address: 'aws_vpc.sandbox', type: 'aws_vpc' },
  { address: 'aws_subnet.bench', type: 'aws_subnet' },
  { address: 'aws_subnet.agent', type: 'aws_subnet' },
  { address: 'aws_vpc_endpoint.s3', type: 'aws_vpc_endpoint' },
  { address: 'aws_vpc_endpoint.interface["ssm"]', type: 'aws_vpc_endpoint' },
  { address: 'aws_vpc_endpoint.interface["ssmmessages"]', type: 'aws_vpc_endpoint' },
  { address: 'aws_vpc_endpoint.interface["ec2messages"]', type: 'aws_vpc_endpoint' },
  { address: 'aws_vpc_endpoint.interface["logs"]', type: 'aws_vpc_endpoint' },
  { address: 'aws_security_group.agent_runner', type: 'aws_security_group' },
  { address: 'aws_security_group.bench', type: 'aws_security_group' },
  { address: 'aws_security_group.endpoints', type: 'aws_security_group' },
];

/** What a drafted environment would create, from its files (the simulation's stand-in for the second tofu stage). */
function mockEnvironmentOutputs(env: SandboxEnvironment): SandboxEnvironmentOutputs {
  const v = validateEnvironment({ scope: env.scope, moduleOnly: true, files: env.files.map((f) => ({ path: `${env.path}/${f.path}`, content: f.content })) });
  const machines: SandboxEnvironmentOutputs['machines'] = {};
  const resources: string[] = [];
  v.machines.forEach((m, i) => {
    const key = m.key ?? m.name;
    machines[key] = { id: `i-0mockenv${String(i).padStart(4, '0')}`, role: m.role, privateIp: `10.42.1.${10 + i}` };
    resources.push(`module.sandbox.aws_instance.machine["${key}"]`);
    if ((m.dataVolumeGb ?? 0) > 0) resources.push(`module.sandbox.aws_ebs_volume.data["${key}"]`, `module.sandbox.aws_volume_attachment.data["${key}"]`);
  });
  return { kind: env.kind, scope: env.scope, hash: env.hash, resources, machines };
}

export interface MockSandboxProviderOptions {
  accountId?: string;
  /** Called on destroy, mirroring TofuSandboxProvider's teardown guard. */
  listRuntimeAgentRoles?: (input: SandboxInput) => Promise<string[]>;
  /** Sandboxes that are already applied (e.g. restored from the ledger after an app restart), by sandbox id. */
  initial?: Record<string, AwsSandboxOutputs>;
}

/** In-memory SandboxProvider for simulation; behaves like TofuSandboxProvider for the engine. */
export class MockSandboxProvider implements SandboxProvider {
  private readonly applied = new Map<string, AwsSandboxOutputs>();
  readonly calls: string[] = [];

  constructor(private readonly opts: MockSandboxProviderOptions = {}) {
    for (const [id, out] of Object.entries(opts.initial ?? {})) this.applied.set(id, out);
  }

  async plan(input: SandboxInput): Promise<{ changes: PlanChange[]; summary: string }> {
    this.calls.push(`plan:${input.sandboxId}`);
    assertValidSandboxId(input.sandboxId);
    const exists = this.applied.has(input.sandboxId);
    const changes: PlanChange[] = SANDBOX_RESOURCES.map((r) => ({
      action: exists ? 'no-op' : 'create',
      address: r.address,
      type: r.type,
    }));
    if (input.environment?.kind === 'drafted') {
      assertDraftedEnvironment(input.environment);
      for (const address of mockEnvironmentOutputs(input.environment).resources) {
        changes.push({ action: exists ? 'no-op' : 'create', address, type: /ebs_volume/.test(address) ? 'aws_ebs_volume' : /volume_attachment/.test(address) ? 'aws_volume_attachment' : 'aws_instance' });
      }
    }
    const n = exists ? 0 : changes.length;
    return { changes, summary: `Plan: ${n} to add, 0 to change, 0 to destroy.` };
  }

  async apply(input: SandboxInput): Promise<AwsSandboxOutputs> {
    this.calls.push(`apply:${input.sandboxId}`);
    assertValidSandboxId(input.sandboxId);
    if (input.environment?.kind === 'drafted') assertDraftedEnvironment(input.environment); // the same refusal as the real provider
    const existing = this.applied.get(input.sandboxId);
    if (existing) return existing;
    const acct = this.opts.accountId ?? '000000000000';
    const p = `ratchet-${input.sandboxId}`;
    const out: AwsSandboxOutputs = {
      artifactsBucket: `${p}-artifacts`,
      sealedBucket: `${p}-sealed`,
      agentBoundaryRef: `arn:aws:iam::${acct}:policy/ratchet/${p}-agent-boundary`,
      evalIdentityRef: `arn:aws:iam::${acct}:instance-profile/ratchet/bench/${p}-bench-runner`,
      agentRolePath: '/ratchet/agent/',
      network: {
        subnetId: 'subnet-0mockagent',
        benchSubnetId: 'subnet-0mockbench',
        agentRunnerSecurityGroupId: 'sg-0mockrunner',
        benchSecurityGroupId: 'sg-0mockbench',
        egressProxyUrl: null,
      },
      region: input.region,
      vpcId: 'vpc-0mock0000',
      evalIdentity: {
        roleArn: `arn:aws:iam::${acct}:role/ratchet/bench/${p}-bench-runner`,
        roleName: `${p}-bench-runner`,
        instanceProfileArn: `arn:aws:iam::${acct}:instance-profile/ratchet/bench/${p}-bench-runner`,
        instanceProfileName: `${p}-bench-runner`,
      },
      confirmIdentity: {
        roleArn: `arn:aws:iam::${acct}:role/ratchet/bench/${p}-confirm-runner`,
        roleName: `${p}-confirm-runner`,
        instanceProfileArn: `arn:aws:iam::${acct}:instance-profile/ratchet/bench/${p}-confirm-runner`,
        instanceProfileName: `${p}-confirm-runner`,
      },
      depsIdentity: {
        roleArn: `arn:aws:iam::${acct}:role/ratchet/bench/${p}-deps-runner`,
        roleName: `${p}-deps-runner`,
        instanceProfileArn: `arn:aws:iam::${acct}:instance-profile/ratchet/bench/${p}-deps-runner`,
        instanceProfileName: `${p}-deps-runner`,
      },
      sealedConfirmPrefix: 'holdout/',
      limits: { maxInstancesPerAgent: input.maxInstancesPerAgent, maxInstanceLifetimeHours: input.maxInstanceLifetimeHours },
      ...(input.environment ? { environment: input.environment.kind === 'drafted' ? mockEnvironmentOutputs(input.environment) : { kind: input.environment.kind, scope: input.environment.scope, hash: input.environment.hash, resources: [], machines: {} } } : {}),
    };
    this.applied.set(input.sandboxId, out);
    return out;
  }

  async destroy(input: SandboxInput): Promise<void> {
    this.calls.push(`destroy:${input.sandboxId}`);
    const roles = (await this.opts.listRuntimeAgentRoles?.(input)) ?? [];
    if (roles.length > 0) throw new TeardownBlockedError(roles);
    this.applied.delete(input.sandboxId);
  }

  async outputs(sandboxId: string): Promise<AwsSandboxOutputs | null> {
    return this.applied.get(sandboxId) ?? null;
  }
}
