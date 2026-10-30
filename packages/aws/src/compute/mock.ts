import type {
  AgentId,
  AgentIdentity,
  BenchLaunchSpec,
  CloudCompute,
  Clock,
  InstanceInfo,
  OptimizationId,
  RemoteCommand,
  RemoteCommandResult,
  RunnerLaunchSpec,
  SecretRef,
} from '@ratchet/core';
import { DEFAULT_AGENT_ROLE_PATH, assertValidId, systemClock } from '../util.js';
import { agentRoleName, secretPrefix } from './aws.js';

export type MockCommandHandler = (
  instance: InstanceInfo,
  command: RemoteCommand,
) => Promise<RemoteCommandResult> | RemoteCommandResult;

export interface MockCloudComputeOptions {
  clock?: Clock;
  /** Executes `runCommand` scripts; lets a local bench/runner stand in for the instance. */
  commandHandler?: MockCommandHandler;
  /** Time from launch to `running`. Default 0 (immediately running). */
  bootDelayMs?: number;
  accountId?: string;
  agentRolePath?: string;
}

interface MockIdentity {
  identity: AgentIdentity;
  revoked: boolean;
}

/**
 * In-memory CloudCompute with the same observable behaviour as AwsCloudCompute:
 * id validation, tagged instances, kill switch, ordered delete, idempotent terminate.
 */
export class MockCloudCompute implements CloudCompute {
  private readonly clock: Clock;
  private readonly instances = new Map<string, InstanceInfo & { bootedAt: number }>();
  private readonly roles = new Map<string, MockIdentity>(); // key: role ARN
  private readonly secrets = new Map<string, string>(); // key: SSM parameter name
  private seq = 0;
  readonly commands: Array<{ instanceId: string; command: RemoteCommand }> = [];
  commandHandler: MockCommandHandler | undefined;

  constructor(private readonly opts: MockCloudComputeOptions = {}) {
    this.clock = opts.clock ?? systemClock;
    this.commandHandler = opts.commandHandler;
  }

  async createAgentIdentity(optimizationId: OptimizationId, agentId: AgentId): Promise<AgentIdentity> {
    assertValidId('optimization', optimizationId);
    assertValidId('agent', agentId);
    const name = agentRoleName(optimizationId, agentId);
    const path = this.opts.agentRolePath ?? DEFAULT_AGENT_ROLE_PATH;
    const acct = this.opts.accountId ?? '000000000000';
    const identity: AgentIdentity = {
      optimizationId,
      agentId,
      roleRef: `arn:aws:iam::${acct}:role${path}${name}`,
      instanceProfileRef: `arn:aws:iam::${acct}:instance-profile${path}${name}`,
    };
    const existing = this.roles.get(identity.roleRef);
    if (existing) return existing.identity; // idempotent, like the real one
    this.roles.set(identity.roleRef, { identity, revoked: false });
    return identity;
  }

  async revokeAgentIdentity(identity: AgentIdentity): Promise<void> {
    const r = this.roles.get(identity.roleRef);
    if (!r) throw new Error(`NoSuchEntity: role ${identity.roleRef}`);
    r.revoked = true;
  }

  async deleteAgentIdentity(identity: AgentIdentity): Promise<void> {
    this.roles.delete(identity.roleRef); // missing is fine (idempotent)
  }

  async putSecret(optimizationId: OptimizationId, agentId: AgentId, name: string, value: string): Promise<SecretRef> {
    assertValidId('optimization', optimizationId);
    assertValidId('agent', agentId);
    if (!/^[A-Za-z0-9_.-]+$/.test(name)) throw new Error(`invalid secret name ${JSON.stringify(name)}`);
    const ref = `${secretPrefix(optimizationId, agentId)}${name}`;
    this.secrets.set(ref, value);
    return { name, ref };
  }

  async deleteSecrets(optimizationId: OptimizationId, agentId: AgentId): Promise<void> {
    const prefix = secretPrefix(optimizationId, agentId);
    for (const k of [...this.secrets.keys()]) if (k.startsWith(prefix)) this.secrets.delete(k);
  }

