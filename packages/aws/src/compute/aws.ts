import {
  DescribeInstancesCommand,
  EC2Client,
  RunInstancesCommand,
  TerminateInstancesCommand,
  type Instance,
  type RunInstancesCommandInput,
  type Tag,
} from '@aws-sdk/client-ec2';
import {
  AddRoleToInstanceProfileCommand,
  CreateInstanceProfileCommand,
  CreateRoleCommand,
  DeleteInstanceProfileCommand,
  DeleteRoleCommand,
  DeleteRolePolicyCommand,
  DetachRolePolicyCommand,
  IAMClient,
  ListAttachedRolePoliciesCommand,
  ListRolePoliciesCommand,
  ListRolesCommand,
  PutRolePolicyCommand,
  RemoveRoleFromInstanceProfileCommand,
} from '@aws-sdk/client-iam';
import {
  AddTagsToResourceCommand,
  DeleteParametersCommand,
  GetCommandInvocationCommand,
  GetParametersByPathCommand,
  PutParameterCommand,
  SendCommandCommand,
  SSMClient,
} from '@aws-sdk/client-ssm';
import { GetCallerIdentityCommand, STSClient } from '@aws-sdk/client-sts';
import type {
  AgentId,
  AgentIdentity,
  BenchLaunchSpec,
  CloudCompute,
  Clock,
  InstanceInfo,
  InstanceRole,
  OptimizationId,
  RemoteCommand,
  RemoteCommandResult,
  RunnerLaunchSpec,
  SecretRef,
} from '@ratchet/core';
import {
  DEFAULT_AGENT_ROLE_PATH,
  TAG_AGENT,
  TAG_OPT,
  TAG_PURPOSE,
  TAG_ROLE,
  TAG_SANDBOX,
  arnName,
  assertValidId,
  assertValidInstanceId,
  boundedName,
  errName,
  shQuote,
  systemClock,
} from '../util.js';
import type { AwsSandboxOutputs } from '../sandbox/tofu.js';

export const KILL_SWITCH_POLICY_NAME = 'ratchet-kill-switch';
export const AGENT_POLICY_NAME = 'ratchet-agent-permissions';

export interface AwsComputeConfig {
  sandboxId: string;
  region: string;
  /** `agent_boundary_ref` output. */
  agentBoundaryArn: string;
  /** `agent_role_path` output, e.g. `/ratchet/agent/`. */
  agentRolePath: string;
  /** Agent subnet (`network_ref.subnet_id`). */
  subnetId: string;
  /**
   * Private bench subnet with no route to the internet (`network_ref.bench_subnet_id`). Required to
   * launch bench instances: there is deliberately no fallback to `subnetId`, which may be public.
   */
  benchSubnetId?: string;
  agentRunnerSecurityGroupId: string;
  benchSecurityGroupId: string;
  /** Bench instance profile for `baseline` and `bench` phases: ARN or name (`eval_identity_ref`). NO sealed access. */
  benchInstanceProfile: string;
  /**
   * Instance profile for `confirm` runs only (`confirm_identity_ref`): the only identity that can read the
   * sealed bucket. Required to launch a confirm instance; must differ from `benchInstanceProfile`.
   */
  confirmInstanceProfile?: string;
  /** `network_ref.egress_proxy_url`; when set, agent runners get HTTPS_PROXY. Null/undefined: no proxy. */
  egressProxyUrl?: string | null;
  artifactsBucket: string;
  /** Local self-destruct for runners/bench (`shutdown` + terminate-on-shutdown). Default 2. */
  maxInstanceLifetimeHours?: number;
}

export function awsComputeConfigFromOutputs(o: AwsSandboxOutputs, sandboxId: string): AwsComputeConfig {
  return {
    sandboxId,
    region: o.region,
    agentBoundaryArn: o.agentBoundaryRef,
    agentRolePath: o.agentRolePath,
    subnetId: o.network.subnetId,
    benchSubnetId: o.network.benchSubnetId,
    agentRunnerSecurityGroupId: o.network.agentRunnerSecurityGroupId,
    benchSecurityGroupId: o.network.benchSecurityGroupId,
    benchInstanceProfile: o.evalIdentity.instanceProfileArn,
    confirmInstanceProfile: o.confirmIdentity.instanceProfileArn,
    egressProxyUrl: o.network.egressProxyUrl,
    artifactsBucket: o.artifactsBucket,
    maxInstanceLifetimeHours: o.limits.maxInstanceLifetimeHours,
  };
}

export interface AwsClients {
  ec2: EC2Client;
  iam: IAMClient;
  ssm: SSMClient;
  sts: STSClient;
}

export interface AwsCloudComputeOptions {
  clients?: Partial<AwsClients>;
  /** Named profile in ~/.aws/config (static, SSO or assume-role); default credential chain when unset. */
  profile?: string;
  clock?: Clock;
  /** SSM poll interval for runCommand. Default 2000. */
  pollIntervalMs?: number;
  /** Retries for IAM eventual consistency on RunInstances. Default 6. */
  iamPropagationRetries?: number;
}

