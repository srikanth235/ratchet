import { describe, it, expect, beforeEach } from 'vitest';
import { mockClient } from 'aws-sdk-client-mock';
import {
  DescribeInstancesCommand,
  EC2Client,
  RunInstancesCommand,
  TerminateInstancesCommand,
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
import {
  AwsCloudCompute,
  KILL_SWITCH_POLICY_NAME,
  agentRoleName,
  awsComputeConfigFromOutputs,
  benchProfileFor,
  benchUserData,
  listAgentRoles,
  runnerUserData,
  secretPrefix,
  toInstanceInfo,
  type AwsComputeConfig,
} from './aws.js';
import { MockSandboxProvider } from '../sandbox/mock.js';
import { ManualClock } from '../util.js';

const cfg: AwsComputeConfig = {
  sandboxId: 'demo',
  region: 'us-east-1',
  agentBoundaryArn: 'arn:aws:iam::123456789012:policy/ratchet/ratchet-demo-agent-boundary',
  agentRolePath: '/ratchet/agent/',
  subnetId: 'subnet-agent',
  benchSubnetId: 'subnet-bench',
  agentRunnerSecurityGroupId: 'sg-runner',
  benchSecurityGroupId: 'sg-bench',
  benchInstanceProfile: 'arn:aws:iam::123456789012:instance-profile/ratchet/bench/ratchet-demo-bench-runner',
  confirmInstanceProfile: 'arn:aws:iam::123456789012:instance-profile/ratchet/bench/ratchet-demo-confirm-runner',
  artifactsBucket: 'ratchet-demo-artifacts',
  maxInstanceLifetimeHours: 2,
};

const ec2 = mockClient(EC2Client);
const iam = mockClient(IAMClient);
const ssm = mockClient(SSMClient);
const sts = mockClient(STSClient);
let clock: ManualClock;
let compute: AwsCloudCompute;

const err = (name: string, message = name) => Object.assign(new Error(message), { name });

beforeEach(() => {
  ec2.reset();
  iam.reset();
  ssm.reset();
  sts.reset();
  iam.onAnyCommand().resolves({});
  ssm.onAnyCommand().resolves({});
  ec2.onAnyCommand().resolves({});
  sts.on(GetCallerIdentityCommand).resolves({ Account: '123456789012', Arn: 'arn:aws:sts::123456789012:assumed-role/x/y' });
  clock = new ManualClock(1_000_000);
  compute = new AwsCloudCompute(cfg, {
    clients: { ec2: new EC2Client({}), iam: new IAMClient({}), ssm: new SSMClient({}), sts: new STSClient({}) },
    clock,
    pollIntervalMs: 1000,
  });
});

const identity = {
  optimizationId: 'opt1',
  agentId: 'a1',
  roleRef: 'arn:aws:iam::123456789012:role/ratchet/agent/ratchet-agent-opt1-a1',
  instanceProfileRef: 'arn:aws:iam::123456789012:instance-profile/ratchet/agent/ratchet-agent-opt1-a1',
};

// Ids are substituted into IAM role names, tag values, SSM paths, S3 prefixes and policy variables
// (`${aws:PrincipalTag/ratchet:opt}` inside Resource ARNs), where `*`, `?`, `/` and `..` would widen a pattern.
const INJECTION = ['*', '?', '/', '..', 'a*', 'a?b', 'a/b', '../x', 'opt1/../opt2', 'opt1/*', 'Opt', 'a b', '', 'a_b', 'a:b', 'a${b}', 'x'.repeat(65)];

describe('id validation (security requirement)', () => {
  const spec = { instanceType: 't3.medium', imageRef: 'ami-1' };
  const runner = (opt: string, agent: string) => ({
    ...spec,
    optimizationId: opt,
    agentId: agent,
    kind: 'claude-code' as const,
    repoUrl: 'https://github.com/o/r',
    branch: 'ratchet/opt/a',
    identity: { ...identity, optimizationId: opt, agentId: agent },
    secrets: [],
  });
  const bench = (opt: string, purpose: 'baseline' | 'bench' | 'confirm' = 'bench') => ({ ...spec, optimizationId: opt, purpose });
  const untouched = () => {
    expect(iam.calls()).toHaveLength(0);
    expect(ssm.calls()).toHaveLength(0);
    expect(ec2.calls()).toHaveLength(0);
  };

  it.each(INJECTION)('rejects %j at every id entry point', async (bad) => {
    await expect(compute.createAgentIdentity(bad, 'a1')).rejects.toThrow(/invalid optimization id/);
    await expect(compute.createAgentIdentity('opt1', bad)).rejects.toThrow(/invalid agent id/);
    await expect(compute.putSecret(bad, 'a1', 'K', 'v')).rejects.toThrow(/invalid optimization id/);
    await expect(compute.putSecret('opt1', bad, 'K', 'v')).rejects.toThrow(/invalid agent id/);
    await expect(compute.deleteSecrets(bad, 'a1')).rejects.toThrow(/invalid optimization id/);
    await expect(compute.deleteSecrets('opt1', bad)).rejects.toThrow(/invalid agent id/);
    await expect(compute.launchRunner(runner(bad, 'a1') as never)).rejects.toThrow(/invalid optimization id/);
    await expect(compute.launchRunner(runner('opt1', bad) as never)).rejects.toThrow(/invalid agent id/);
    await expect(compute.launchBenchInstance(bench(bad))).rejects.toThrow(/invalid optimization id/);
    await expect(compute.launchBenchInstance(bench(bad, 'confirm'))).rejects.toThrow(/invalid optimization id/);
    await expect(compute.listTagged(bad)).rejects.toThrow(/invalid optimization id/);
    expect(() => agentRoleName(bad, 'a1')).toThrow(/invalid optimization id/);
    expect(() => agentRoleName('opt1', bad)).toThrow(/invalid agent id/);
    expect(() => secretPrefix(bad, 'a1')).toThrow(/invalid optimization id/);
    expect(() => secretPrefix('opt1', bad)).toThrow(/invalid agent id/);
    untouched();
  });

  it.each(['*', '?', 'i-*', 'i-1/../i-2', '../x', '', 'i-', 'I-ABC'])('rejects instance id %j in terminate and runCommand', async (bad) => {
    await expect(compute.terminate([bad])).rejects.toThrow(/invalid instance id/);
    await expect(compute.terminate(['i-0ok', bad])).rejects.toThrow(/invalid instance id/);
    await expect(compute.runCommand(bad, { script: 'true' })).rejects.toThrow(/invalid instance id/);
    untouched();
  });

  it('rejects bad secret names, including "..", "/" and wildcards', async () => {
    for (const bad of ['../../x', '..', 'a/b', 'a*', 'a?', '', 'a b']) {
      await expect(compute.putSecret('opt1', 'a1', bad, 'v')).rejects.toThrow(/invalid secret name/);
    }
    untouched();
  });
  it('accepts well-formed ids', async () => {
    await expect(compute.putSecret('opt-1', 'claude-code-2', 'ANTHROPIC_API_KEY', 'v')).resolves.toBeDefined();
  });
  it('shortens very long role names but keeps them unique and <= 64 chars', () => {
    const a = agentRoleName('o'.repeat(60), 'a'.repeat(60));
    const b = agentRoleName('o'.repeat(60), 'a'.repeat(59) + 'b');
    expect(a.length).toBeLessThanOrEqual(64);
    expect(a).not.toBe(b);
  });
});

describe('createAgentIdentity', () => {
  it('creates role under path with boundary + tags, policy, profile', async () => {
    iam.on(CreateRoleCommand).resolves({ Role: { Arn: identity.roleRef } as never });
    iam.on(CreateInstanceProfileCommand).resolves({ InstanceProfile: { Arn: identity.instanceProfileRef } as never });
    const got = await compute.createAgentIdentity('opt1', 'a1');
    expect(got).toEqual(identity);

    const role = iam.commandCalls(CreateRoleCommand)[0]!.args[0].input;
    expect(role.RoleName).toBe('ratchet-agent-opt1-a1');
    expect(role.Path).toBe('/ratchet/agent/');
    expect(role.PermissionsBoundary).toBe(cfg.agentBoundaryArn);
    expect(role.Tags).toEqual(
      expect.arrayContaining([
        { Key: 'ratchet:opt', Value: 'opt1' },
        { Key: 'ratchet:agent', Value: 'a1' },
      ]),
    );
    const trust = JSON.parse(role.AssumeRolePolicyDocument!);
    expect(trust.Statement[0].Principal).toEqual({ Service: 'ec2.amazonaws.com' });

    const pol = iam.commandCalls(PutRolePolicyCommand)[0]!.args[0].input;
    expect(pol.RoleName).toBe('ratchet-agent-opt1-a1');
    const doc = JSON.parse(pol.PolicyDocument!);
    const s3 = doc.Statement.find((s: { Sid: string }) => s.Sid === 'ArtifactObjects');
    expect(s3.Resource).toBe('arn:aws:s3:::ratchet-demo-artifacts/artifacts/opt1/a1/*');
    const sec = doc.Statement.find((s: { Sid: string }) => s.Sid === 'OwnSecrets');
    expect(sec.Resource).toBe('arn:aws:ssm:us-east-1:123456789012:parameter/ratchet/opt1/a1/*');
    expect(JSON.stringify(doc)).not.toContain('iam:');

    const prof = iam.commandCalls(CreateInstanceProfileCommand)[0]!.args[0].input;
    expect(prof.InstanceProfileName).toBe('ratchet-agent-opt1-a1');
    expect(prof.Path).toBe('/ratchet/agent/');
    expect(iam.commandCalls(AddRoleToInstanceProfileCommand)[0]!.args[0].input).toEqual({
      InstanceProfileName: 'ratchet-agent-opt1-a1',
      RoleName: 'ratchet-agent-opt1-a1',
    });
  });

  it('is idempotent when role/profile already exist', async () => {
    iam.on(CreateRoleCommand).rejects(err('EntityAlreadyExistsException'));
    iam.on(CreateInstanceProfileCommand).rejects(err('EntityAlreadyExistsException'));
    iam.on(AddRoleToInstanceProfileCommand).rejects(err('LimitExceededException'));
    expect(await compute.createAgentIdentity('opt1', 'a1')).toEqual(identity);
  });

  it('propagates unexpected IAM errors', async () => {
    iam.on(CreateRoleCommand).rejects(err('AccessDenied'));
    await expect(compute.createAgentIdentity('opt1', 'a1')).rejects.toThrow('AccessDenied');
  });

  it('rejects a malformed role path', () => {
    expect(() => new AwsCloudCompute({ ...cfg, agentRolePath: 'ratchet/agent' }, { clients: {} })).toThrow(/agentRolePath/);
  });
});

describe('kill switch + delete', () => {
  it('revoke attaches an inline deny-all', async () => {
    iam.on(PutRolePolicyCommand).resolves({});
    await compute.revokeAgentIdentity(identity);
    const i = iam.commandCalls(PutRolePolicyCommand)[0]!.args[0].input;
    expect(i.RoleName).toBe('ratchet-agent-opt1-a1');
    expect(i.PolicyName).toBe(KILL_SWITCH_POLICY_NAME);
    expect(JSON.parse(i.PolicyDocument!).Statement).toEqual([{ Sid: 'KillSwitchDenyAll', Effect: 'Deny', Action: '*', Resource: '*' }]);
  });

  it('deletes in dependency order: profile role-removal, profile, policies, role', async () => {
    iam.on(ListRolePoliciesCommand).resolves({ PolicyNames: ['ratchet-agent-permissions', KILL_SWITCH_POLICY_NAME] });
    iam.on(ListAttachedRolePoliciesCommand).resolves({ AttachedPolicies: [{ PolicyArn: 'arn:aws:iam::aws:policy/Foo' }] });
    await compute.deleteAgentIdentity(identity);
    const order = iam.calls().map((c) => c.args[0].constructor.name);
    expect(order).toEqual([
      'RemoveRoleFromInstanceProfileCommand',
      'DeleteInstanceProfileCommand',
      'ListRolePoliciesCommand',
      'DeleteRolePolicyCommand',
      'DeleteRolePolicyCommand',
      'ListAttachedRolePoliciesCommand',
      'DetachRolePolicyCommand',
      'DeleteRoleCommand',
    ]);
    expect(iam.commandCalls(RemoveRoleFromInstanceProfileCommand)[0]!.args[0].input).toEqual({
      InstanceProfileName: 'ratchet-agent-opt1-a1',
      RoleName: 'ratchet-agent-opt1-a1',
    });
    expect(iam.commandCalls(DeleteInstanceProfileCommand)[0]!.args[0].input).toEqual({ InstanceProfileName: 'ratchet-agent-opt1-a1' });
    expect(iam.commandCalls(DeleteRolePolicyCommand).map((c) => c.args[0].input.PolicyName)).toEqual(['ratchet-agent-permissions', KILL_SWITCH_POLICY_NAME]);
    expect(iam.commandCalls(DetachRolePolicyCommand)[0]!.args[0].input.PolicyArn).toBe('arn:aws:iam::aws:policy/Foo');
    expect(iam.commandCalls(DeleteRoleCommand)[0]!.args[0].input).toEqual({ RoleName: 'ratchet-agent-opt1-a1' });
  });

  it('is idempotent when everything is already gone', async () => {
    iam.rejects(err('NoSuchEntityException'));
    await expect(compute.deleteAgentIdentity(identity)).resolves.toBeUndefined();
  });

  it('does not swallow other errors', async () => {
    iam.on(RemoveRoleFromInstanceProfileCommand).rejects(err('AccessDenied'));
    await expect(compute.deleteAgentIdentity(identity)).rejects.toThrow('AccessDenied');
  });

  it('listAgentRoles pages through the agent path', async () => {
    iam
      .on(ListRolesCommand)
      .resolvesOnce({ Roles: [{ RoleName: 'r1' } as never], IsTruncated: true, Marker: 'm' })
      .resolvesOnce({ Roles: [{ RoleName: 'r2' } as never], IsTruncated: false });
    expect(await listAgentRoles(new IAMClient({}))).toEqual(['r1', 'r2']);
    const calls = iam.commandCalls(ListRolesCommand);
    expect(calls[0]!.args[0].input.PathPrefix).toBe('/ratchet/agent/');
    expect(calls[1]!.args[0].input.Marker).toBe('m');
  });
});

describe('secrets', () => {
  it('puts a SecureString under /ratchet/<opt>/<agent>/ and tags it', async () => {
    ssm.on(PutParameterCommand).resolves({});
    ssm.on(AddTagsToResourceCommand).resolves({});
    const ref = await compute.putSecret('opt1', 'a1', 'ANTHROPIC_API_KEY', 'sk-secret');
    expect(ref).toEqual({ name: 'ANTHROPIC_API_KEY', ref: '/ratchet/opt1/a1/ANTHROPIC_API_KEY' });
    expect(ssm.commandCalls(PutParameterCommand)[0]!.args[0].input).toEqual({
      Name: '/ratchet/opt1/a1/ANTHROPIC_API_KEY',
      Value: 'sk-secret',
      Type: 'SecureString',
      Overwrite: true,
    });
    expect(ssm.commandCalls(AddTagsToResourceCommand)[0]!.args[0].input).toEqual({
      ResourceType: 'Parameter',
      ResourceId: '/ratchet/opt1/a1/ANTHROPIC_API_KEY',
      Tags: [
        { Key: 'ratchet:opt', Value: 'opt1' },
        { Key: 'ratchet:agent', Value: 'a1' },
      ],
    });
  });

  it('deletes all secrets under the agent path, paging and batching by 10', async () => {
    const names = Array.from({ length: 12 }, (_, i) => ({ Name: `/ratchet/opt1/a1/K${i}` }));
    ssm
      .on(GetParametersByPathCommand)
      .resolvesOnce({ Parameters: names.slice(0, 7), NextToken: 't' })
      .resolvesOnce({ Parameters: names.slice(7) });
    ssm.on(DeleteParametersCommand).resolves({});
    await compute.deleteSecrets('opt1', 'a1');
    const g = ssm.commandCalls(GetParametersByPathCommand);
    expect(g[0]!.args[0].input).toMatchObject({ Path: '/ratchet/opt1/a1/', Recursive: true, WithDecryption: false });
    expect(g[1]!.args[0].input.NextToken).toBe('t');
    const d = ssm.commandCalls(DeleteParametersCommand).map((c) => c.args[0].input.Names!.length);
    expect(d).toEqual([10, 2]);
  });

  it('deleteSecrets with none present does not call Delete', async () => {
    ssm.on(GetParametersByPathCommand).resolves({ Parameters: [] });
    await compute.deleteSecrets('opt1', 'a1');
    expect(ssm.commandCalls(DeleteParametersCommand)).toHaveLength(0);
  });
});

describe('launch', () => {
  const runnerSpec = {
    optimizationId: 'opt1',
    agentId: 'a1',
    kind: 'claude-code' as const,
    instanceType: 'c7i.large',
    imageRef: 'ami-runner',
    identity,
    secrets: [{ name: 'ANTHROPIC_API_KEY', ref: '/ratchet/opt1/a1/ANTHROPIC_API_KEY' }],
    repoUrl: "https://github.com/o/r'; rm -rf / #.git",
    branch: 'ratchet/opt1/a1',
  };

  it('launchRunner: tags, IMDSv2, terminate-on-shutdown, profile, network, user-data', async () => {
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-1', LaunchTime: new Date(5000) }] });
    const info = await compute.launchRunner(runnerSpec);
    expect(info).toEqual({
      id: 'i-1',
      role: 'agent_runner',
      optimizationId: 'opt1',
      agentId: 'a1',
      instanceType: 'c7i.large',
      state: 'pending',
      launchedAt: 5000,
    });
    const input = ec2.commandCalls(RunInstancesCommand)[0]!.args[0].input;
    expect(input).toMatchObject({
      ImageId: 'ami-runner',
      InstanceType: 'c7i.large',
      MinCount: 1,
      MaxCount: 1,
      SubnetId: 'subnet-agent',
      SecurityGroupIds: ['sg-runner'],
      IamInstanceProfile: { Arn: identity.instanceProfileRef },
      MetadataOptions: { HttpTokens: 'required', HttpEndpoint: 'enabled', HttpPutResponseHopLimit: 1 },
      InstanceInitiatedShutdownBehavior: 'terminate',
    });
    const instTags = input.TagSpecifications!.find((t) => t.ResourceType === 'instance')!.Tags!;
    expect(instTags).toEqual(
      expect.arrayContaining([
        { Key: 'ratchet:opt', Value: 'opt1' },
        { Key: 'ratchet:agent', Value: 'a1' },
        { Key: 'ratchet:role', Value: 'agent_runner' },
      ]),
    );
    const volTags = input.TagSpecifications!.find((t) => t.ResourceType === 'volume')!.Tags!;
    expect(volTags).toEqual(expect.arrayContaining([{ Key: 'ratchet:opt', Value: 'opt1' }, { Key: 'ratchet:agent', Value: 'a1' }]));

    const ud = Buffer.from(input.UserData!, 'base64').toString();
    expect(ud).toContain('ratchet runner');
    expect(ud).toContain('shutdown -h +120');
    expect(ud).toContain('RATCHET_SECRET_REFS=');
    expect(ud).toContain('/ratchet/opt1/a1/ANTHROPIC_API_KEY');
    // hostile repo url is single-quoted, not executable
    expect(ud).toContain(`RATCHET_REPO_URL='https://github.com/o/r'\\''; rm -rf / #.git'`);
  });

  const benchSpec = (purpose: 'baseline' | 'bench' | 'confirm') => ({ optimizationId: 'opt1', instanceType: 'c7i.2xlarge', imageRef: 'ami-bench', purpose });

  it('launchBenchInstance (confirm) uses the confirm profile, the private bench subnet + bench SG, no agent tag', async () => {
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-b' }] });
    const info = await compute.launchBenchInstance(benchSpec('confirm'));
    expect(info.role).toBe('bench');
    expect(info.agentId).toBeUndefined();
    expect(info.launchedAt).toBe(1_000_000); // clock fallback when LaunchTime is absent
    const input = ec2.commandCalls(RunInstancesCommand)[0]!.args[0].input;
    expect(input.IamInstanceProfile).toEqual({ Arn: cfg.confirmInstanceProfile });
    expect(input.SubnetId).toBe('subnet-bench');
    expect(input.SecurityGroupIds).toEqual(['sg-bench']);
    expect(input.MetadataOptions).toEqual({ HttpTokens: 'required', HttpEndpoint: 'enabled', HttpPutResponseHopLimit: 1 });
    expect(input.InstanceInitiatedShutdownBehavior).toBe('terminate');
    const tags = input.TagSpecifications![0]!.Tags!;
    expect(tags).toEqual(expect.arrayContaining([{ Key: 'ratchet:opt', Value: 'opt1' }, { Key: 'ratchet:role', Value: 'bench' }, { Key: 'ratchet:purpose', Value: 'confirm' }]));
    expect(tags.find((t) => t.Key === 'ratchet:agent')).toBeUndefined();
    expect(Buffer.from(input.UserData!, 'base64').toString()).toContain('ratchet bench-agent');
  });

  it.each(['baseline', 'bench'] as const)('launchBenchInstance (%s) never gets the sealed-capable profile', async (purpose) => {
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-b' }] });
    await compute.launchBenchInstance(benchSpec(purpose));
    const input = ec2.commandCalls(RunInstancesCommand)[0]!.args[0].input;
    expect(input.IamInstanceProfile).toEqual({ Arn: cfg.benchInstanceProfile });
    expect(JSON.stringify(input)).not.toContain('confirm-runner');
    expect(input.SubnetId).toBe('subnet-bench');
  });

  it('picks the profile by purpose, and fails closed for anything else', () => {
    expect(benchProfileFor(cfg, 'baseline')).toBe(cfg.benchInstanceProfile);
    expect(benchProfileFor(cfg, 'bench')).toBe(cfg.benchInstanceProfile);
    expect(benchProfileFor(cfg, 'confirm')).toBe(cfg.confirmInstanceProfile);
    expect(() => benchProfileFor(cfg, 'confirm-ish' as never)).toThrow(/unknown bench purpose/);
    expect(() => benchProfileFor({ benchInstanceProfile: 'b' }, 'confirm')).toThrow(/confirmInstanceProfile is not configured/);
  });

  it('refuses a confirm launch without a confirm profile instead of falling back to the bench one', async () => {
    const { confirmInstanceProfile: _drop, ...noConfirm } = cfg;
    const c = new AwsCloudCompute(noConfirm, { clients: { ec2: new EC2Client({}) }, clock });
    await expect(c.launchBenchInstance(benchSpec('confirm'))).rejects.toThrow(/confirmInstanceProfile is not configured/);
    expect(ec2.commandCalls(RunInstancesCommand)).toHaveLength(0);
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-b' }] });
    await expect(c.launchBenchInstance(benchSpec('bench'))).resolves.toBeDefined(); // bench phases need no sealed identity
  });

  it('refuses to share one profile between bench and confirm', () => {
    expect(() => new AwsCloudCompute({ ...cfg, confirmInstanceProfile: cfg.benchInstanceProfile }, { clients: {} })).toThrow(/must differ/);
  });

  it('refuses to launch bench instances without a bench subnet (no fallback to the agent subnet)', async () => {
    const { benchSubnetId: _drop, ...noBench } = cfg;
    const c = new AwsCloudCompute(noBench, { clients: { ec2: new EC2Client({}) }, clock });
    await expect(c.launchBenchInstance(benchSpec('bench'))).rejects.toThrow(/benchSubnetId is not configured/);
    expect(ec2.commandCalls(RunInstancesCommand)).toHaveLength(0);
  });

  it('launches agent runners in the agent subnet', async () => {
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-r' }] });
    await compute.launchRunner(runnerSpec);
    expect(ec2.commandCalls(RunInstancesCommand)[0]!.args[0].input.SubnetId).toBe('subnet-agent');
  });

  it('accepts a bare profile name for bench', async () => {
    const c = new AwsCloudCompute({ ...cfg, benchInstanceProfile: 'bench-profile' }, { clients: { ec2: new EC2Client({}) }, clock });
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-b' }] });
    await c.launchBenchInstance({ optimizationId: 'opt1', instanceType: 'c7i.2xlarge', imageRef: 'ami', purpose: 'bench' });
    expect(ec2.commandCalls(RunInstancesCommand)[0]!.args[0].input.IamInstanceProfile).toEqual({ Name: 'bench-profile' });
  });

  it('bench user-data creates the unprivileged user and blocks its access to IMDS', () => {
    const ud = benchUserData({ optimizationId: 'opt1', instanceType: 'c7i.large', imageRef: 'ami', purpose: 'confirm' }, 2);
    expect(ud).toContain('useradd --system');
    expect(ud).toContain('ratchet-bench');
    expect(ud).toContain('iptables -I OUTPUT 1 -m owner --uid-owner ratchet-bench -d 169.254.169.254 -j REJECT');
    // the rule is installed before the bench agent starts
    expect(ud.indexOf('iptables')).toBeLessThan(ud.indexOf('ratchet bench-agent'));
  });

  it('runner user-data exports the proxy only when one is configured', () => {
    const none = runnerUserData(runnerSpec, 2, { region: 'us-east-1' });
    expect(none).not.toMatch(/HTTPS_PROXY|profile\.d/);
    const ud = runnerUserData(runnerSpec, 2, { region: 'us-east-1', proxyUrl: 'http://10.42.3.10:3128' });
    expect(ud).toContain("HTTPS_PROXY='http://10.42.3.10:3128'");
    expect(ud).toContain("https_proxy='http://10.42.3.10:3128'");
    // AWS service endpoints and IMDS bypass the proxy
    expect(ud).toMatch(/NO_PROXY='[^']*169\.254\.169\.254[^']*ssm\.us-east-1\.amazonaws\.com[^']*'/);
    expect(ud).toContain('/etc/profile.d/ratchet-proxy.sh');
    expect(() => runnerUserData(runnerSpec, 2, { region: 'us-east-1', proxyUrl: "http://x:1'; curl evil #" })).toThrow(/invalid egress proxy url/);
  });

  it('launchRunner passes the configured proxy into user-data', async () => {
    ec2.on(RunInstancesCommand).resolves({ Instances: [{ InstanceId: 'i-r' }] });
    const c = new AwsCloudCompute({ ...cfg, egressProxyUrl: 'http://10.42.3.10:3128' }, { clients: { ec2: new EC2Client({}) }, clock });
    await c.launchRunner(runnerSpec);
    const ud = Buffer.from(ec2.commandCalls(RunInstancesCommand)[0]!.args[0].input.UserData!, 'base64').toString();
    expect(ud).toContain("HTTPS_PROXY='http://10.42.3.10:3128'");
  });

  it('derives the compute config from sandbox outputs: bench profile for bench, confirm profile for confirm', async () => {
    const out = await new MockSandboxProvider().apply({ sandboxId: 'sim', region: 'us-east-1', allowedInstanceTypes: ['t3.medium'], maxInstancesPerAgent: 4, maxInstanceLifetimeHours: 2 } as never);
    const derived = awsComputeConfigFromOutputs(out, 'sim');
    expect(derived.benchInstanceProfile).toBe(out.evalIdentity.instanceProfileArn);
    expect(derived.confirmInstanceProfile).toBe(out.confirmIdentity.instanceProfileArn);
    expect(derived.benchInstanceProfile).not.toBe(derived.confirmInstanceProfile);
    expect(derived.benchSubnetId).toBe(out.network.benchSubnetId);
    expect(derived.benchSubnetId).not.toBe(derived.subnetId);
  });

  it('retries while the fresh instance profile propagates, then succeeds', async () => {
    ec2
      .on(RunInstancesCommand)
      .rejectsOnce(err('InvalidParameterValue', 'Value (x) for parameter iamInstanceProfile.arn is invalid. Invalid IAM Instance Profile ARN'))
      .rejectsOnce(err('InvalidParameterValue', 'Invalid IAM Instance Profile name'))
      .resolves({ Instances: [{ InstanceId: 'i-2' }] });
    const t0 = clock.now();
    const info = await compute.launchRunner(runnerSpec);
    expect(info.id).toBe('i-2');
    expect(ec2.commandCalls(RunInstancesCommand)).toHaveLength(3);
    expect(clock.now() - t0).toBe(2000 + 4000);
  });

  it('does not retry unrelated errors, and gives up after the retry budget', async () => {
    ec2.on(RunInstancesCommand).rejects(err('UnauthorizedOperation'));
    await expect(compute.launchRunner(runnerSpec)).rejects.toThrow('UnauthorizedOperation');
    expect(ec2.commandCalls(RunInstancesCommand)).toHaveLength(1);

    ec2.reset();
    ec2.on(RunInstancesCommand).rejects(err('InvalidParameterValue', 'Invalid IAM Instance Profile'));
    await expect(compute.launchRunner(runnerSpec)).rejects.toThrow('Invalid IAM Instance Profile');
    expect(ec2.commandCalls(RunInstancesCommand)).toHaveLength(7);
  });

  it('refuses invalid ids before calling AWS', async () => {
    await expect(compute.launchRunner({ ...runnerSpec, agentId: 'A B' })).rejects.toThrow(/invalid agent id/);
    expect(ec2.calls()).toHaveLength(0);
  });
});

describe('terminate + listTagged', () => {
  it('terminates, no-ops on empty, tolerates already-gone ids', async () => {
    ec2.on(TerminateInstancesCommand).resolves({});
    await compute.terminate([]);
    expect(ec2.calls()).toHaveLength(0);
    await compute.terminate(['i-1', 'i-2']);
    expect(ec2.commandCalls(TerminateInstancesCommand)[0]!.args[0].input).toEqual({ InstanceIds: ['i-1', 'i-2'] });
    ec2.reset();
    ec2.on(TerminateInstancesCommand).rejects(err('InvalidInstanceID.NotFound'));
    await expect(compute.terminate(['i-9'])).resolves.toBeUndefined();
    ec2.reset();
    ec2.on(TerminateInstancesCommand).rejects(err('UnauthorizedOperation'));
    await expect(compute.terminate(['i-9'])).rejects.toThrow();
  });

  const inst = (id: string, tags: Record<string, string>, state = 'running', extra: object = {}) => ({
    InstanceId: id,
    InstanceType: 'c7i.large',
    LaunchTime: new Date(1000),
    State: { Name: state },
    Tags: Object.entries(tags).map(([Key, Value]) => ({ Key, Value })),
    ...extra,
  });

  it('filters by tag, pages, infers roles, and skips untagged', async () => {
    ec2
      .on(DescribeInstancesCommand)
      .resolvesOnce({
        Reservations: [{ Instances: [inst('i-r', { 'ratchet:opt': 'o', 'ratchet:agent': 'a', 'ratchet:role': 'agent_runner' }), inst('i-x', { Name: 'foreign' })] as never }],
        NextToken: 'n',
      })
      .resolvesOnce({
        Reservations: [
          {
            Instances: [
              inst('i-e', { 'ratchet:opt': 'o', 'ratchet:agent': 'a' }),
              inst('i-b', { 'ratchet:opt': 'o', 'ratchet:role': 'bench' }, 'shutting-down'),
              inst('i-t', { 'ratchet:opt': 'o', 'ratchet:agent': 'a' }, 'terminated', { StateTransitionReason: 'User initiated (2026-01-02 03:04:05 GMT)' }),
            ] as never,
          },
        ],
      });
    const list = await compute.listTagged('o');
    const calls = ec2.commandCalls(DescribeInstancesCommand);
    expect(calls[0]!.args[0].input.Filters).toEqual([{ Name: 'tag:ratchet:opt', Values: ['o'] }]);
    expect(calls[1]!.args[0].input.NextToken).toBe('n');
    expect(list.map((i) => [i.id, i.role, i.state])).toEqual([
      ['i-r', 'agent_runner', 'running'],
      ['i-e', 'experiment', 'running'],
      ['i-b', 'bench', 'stopping'],
      ['i-t', 'experiment', 'terminated'],
    ]);
    expect(list[3]!.terminatedAt).toBe(Date.parse('2026-01-02T03:04:05Z'));
    expect(list[0]!.agentId).toBe('a');
    expect(list[2]!.agentId).toBeUndefined();
  });

  it('without an optimization uses a tag-key filter', async () => {
    ec2.on(DescribeInstancesCommand).resolves({ Reservations: [] });
    expect(await compute.listTagged()).toEqual([]);
    expect(ec2.commandCalls(DescribeInstancesCommand)[0]!.args[0].input.Filters).toEqual([{ Name: 'tag-key', Values: ['ratchet:opt'] }]);
  });

  it('toInstanceInfo ignores instances without ratchet:opt', () => {
    expect(toInstanceInfo({ InstanceId: 'i', Tags: [] })).toBeNull();
  });
});

describe('runCommand', () => {
  it('sends AWS-RunShellScript with env exports and polls to completion', async () => {
    ssm.on(SendCommandCommand).resolves({ Command: { CommandId: 'c-1' } });
    ssm
      .on(GetCommandInvocationCommand)
      .rejectsOnce(err('InvocationDoesNotExist'))
      .resolvesOnce({ Status: 'InProgress' })
      .resolvesOnce({ Status: 'Success', ResponseCode: 0, StandardOutputContent: 'hi\n', StandardErrorContent: '' });
    const r = await compute.runCommand('i-1', { script: 'echo $X', env: { X: "a'b" }, timeoutSeconds: 120 });
    expect(r).toEqual({ exitCode: 0, stdout: 'hi\n', stderr: '' });
    const s = ssm.commandCalls(SendCommandCommand)[0]!.args[0].input;
    expect(s).toMatchObject({
      InstanceIds: ['i-1'],
      DocumentName: 'AWS-RunShellScript',
      TimeoutSeconds: 120,
      Parameters: { commands: ["export X='a'\\''b'\necho $X"], executionTimeout: ['120'] },
    });
    expect(ssm.commandCalls(GetCommandInvocationCommand)[0]!.args[0].input).toEqual({ CommandId: 'c-1', InstanceId: 'i-1' });
  });

  it('returns non-zero exit codes and timeouts without throwing', async () => {
    ssm.on(SendCommandCommand).resolves({ Command: { CommandId: 'c-1' } });
    ssm.on(GetCommandInvocationCommand).resolvesOnce({ Status: 'Failed', ResponseCode: 3, StandardErrorContent: 'boom' });
    expect(await compute.runCommand('i-1', { script: 'false' })).toMatchObject({ exitCode: 3, stderr: expect.stringContaining('boom') });
    ssm.on(GetCommandInvocationCommand).resolves({ Status: 'TimedOut', ResponseCode: -1 });
    expect((await compute.runCommand('i-1', { script: 'sleep 9' })).exitCode).toBe(124);
  });

  it('rejects bad env names and gives up past the deadline', async () => {
    await expect(compute.runCommand('i-1', { script: 'x', env: { 'A;B': '1' } })).rejects.toThrow(/invalid env var/);
    ssm.on(SendCommandCommand).resolves({ Command: { CommandId: 'c-1' } });
    ssm.on(GetCommandInvocationCommand).resolves({ Status: 'InProgress' });
    await expect(compute.runCommand('i-1', { script: 'x', timeoutSeconds: 5 })).rejects.toThrow(/still InProgress/);
  });
});

describe('preflight', () => {
  it('checks the account', async () => {
    expect(await compute.preflight('123456789012')).toMatchObject({ accountId: '123456789012' });
    await expect(compute.preflight('999999999999')).rejects.toThrow(/expected sandbox account/);
  });
});
