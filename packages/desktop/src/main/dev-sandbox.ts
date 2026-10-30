import type { PlanChange, SandboxInput, SandboxOutputs, SandboxProvider } from '@ratchet/core';

/**
 * Temporary stand-in for the real `SandboxProvider` (@ratchet/aws TofuRunner / MockSandboxProvider).
 * Returns a realistic OpenTofu plan so the AWS sandbox screen has something to render.
 */
export class DevSandboxProvider implements SandboxProvider {
  private applied = new Map<string, SandboxOutputs>();

  async plan(input: SandboxInput): Promise<{ changes: PlanChange[]; summary: string }> {
    const exists = this.applied.has(input.sandboxId);
    const act = (a: PlanChange['action']): PlanChange['action'] => (exists ? 'no-op' : a);
    const changes: PlanChange[] = [
      { action: act('create'), address: 'aws_s3_bucket.artifacts', type: 'aws_s3_bucket' },
      { action: act('create'), address: 'aws_s3_bucket.sealed', type: 'aws_s3_bucket' },
      { action: act('create'), address: 'aws_s3_bucket_public_access_block.sealed', type: 'aws_s3_bucket_public_access_block' },
      { action: act('create'), address: 'aws_iam_policy.agent_boundary', type: 'aws_iam_policy' },
      { action: act('create'), address: 'aws_iam_role.eval_identity', type: 'aws_iam_role' },
      { action: act('create'), address: 'aws_vpc.sandbox', type: 'aws_vpc' },
      { action: act('create'), address: 'aws_subnet.runners', type: 'aws_subnet' },
      { action: act('create'), address: 'aws_security_group.agent_runner', type: 'aws_security_group' },
      { action: act('create'), address: 'aws_security_group.bench', type: 'aws_security_group' },
      { action: act('update'), address: 'aws_budgets_budget.sandbox', type: 'aws_budgets_budget' },
    ];
    const n = (a: PlanChange['action']) => changes.filter((c) => c.action === a).length;
    return { changes, summary: `Plan: ${n('create')} to add, ${n('update')} to change, ${n('delete')} to destroy.` };
  }

  async apply(input: SandboxInput): Promise<SandboxOutputs> {
    const out = this.outputsFor(input.sandboxId);
    this.applied.set(input.sandboxId, out);
    return out;
  }

  async destroy(input: SandboxInput): Promise<void> {
    this.applied.delete(input.sandboxId);
  }

  async outputs(sandboxId: string): Promise<SandboxOutputs | null> {
    // The dev stub always reports an applied sandbox so the outputs panel is populated.
    return this.applied.get(sandboxId) ?? this.outputsFor(sandboxId);
  }

  private outputsFor(id: string): SandboxOutputs {
    return {
      artifactsBucket: `ratchet-${id}-artifacts`,
      sealedBucket: `ratchet-${id}-sealed`,
      agentBoundaryRef: `arn:aws:iam::000000000000:policy/ratchet-${id}-agent-boundary`,
      evalIdentityRef: `arn:aws:iam::000000000000:role/ratchet-${id}-eval`,
      agentRolePath: `/ratchet/${id}/agents/`,
      network: { subnetId: 'subnet-0a1b2c3d4e5f60718', agentRunnerSecurityGroupId: 'sg-0123456789abcdef0', benchSecurityGroupId: 'sg-0fedcba9876543210' },
    };
  }
}