const TERMINAL_SSM = new Set(['Success', 'Failed', 'Cancelled', 'TimedOut', 'DeliveryTimedOut', 'ExecutionTimedOut', 'Undeliverable', 'Terminated']);
const ENV_KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;
const SECRET_NAME = /^[A-Za-z0-9_.-]+$/;

export class AwsCloudCompute implements CloudCompute {
  readonly clients: AwsClients;
  private readonly clock: Clock;
  private account?: { accountId: string; partition: string };

  constructor(
    private readonly cfg: AwsComputeConfig,
    private readonly opts: AwsCloudComputeOptions = {},
  ) {
    const region = cfg.region;
    const profile = opts.profile ? { profile: opts.profile } : {};
    this.clients = {
      ec2: opts.clients?.ec2 ?? new EC2Client({ region, ...profile }),
      iam: opts.clients?.iam ?? new IAMClient({ region, ...profile }),
      ssm: opts.clients?.ssm ?? new SSMClient({ region, ...profile }),
      sts: opts.clients?.sts ?? new STSClient({ region, ...profile }),
    };
    this.clock = opts.clock ?? systemClock;
    if (!cfg.agentRolePath.startsWith('/') || !cfg.agentRolePath.endsWith('/')) {
      throw new Error(`agentRolePath must start and end with "/": ${cfg.agentRolePath}`);
    }
    if (cfg.confirmInstanceProfile && cfg.confirmInstanceProfile === cfg.benchInstanceProfile) {
      throw new Error('confirmInstanceProfile must differ from benchInstanceProfile: baseline/bench phases must not hold sealed access');
    }
  }

  /** Verify credentials; optionally that they belong to the expected (dedicated) account. */
  async preflight(expectedAccountId?: string): Promise<{ accountId: string; arn: string }> {
    const r = await this.clients.sts.send(new GetCallerIdentityCommand({}));
    const accountId = r.Account ?? '';
    if (expectedAccountId && accountId !== expectedAccountId) {
      throw new Error(`credentials are for account ${accountId}, expected sandbox account ${expectedAccountId}`);
    }
    return { accountId, arn: r.Arn ?? '' };
  }

  private async acct(): Promise<{ accountId: string; partition: string }> {
    if (!this.account) {
      const r = await this.clients.sts.send(new GetCallerIdentityCommand({}));
      const arn = r.Arn ?? '';
      this.account = { accountId: r.Account ?? '', partition: arn.split(':')[1] || 'aws' };
    }
    return this.account;
  }

  // ---- identity -----------------------------------------------------------

  async createAgentIdentity(optimizationId: OptimizationId, agentId: AgentId): Promise<AgentIdentity> {
    assertValidId('optimization', optimizationId);
    assertValidId('agent', agentId);
    const { accountId, partition } = await this.acct();
    const name = agentRoleName(optimizationId, agentId);
    const tags = [
      { Key: TAG_OPT, Value: optimizationId },
      { Key: TAG_AGENT, Value: agentId },
      { Key: TAG_SANDBOX, Value: this.cfg.sandboxId },
    ];
    const { iam } = this.clients;

    const role = await tolerate('EntityAlreadyExists', async () => {
      const r = await iam.send(
        new CreateRoleCommand({
          RoleName: name,
          Path: this.cfg.agentRolePath,
          AssumeRolePolicyDocument: JSON.stringify({
            Version: '2012-10-17',
            Statement: [
              {
                Effect: 'Allow',
                Principal: { Service: 'ec2.amazonaws.com' },
                Action: 'sts:AssumeRole',
                Condition: { StringEquals: { 'aws:SourceAccount': accountId } },
              },
            ],
          }),
          PermissionsBoundary: this.cfg.agentBoundaryArn,
          Tags: tags,
          MaxSessionDuration: 3600,
          Description: `Ratchet agent ${agentId} of optimization ${optimizationId}`,
        }),
      );
      return r.Role?.Arn;
    });
    const roleArn = role ?? `arn:${partition}:iam::${accountId}:role${this.cfg.agentRolePath}${name}`;

    await iam.send(
      new PutRolePolicyCommand({
        RoleName: name,
        PolicyName: AGENT_POLICY_NAME,
        PolicyDocument: JSON.stringify(this.agentIdentityPolicy(partition, accountId, optimizationId, agentId)),
      }),
    );

    const profile = await tolerate('EntityAlreadyExists', async () => {
      const r = await iam.send(
        new CreateInstanceProfileCommand({ InstanceProfileName: name, Path: this.cfg.agentRolePath, Tags: tags }),
      );
      return r.InstanceProfile?.Arn;
    });
    await tolerate('LimitExceeded', () =>
      iam.send(new AddRoleToInstanceProfileCommand({ InstanceProfileName: name, RoleName: name })).then(() => undefined),
    );
    const profileArn = profile ?? `arn:${partition}:iam::${accountId}:instance-profile${this.cfg.agentRolePath}${name}`;

    return { optimizationId, agentId, roleRef: roleArn, instanceProfileRef: profileArn };
  }

