import type { PlanChange, SandboxInput, SandboxProvider } from '@ratchet/core';
import { assertValidSandboxId } from '../util.js';
import { TeardownBlockedError, type AwsSandboxOutputs } from './tofu.js';

/** The 8 headline resources of infra/aws plus the bench role policy, with real addresses. */
export const SANDBOX_RESOURCES: ReadonlyArray<{ address: string; type: string }> = [
  { address: 'aws_s3_bucket.artifacts', type: 'aws_s3_bucket' },
  { address: 'aws_s3_bucket.sealed', type: 'aws_s3_bucket' },
  { address: 'aws_s3_bucket_policy.sealed_deny_agents', type: 'aws_s3_bucket_policy' },
  { address: 'aws_iam_policy.agent_boundary', type: 'aws_iam_policy' },
  { address: 'aws_iam_role.bench_runner', type: 'aws_iam_role' },
  { address: 'aws_iam_instance_profile.bench_runner', type: 'aws_iam_instance_profile' },
  { address: 'aws_security_group.agent_runner', type: 'aws_security_group' },
  { address: 'aws_security_group.bench', type: 'aws_security_group' },
];

export interface MockSandboxProviderOptions {
  accountId?: string;
  /** Called on destroy, mirroring TofuSandboxProvider's teardown guard. */
  listRuntimeAgentRoles?: (input: SandboxInput) => Promise<string[]>;
}

/** In-memory SandboxProvider for simulation; behaves like TofuSandboxProvider for the engine. */
export class MockSandboxProvider implements SandboxProvider {
  private readonly applied = new Map<string, AwsSandboxOutputs>();
  readonly calls: string[] = [];

  constructor(private readonly opts: MockSandboxProviderOptions = {}) {}

  async plan(input: SandboxInput): Promise<{ changes: PlanChange[]; summary: string }> {
    this.calls.push(`plan:${input.sandboxId}`);
    assertValidSandboxId(input.sandboxId);
    const exists = this.applied.has(input.sandboxId);
    const changes: PlanChange[] = SANDBOX_RESOURCES.map((r) => ({
      action: exists ? 'no-op' : 'create',
      address: r.address,
      type: r.type,
    }));
    const n = exists ? 0 : changes.length;
    return { changes, summary: `Plan: ${n} to add, 0 to change, 0 to destroy.` };
  }

  async apply(input: SandboxInput): Promise<AwsSandboxOutputs> {
    this.calls.push(`apply:${input.sandboxId}`);
    assertValidSandboxId(input.sandboxId);
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
      network: { subnetId: 'subnet-0mock0000', agentRunnerSecurityGroupId: 'sg-0mockrunner', benchSecurityGroupId: 'sg-0mockbench' },
      region: input.region,
      vpcId: 'vpc-0mock0000',
      evalIdentity: {
        roleArn: `arn:aws:iam::${acct}:role/ratchet/bench/${p}-bench-runner`,
        roleName: `${p}-bench-runner`,
        instanceProfileArn: `arn:aws:iam::${acct}:instance-profile/ratchet/bench/${p}-bench-runner`,
        instanceProfileName: `${p}-bench-runner`,
      },
      limits: { maxInstancesPerAgent: input.maxInstancesPerAgent, maxInstanceLifetimeHours: input.maxInstanceLifetimeHours },
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