  async launchRunner(spec: RunnerLaunchSpec): Promise<InstanceInfo> {
    assertValidId('optimization', spec.optimizationId);
    assertValidId('agent', spec.agentId);
    if (!this.roles.has(spec.identity.roleRef)) throw new Error(`InvalidParameterValue: Invalid IAM Instance Profile ${spec.identity.instanceProfileRef}`);
    return this.add('agent_runner', spec.optimizationId, spec.agentId, spec.instanceType);
  }

  async launchBenchInstance(spec: BenchLaunchSpec): Promise<InstanceInfo> {
    assertValidId('optimization', spec.optimizationId);
    return this.add('bench', spec.optimizationId, undefined, spec.instanceType);
  }

  /** Simulation helper: an experiment instance an agent launched itself (tags opt+agent only). */
  simulateExperiment(optimizationId: OptimizationId, agentId: AgentId, instanceType: string): InstanceInfo {
    return this.add('experiment', optimizationId, agentId, instanceType);
  }

  /** Simulation helper: guest-initiated shutdown (terminates, as InstanceInitiatedShutdownBehavior=terminate). */
  simulateShutdown(instanceId: string): void {
    void this.terminate([instanceId]);
  }

  /** Simulation helper: an instance that ended up merely stopped (e.g. launched without terminate-on-shutdown). */
  simulateStopped(instanceId: string): void {
    const i = this.instances.get(instanceId);
    if (i && i.state !== 'terminated') {
      i.state = 'stopped';
      i.terminatedAt = this.clock.now();
    }
  }

  private add(role: InstanceInfo['role'], optimizationId: string, agentId: string | undefined, instanceType: string): InstanceInfo {
    const id = `i-mock${(++this.seq).toString(16).padStart(12, '0')}`;
    const now = this.clock.now();
    const inst = {
      id,
      role,
      optimizationId,
      ...(agentId ? { agentId } : {}),
      instanceType,
      state: 'pending' as InstanceInfo['state'],
      launchedAt: now,
      bootedAt: now + (this.opts.bootDelayMs ?? 0),
    };
    this.instances.set(id, inst);
    return this.view(inst);
  }

  async terminate(instanceIds: string[]): Promise<void> {
    const now = this.clock.now();
    for (const id of instanceIds) {
      const i = this.instances.get(id);
      if (!i || i.state === 'terminated') continue; // idempotent
      i.state = 'terminated';
      i.terminatedAt = now;
    }
  }

  async listTagged(optimizationId?: OptimizationId): Promise<InstanceInfo[]> {
    return [...this.instances.values()]
      .filter((i) => !optimizationId || i.optimizationId === optimizationId)
      .map((i) => this.view(i));
  }

  async runCommand(instanceId: string, command: RemoteCommand): Promise<RemoteCommandResult> {
    const i = this.instances.get(instanceId);
    if (!i) throw new Error(`InvalidInstanceId: ${instanceId}`);
    const view = this.view(i);
    if (view.state !== 'running') throw new Error(`InvalidInstanceId: ${instanceId} is ${view.state}, not running / not SSM-managed`);
    this.commands.push({ instanceId, command });
    if (!this.commandHandler) return { exitCode: 0, stdout: '', stderr: '' };
    return this.commandHandler(view, command);
  }

  // ---- inspection ---------------------------------------------------------

  isRevoked(identity: AgentIdentity): boolean {
    return this.roles.get(identity.roleRef)?.revoked ?? false;
  }
  identities(): AgentIdentity[] {
    return [...this.roles.values()].map((r) => r.identity);
  }
  secretNames(): string[] {
    return [...this.secrets.keys()].sort();
  }
  secretValue(ref: string): string | undefined {
    return this.secrets.get(ref);
  }

  private view(i: InstanceInfo & { bootedAt: number }): InstanceInfo {
    if (i.state === 'pending' && this.clock.now() >= i.bootedAt) i.state = 'running';
    const { bootedAt: _b, ...info } = i;
    return { ...info };
  }
}