  /** What the agent needs; the boundary caps it (effective = intersection). */
  private agentIdentityPolicy(partition: string, account: string, opt: string, agent: string): object {
    const region = this.cfg.region;
    return {
      Version: '2012-10-17',
      Statement: [
        {
          Sid: 'Ec2',
          Effect: 'Allow',
          Action: ['ec2:RunInstances', 'ec2:CreateTags', 'ec2:StartInstances', 'ec2:StopInstances', 'ec2:TerminateInstances', 'ec2:Describe*'],
          Resource: '*',
        },
        {
          Sid: 'ArtifactObjects',
          Effect: 'Allow',
          Action: ['s3:GetObject', 's3:PutObject', 's3:AbortMultipartUpload', 's3:ListMultipartUploadParts'],
          Resource: `arn:${partition}:s3:::${this.cfg.artifactsBucket}/artifacts/${opt}/${agent}/*`,
        },
        {
          Sid: 'ArtifactList',
          Effect: 'Allow',
          Action: 's3:ListBucket',
          Resource: `arn:${partition}:s3:::${this.cfg.artifactsBucket}`,
          Condition: { StringLike: { 's3:prefix': `artifacts/${opt}/${agent}/*` } },
        },
        {
          Sid: 'OwnSecrets',
          Effect: 'Allow',
          Action: ['ssm:GetParameter', 'ssm:GetParameters'],
          Resource: `arn:${partition}:ssm:${region}:${account}:parameter/ratchet/${opt}/${agent}/*`,
        },
        {
          Sid: 'SsmManagedInstance',
          Effect: 'Allow',
          Action: [
            'ssm:UpdateInstanceInformation',
            'ssmmessages:CreateControlChannel',
            'ssmmessages:CreateDataChannel',
            'ssmmessages:OpenControlChannel',
            'ssmmessages:OpenDataChannel',
            'ec2messages:AcknowledgeMessage',
            'ec2messages:DeleteMessage',
            'ec2messages:FailMessage',
            'ec2messages:GetEndpoint',
            'ec2messages:GetMessages',
            'ec2messages:SendReply',
          ],
          Resource: '*',
        },
      ],
    };
  }

  async revokeAgentIdentity(identity: AgentIdentity): Promise<void> {
    await this.clients.iam.send(
      new PutRolePolicyCommand({
        RoleName: arnName(identity.roleRef),
        PolicyName: KILL_SWITCH_POLICY_NAME,
        PolicyDocument: JSON.stringify({
          Version: '2012-10-17',
          Statement: [{ Sid: 'KillSwitchDenyAll', Effect: 'Deny', Action: '*', Resource: '*' }],
        }),
      }),
    );
  }

  /**
   * Order matters: the role must leave its instance profile before either can
   * be deleted, and a role cannot be deleted while inline/managed policies remain.
   */
  async deleteAgentIdentity(identity: AgentIdentity): Promise<void> {
    const { iam } = this.clients;
    const role = arnName(identity.roleRef);
    const profile = arnName(identity.instanceProfileRef);
    const ignoreMissing = (p: Promise<unknown>) => p.catch(ignoreCodes('NoSuchEntity'));

    await ignoreMissing(iam.send(new RemoveRoleFromInstanceProfileCommand({ InstanceProfileName: profile, RoleName: role })));
    await ignoreMissing(iam.send(new DeleteInstanceProfileCommand({ InstanceProfileName: profile })));

    const inline = await iam.send(new ListRolePoliciesCommand({ RoleName: role })).catch(ignoreCodes('NoSuchEntity'));
    for (const name of inline?.PolicyNames ?? []) {
      await ignoreMissing(iam.send(new DeleteRolePolicyCommand({ RoleName: role, PolicyName: name })));
    }
    const attached = await iam.send(new ListAttachedRolePoliciesCommand({ RoleName: role })).catch(ignoreCodes('NoSuchEntity'));
    for (const p of attached?.AttachedPolicies ?? []) {
      await ignoreMissing(iam.send(new DetachRolePolicyCommand({ RoleName: role, PolicyArn: p.PolicyArn })));
    }
    await ignoreMissing(iam.send(new DeleteRoleCommand({ RoleName: role })));
  }

  // ---- secrets ------------------------------------------------------------

  async putSecret(optimizationId: OptimizationId, agentId: AgentId, name: string, value: string): Promise<SecretRef> {
    assertValidId('optimization', optimizationId);
    assertValidId('agent', agentId);
    if (!SECRET_NAME.test(name) || name.includes('..')) throw new Error(`invalid secret name ${JSON.stringify(name)}`);
    const ref = `${secretPrefix(optimizationId, agentId)}${name}`;
    const { ssm } = this.clients;
    // PutParameter rejects Tags together with Overwrite, so tag separately.
    await ssm.send(new PutParameterCommand({ Name: ref, Value: value, Type: 'SecureString', Overwrite: true }));
    await ssm.send(
      new AddTagsToResourceCommand({
        ResourceType: 'Parameter',
        ResourceId: ref,
        Tags: [
          { Key: TAG_OPT, Value: optimizationId },
          { Key: TAG_AGENT, Value: agentId },
        ],
      }),
    );
    return { name, ref };
  }

  async deleteSecrets(optimizationId: OptimizationId, agentId: AgentId): Promise<void> {
    assertValidId('optimization', optimizationId);
    assertValidId('agent', agentId);
    const { ssm } = this.clients;
    const names: string[] = [];
    let token: string | undefined;
    do {
      const r = await ssm.send(
        new GetParametersByPathCommand({ Path: secretPrefix(optimizationId, agentId), Recursive: true, WithDecryption: false, NextToken: token }),
      );
      for (const p of r.Parameters ?? []) if (p.Name) names.push(p.Name);
      token = r.NextToken;
    } while (token);
    for (let i = 0; i < names.length; i += 10) {
      await ssm.send(new DeleteParametersCommand({ Names: names.slice(i, i + 10) }));
    }
  }

  // ---- compute ------------------------------------------------------------

  async launchRunner(spec: RunnerLaunchSpec): Promise<InstanceInfo> {
    assertValidId('optimization', spec.optimizationId);
    assertValidId('agent', spec.agentId);
    const params = this.baseRunParams({
      imageRef: spec.imageRef,
      instanceType: spec.instanceType,
      subnetId: this.cfg.subnetId,
      securityGroupId: this.cfg.agentRunnerSecurityGroupId,
      profile: { Arn: spec.identity.instanceProfileRef },
      userData: runnerUserData(spec, this.cfg.maxInstanceLifetimeHours ?? 2, { region: this.cfg.region, proxyUrl: this.cfg.egressProxyUrl }),
      tags: [
        { Key: TAG_OPT, Value: spec.optimizationId },
        { Key: TAG_AGENT, Value: spec.agentId },
        { Key: TAG_ROLE, Value: 'agent_runner' },
        { Key: TAG_SANDBOX, Value: this.cfg.sandboxId },
        { Key: 'Name', Value: `ratchet-runner-${spec.optimizationId}-${spec.agentId}` },
      ],
    });
    return this.runInstance(params, 'agent_runner', spec.optimizationId, spec.agentId, spec.instanceType);
  }

  /**
   * Bench instances sit in the private bench subnet. The instance profile depends on the phase:
   * only `confirm` gets the identity that can read the sealed bucket; `baseline` and `bench` run
   * candidate code with no sealed access at all. Anything else, or a missing profile, fails closed.
   */
  async launchBenchInstance(spec: BenchLaunchSpec): Promise<InstanceInfo> {
    assertValidId('optimization', spec.optimizationId);
    const p = benchProfileFor(this.cfg, spec.purpose);
    if (!this.cfg.benchSubnetId) {
      throw new Error('benchSubnetId is not configured (network_ref.bench_subnet_id): refusing to launch a bench instance in a subnet that may have internet access');
    }
    const params = this.baseRunParams({
      imageRef: spec.imageRef,
      instanceType: spec.instanceType,
      subnetId: this.cfg.benchSubnetId,
      securityGroupId: this.cfg.benchSecurityGroupId,
      profile: p.startsWith('arn:') ? { Arn: p } : { Name: p },
      userData: benchUserData(spec, this.cfg.maxInstanceLifetimeHours ?? 2),
      tags: [
        { Key: TAG_OPT, Value: spec.optimizationId },
        { Key: TAG_ROLE, Value: 'bench' },
        { Key: TAG_PURPOSE, Value: spec.purpose },
        { Key: TAG_SANDBOX, Value: this.cfg.sandboxId },
        { Key: 'Name', Value: `ratchet-bench-${spec.optimizationId}-${spec.purpose}` },
      ],
    });
    return this.runInstance(params, 'bench', spec.optimizationId, undefined, spec.instanceType);
  }

  private baseRunParams(a: {
    imageRef: string;
    instanceType: string;
    subnetId: string;
    securityGroupId: string;
    profile: { Arn?: string; Name?: string };
    userData: string;
    tags: Tag[];
  }): RunInstancesCommandInput {
    return {
      ImageId: a.imageRef,
      InstanceType: a.instanceType as RunInstancesCommandInput['InstanceType'],
      MinCount: 1,
      MaxCount: 1,
      SubnetId: a.subnetId,
      SecurityGroupIds: [a.securityGroupId],
      IamInstanceProfile: a.profile,
      MetadataOptions: { HttpTokens: 'required', HttpEndpoint: 'enabled', HttpPutResponseHopLimit: 1 },
      InstanceInitiatedShutdownBehavior: 'terminate',
      UserData: Buffer.from(a.userData, 'utf8').toString('base64'),
      TagSpecifications: [
        { ResourceType: 'instance', Tags: a.tags },
        { ResourceType: 'volume', Tags: a.tags.filter((t) => t.Key !== 'Name') },
      ],
    };
  }

  private async runInstance(
    params: RunInstancesCommandInput,
    role: InstanceRole,
    optimizationId: string,
    agentId: string | undefined,
    instanceType: string,
  ): Promise<InstanceInfo> {
    const retries = this.opts.iamPropagationRetries ?? 6;
    for (let attempt = 0; ; attempt++) {
      try {
        const r = await this.clients.ec2.send(new RunInstancesCommand(params));
        const inst = r.Instances?.[0];
        if (!inst?.InstanceId) throw new Error('RunInstances returned no instance');
        return {
          id: inst.InstanceId,
          role,
          optimizationId,
          ...(agentId ? { agentId } : {}),
          instanceType,
          state: 'pending',
          launchedAt: inst.LaunchTime?.getTime() ?? this.clock.now(),
        };
      } catch (e) {
        // A just-created instance profile is not visible to EC2 for a few seconds.
        const msg = (e as Error).message ?? '';
        if (attempt < retries && errName(e) === 'InvalidParameterValue' && /instance profile/i.test(msg)) {
          await this.clock.sleep(Math.min(2000 * 2 ** attempt, 15000));
          continue;
        }
        throw e;
      }
    }
  }

  async terminate(instanceIds: string[]): Promise<void> {
    if (instanceIds.length === 0) return;
    instanceIds.forEach(assertValidInstanceId);
    try {
      await this.clients.ec2.send(new TerminateInstancesCommand({ InstanceIds: instanceIds }));
    } catch (e) {
      if (errName(e) !== 'InvalidInstanceID.NotFound') throw e; // already gone: idempotent
    }
  }

  async listTagged(optimizationId?: OptimizationId): Promise<InstanceInfo[]> {
    // `*` and `?` are wildcards in EC2 filter values, so an unvalidated id could list other optimizations.
    if (optimizationId !== undefined) assertValidId('optimization', optimizationId);
    const filters = optimizationId
      ? [{ Name: `tag:${TAG_OPT}`, Values: [optimizationId] }]
      : [{ Name: 'tag-key', Values: [TAG_OPT] }];
    const out: InstanceInfo[] = [];
    let token: string | undefined;
    do {
      const r = await this.clients.ec2.send(new DescribeInstancesCommand({ Filters: filters, NextToken: token }));
      for (const res of r.Reservations ?? []) for (const i of res.Instances ?? []) {
        const info = toInstanceInfo(i);
        if (info) out.push(info);
      }
      token = r.NextToken;
    } while (token);
    return out;
  }

  // ---- remote commands ----------------------------------------------------

  async runCommand(instanceId: string, command: RemoteCommand): Promise<RemoteCommandResult> {
    assertValidInstanceId(instanceId);
    const timeout = command.timeoutSeconds ?? 600;
    const script = [
      ...Object.entries(command.env ?? {}).map(([k, v]) => {
        if (!ENV_KEY.test(k)) throw new Error(`invalid env var name ${JSON.stringify(k)}`);
        return `export ${k}=${shQuote(v)}`;
      }),
      command.script,
    ].join('\n');
    const { ssm } = this.clients;
    const sent = await ssm.send(
      new SendCommandCommand({
        InstanceIds: [instanceId],
        DocumentName: 'AWS-RunShellScript',
        Parameters: { commands: [script], executionTimeout: [String(timeout)] },
        TimeoutSeconds: Math.max(30, timeout),
        Comment: 'ratchet',
      }),
    );
    const commandId = sent.Command?.CommandId;
    if (!commandId) throw new Error('SendCommand returned no CommandId');

    const interval = this.opts.pollIntervalMs ?? 2000;
    const deadline = this.clock.now() + (timeout + 60) * 1000;
    for (;;) {
      await this.clock.sleep(interval);
      let inv;
      try {
        inv = await ssm.send(new GetCommandInvocationCommand({ CommandId: commandId, InstanceId: instanceId }));
      } catch (e) {
        if (errName(e) !== 'InvocationDoesNotExist') throw e; // not registered yet
        if (this.clock.now() > deadline) throw new Error(`SSM command ${commandId} never registered on ${instanceId}`);
        continue;
      }
      const status = inv.Status ?? 'Pending';
      if (TERMINAL_SSM.has(status)) {
        const timedOut = status.includes('TimedOut');
        return {
          exitCode: timedOut ? 124 : (inv.ResponseCode ?? (status === 'Success' ? 0 : 1)),
          stdout: inv.StandardOutputContent ?? '',
          stderr: (inv.StandardErrorContent ?? '') + (status === 'Success' ? '' : `${inv.StandardErrorContent ? '\n' : ''}[ssm status: ${status}]`),
        };
      }
      if (this.clock.now() > deadline) throw new Error(`SSM command ${commandId} on ${instanceId} still ${status} after ${timeout + 60}s`);
    }
  }
}

// ---- helpers ----------------------------------------------------------------

export function agentRoleName(opt: string, agent: string): string {
  assertValidId('optimization', opt);
  assertValidId('agent', agent);
  return boundedName(`ratchet-agent-${opt}-${agent}`);
}

export function secretPrefix(opt: string, agent: string): string {
  assertValidId('optimization', opt);
  assertValidId('agent', agent);
  return `/ratchet/${opt}/${agent}/`;
}

/** Picks the instance profile for a bench phase; the sealed-capable one only for `confirm`. */
export function benchProfileFor(cfg: Pick<AwsComputeConfig, 'benchInstanceProfile' | 'confirmInstanceProfile'>, purpose: BenchLaunchSpec['purpose']): string {
  switch (purpose) {
    case 'baseline':
    case 'bench':
      return cfg.benchInstanceProfile;
    case 'confirm':
      if (!cfg.confirmInstanceProfile) {
        throw new Error('confirmInstanceProfile is not configured (confirm_identity_ref): refusing to run a confirm job under the bench identity');
      }
      return cfg.confirmInstanceProfile;
    default:
      throw new Error(`unknown bench purpose ${JSON.stringify(purpose)}`);
  }
}

function ignoreCodes(...names: string[]) {
  return (e: unknown): undefined => {
    if (names.some((n) => errName(e).startsWith(n))) return undefined;
    throw e;
  };
}

async function tolerate<T>(code: string, fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await fn();
  } catch (e) {
    if (errName(e).startsWith(code)) return undefined;
    throw e;
  }
}

const STATE: Record<string, InstanceInfo['state']> = {
  pending: 'pending',
  running: 'running',
  'shutting-down': 'stopping',
  stopping: 'stopping',
  stopped: 'stopped',
  terminated: 'terminated',
};

export function toInstanceInfo(i: Instance): InstanceInfo | null {
  const tags = new Map((i.Tags ?? []).map((t) => [t.Key ?? '', t.Value ?? '']));
  const opt = tags.get(TAG_OPT);
  if (!i.InstanceId || !opt) return null;
  const agent = tags.get(TAG_AGENT);
  const tagged = tags.get(TAG_ROLE);
  // Agent-launched experiments carry only opt+agent tags (the boundary forces just those).
  const role: InstanceRole = tagged === 'agent_runner' || tagged === 'bench' || tagged === 'experiment' ? tagged : agent ? 'experiment' : 'bench';
  const state = STATE[i.State?.Name ?? ''] ?? 'pending';
  const info: InstanceInfo = {
    id: i.InstanceId,
    role,
    optimizationId: opt,
    ...(agent ? { agentId: agent } : {}),
    instanceType: i.InstanceType ?? 'unknown',
    state,
    launchedAt: i.LaunchTime?.getTime() ?? 0,
  };
  if (state === 'terminated' || state === 'stopped') {
    // StateTransitionReason: "User initiated (2026-01-02 03:04:05 GMT)"
    const m = /\((\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}) GMT\)/.exec(i.StateTransitionReason ?? '');
    if (m) info.terminatedAt = Date.parse(`${m[1]}T${m[2]}Z`);
  }
  return info;
}

/** Runtime agent roles still present under the agent path (teardown guard for TofuSandboxProvider). */
export async function listAgentRoles(iam: IAMClient, agentRolePath = DEFAULT_AGENT_ROLE_PATH): Promise<string[]> {
  const names: string[] = [];
  let marker: string | undefined;
  do {
    const r = await iam.send(new ListRolesCommand({ PathPrefix: agentRolePath, Marker: marker }));
    for (const role of r.Roles ?? []) if (role.RoleName) names.push(role.RoleName);
    marker = r.IsTruncated ? r.Marker : undefined;
  } while (marker);
  return names;
}

const COMMON_SCRIPT_HEAD = '#!/bin/bash\nset -euo pipefail\n';

function lifetimeGuard(hours: number): string {
  // Local backstop to the reaper: shutdown terminates (InstanceInitiatedShutdownBehavior=terminate).
  return `shutdown -h +${Math.max(1, Math.round(hours * 60))} >/dev/null 2>&1 || true\n`;
}

function envFile(vars: Record<string, string>): string {
  const lines = Object.entries(vars).map(([k, v]) => `${k}=${shQuote(v)}`);
  return `install -d -m 700 /etc/ratchet\numask 077\ncat > /etc/ratchet/runner.env <<'RATCHET_ENV_EOF'\n${lines.join('\n')}\nRATCHET_ENV_EOF\n`;
}

export interface RunnerNetworkOptions {
  region: string;
  /** Egress proxy URL (`http://ip:3128`); null/undefined = direct HTTPS. */
  proxyUrl?: string | null;
}

/** Service hostnames reached through the VPC interface/gateway endpoints, never through the proxy. */
function noProxyList(region: string): string {
  return [
    'localhost',
    '127.0.0.1',
    '169.254.169.254',
    `ssm.${region}.amazonaws.com`,
    `ssmmessages.${region}.amazonaws.com`,
    `ec2messages.${region}.amazonaws.com`,
    `logs.${region}.amazonaws.com`,
    `s3.${region}.amazonaws.com`,
    `.s3.${region}.amazonaws.com`,
  ].join(',');
}

/** Starts `ratchet runner`. Secret VALUES never appear here: only SSM refs, fetched with the instance profile. */
export function runnerUserData(spec: RunnerLaunchSpec, maxLifetimeHours: number, net?: RunnerNetworkOptions): string {
  // Comma-separated: the format runner-main's parseSecretRefs reads. Neither env names nor SSM parameter names allow commas.
  for (const s of spec.secrets) {
    if (!/^[A-Z_][A-Z0-9_]*$/.test(s.name) || !/^[A-Za-z0-9_.\-/]+$/.test(s.ref)) throw new Error(`invalid secret ref ${JSON.stringify(`${s.name}=${s.ref}`)}`);
  }
  const refs = spec.secrets.map((s) => `${s.name}=${s.ref}`).join(',');
  const proxy: Record<string, string> = net?.proxyUrl
    ? {
        HTTPS_PROXY: net.proxyUrl,
        https_proxy: net.proxyUrl,
        HTTP_PROXY: net.proxyUrl,
        http_proxy: net.proxyUrl,
        NO_PROXY: noProxyList(net.region),
        no_proxy: noProxyList(net.region),
        // Node's built-in fetch ignores the variables above unless this is set (the agent CLIs are Node programs).
        NODE_USE_ENV_PROXY: '1',
      }
    : {};
  if (net?.proxyUrl && !/^http:\/\/[0-9.]+:\d+$/.test(net.proxyUrl)) throw new Error(`invalid egress proxy url ${JSON.stringify(net.proxyUrl)}`);
  return (
    COMMON_SCRIPT_HEAD +
    lifetimeGuard(maxLifetimeHours) +
    envFile({
      RATCHET_OPTIMIZATION_ID: spec.optimizationId,
      RATCHET_AGENT_ID: spec.agentId,
      RATCHET_AGENT_KIND: spec.kind,
      RATCHET_REPO_URL: spec.repoUrl,
      RATCHET_BRANCH: spec.branch,
      RATCHET_SECRET_REFS: refs,
      ...proxy,
    }) +
    'set -a; . /etc/ratchet/runner.env; set +a\n' +
    // Login shells (SSM sessions, agents spawning bash) get the proxy too.
    (net?.proxyUrl
      ? "grep -E '^(HTTPS?_PROXY|https?_proxy|NO_PROXY|no_proxy|NODE_USE_ENV_PROXY)=' /etc/ratchet/runner.env | sed 's/^/export /' > /etc/profile.d/ratchet-proxy.sh\nchmod 644 /etc/profile.d/ratchet-proxy.sh\n"
      : '') +
    'nohup ratchet runner >/var/log/ratchet-runner.log 2>&1 &\n'
  );
}

/** Unprivileged OS user that runs setup, guardrails and every process of the candidate. Must match remote-scripts `BENCH_USER`. */
export const BENCH_OS_USER = 'ratchet-bench';
/** Unprivileged OS user of the evaluator harness: the only one that can open the holdout (mode 0600). Must match remote-scripts `HOLDOUT_USER`. */
export const HOLDOUT_OS_USER = 'ratchet-holdout';
/** What the harness runs to start a candidate process as BENCH_OS_USER. Must match remote-scripts `CANDIDATE_LAUNCHER`. */
export const CANDIDATE_LAUNCHER_PATH = '/usr/local/bin/ratchet-candidate';
/** Runs as BENCH_OS_USER, only reachable through the sudo rule below. */
export const CANDIDATE_EXEC_PATH = '/usr/local/sbin/ratchet-candidate-exec';
const CANDIDATE_SUDOERS_PATH = '/etc/sudoers.d/ratchet-holdout';
/** Variables sudo keeps for the launcher (set by the root-run bench scripts before the harness starts). */
export const CANDIDATE_ENV_KEEP = ['RATCHET_CANDIDATE_DIR', 'RATCHET_CANDIDATE_HOME', 'RATCHET_CANDIDATE_TMP', 'RATCHET_CANDIDATE_PATH'] as const;

/**
 * `/usr/local/sbin/ratchet-candidate-exec`: runs ONE command as `ratchet-bench` in the candidate's checkout with
 * a clean environment (`env -i`): no `RATCHET_TRACE`, no `AWS_*`, nothing the harness (a different user) holds.
 * Leading `NAME=value` arguments become the candidate's environment, except the holdout and credentials, which
 * are refused. stdin, stdout and stderr are the caller's.
 */
export const CANDIDATE_EXEC_SCRIPT = `#!/bin/bash
set -eu
[ "$(id -un)" = ${BENCH_OS_USER} ] || { echo "ratchet-candidate-exec must run as ${BENCH_OS_USER}" >&2; exit 77; }
: "\${RATCHET_CANDIDATE_DIR:?}" "\${RATCHET_CANDIDATE_HOME:?}" "\${RATCHET_CANDIDATE_TMP:?}" "\${RATCHET_CANDIDATE_PATH:?}"
for a in "$@"; do
  case "$a" in
    [A-Za-z_]*=*)
      case "$a" in
        RATCHET_TRACE=*|AWS_*=*) echo "refusing to pass \${a%%=*} to candidate code" >&2; exit 78 ;;
      esac ;;
    *) break ;;
  esac
done
cd "$RATCHET_CANDIDATE_DIR"
exec env -i PATH="$RATCHET_CANDIDATE_PATH" HOME="$RATCHET_CANDIDATE_HOME" TMPDIR="$RATCHET_CANDIDATE_TMP" USER=${BENCH_OS_USER} LOGNAME=${BENCH_OS_USER} LANG=C.UTF-8 "$@"
`;

/** `/usr/local/bin/ratchet-candidate`: what the harness calls (`$RATCHET_CANDIDATE_EXEC`). */
export const CANDIDATE_LAUNCHER_SCRIPT = `#!/bin/sh
exec sudo -n -u ${BENCH_OS_USER} ${CANDIDATE_EXEC_PATH} "$@"
`;

/**
 * The only privilege the harness user has: run the launcher script as the bench user. `!use_pty` so stdin/stdout
 * pipes between harness and candidate stay pipes; `env_keep` carries the job's checkout location through sudo.
 */
export const CANDIDATE_SUDOERS = `Defaults:${HOLDOUT_OS_USER} !requiretty, !use_pty, !lecture, env_keep += "${CANDIDATE_ENV_KEEP.join(' ')}"
${HOLDOUT_OS_USER} ALL=(${BENCH_OS_USER}) NOPASSWD: ${CANDIDATE_EXEC_PATH}
`;

/**
 * Two unprivileged users (the bench runner's checkout step verifies both, the IMDS block for both, and the
 * launcher, and refuses to run the candidate otherwise, so a failure here cannot silently weaken the bench):
 *
 *  - `ratchet-bench` runs setup, guardrails and every candidate process; it can never open the holdout.
 *  - `ratchet-holdout` runs the evaluator harness (trusted code from bench.ref) and owns the holdout file
 *    (mode 0600). Its one privilege is a sudo rule that starts a candidate process as `ratchet-bench`.
 *
 * Neither user may read instance metadata: an iptables owner match rejects 169.254.169.254 for both uids, so
 * neither can fetch the instance-profile credentials (IMDSv2 with hop limit 1 is also set at launch). `hidepid=2`
 * hides each user's processes from the other. The launcher and the sudo rule are installed here, root-owned,
 * before the bench agent starts.
 */
export function benchUserData(spec: BenchLaunchSpec, maxLifetimeHours: number): string {
  const users = [BENCH_OS_USER, HOLDOUT_OS_USER];
  return (
    COMMON_SCRIPT_HEAD +
    lifetimeGuard(maxLifetimeHours) +
    users.map((u) => `id -u ${u} >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin ${u}\n`).join('') +
    users.map((u) => `iptables -I OUTPUT 1 -m owner --uid-owner ${u} -d 169.254.169.254 -j REJECT\n`).join('') +
    users.map((u) => `command -v ip6tables >/dev/null 2>&1 && ip6tables -I OUTPUT 1 -m owner --uid-owner ${u} -d fd00:ec2::254 -j REJECT || true\n`).join('') +
    'mount -o remount,hidepid=2 /proc >/dev/null 2>&1 || true\n' +
    heredocInstall(CANDIDATE_EXEC_PATH, CANDIDATE_EXEC_SCRIPT, '755', 'RATCHET_CANDIDATE_EXEC_EOF') +
    heredocInstall(CANDIDATE_LAUNCHER_PATH, CANDIDATE_LAUNCHER_SCRIPT, '755', 'RATCHET_CANDIDATE_LAUNCHER_EOF') +
    heredocInstall(`${CANDIDATE_SUDOERS_PATH}.tmp`, CANDIDATE_SUDOERS, '440', 'RATCHET_SUDOERS_EOF') +
    `visudo -cf ${CANDIDATE_SUDOERS_PATH}.tmp >/dev/null || { rm -f ${CANDIDATE_SUDOERS_PATH}.tmp; echo "invalid sudoers rule for ${HOLDOUT_OS_USER}" >&2; exit 1; }\n` +
    `mv ${CANDIDATE_SUDOERS_PATH}.tmp ${CANDIDATE_SUDOERS_PATH}\n` +
    envFile({ RATCHET_OPTIMIZATION_ID: spec.optimizationId, RATCHET_BENCH_PURPOSE: spec.purpose }) +
    'set -a; . /etc/ratchet/runner.env; set +a\n' +
    'nohup ratchet bench-agent >/var/log/ratchet-bench.log 2>&1 &\n'
  );
}

function heredocInstall(path: string, body: string, mode: string, delimiter: string): string {
  if (body.includes(delimiter)) throw new Error('script contains its heredoc delimiter');
  return `install -D -m ${mode} -o root -g root /dev/null ${path}\ncat > ${path} <<'${delimiter}'\n${body}${delimiter}\n`;
}
