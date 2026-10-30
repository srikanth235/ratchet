import { describe, it, expect } from 'vitest';
import { MockCloudCompute } from './mock.js';
import { ManualClock } from '../util.js';

const mk = (o = {}) => {
  const clock = new ManualClock(1000);
  return { clock, cloud: new MockCloudCompute({ clock, ...o }) };
};
const spec = (identity: Awaited<ReturnType<MockCloudCompute['createAgentIdentity']>>) => ({
  optimizationId: 'o1',
  agentId: 'a1',
  kind: 'mock' as const,
  instanceType: 'c7i.large',
  imageRef: 'ami-1',
  identity,
  secrets: [],
  repoUrl: 'x',
  branch: 'b',
});

describe('MockCloudCompute', () => {
  it('validates ids like the real one', async () => {
    const { cloud } = mk();
    await expect(cloud.createAgentIdentity('O 1', 'a')).rejects.toThrow(/invalid/);
    await expect(cloud.putSecret('o1', 'a/b', 'K', 'v')).rejects.toThrow();
  });

  it('identity lifecycle: create (idempotent), revoke, delete', async () => {
    const { cloud } = mk();
    const id = await cloud.createAgentIdentity('o1', 'a1');
    expect(id.roleRef).toContain('/ratchet/agent/ratchet-agent-o1-a1');
    expect(await cloud.createAgentIdentity('o1', 'a1')).toBe(id);
    expect(cloud.isRevoked(id)).toBe(false);
    await cloud.revokeAgentIdentity(id);
    expect(cloud.isRevoked(id)).toBe(true);
    await cloud.deleteAgentIdentity(id);
    await cloud.deleteAgentIdentity(id); // idempotent
    expect(cloud.identities()).toEqual([]);
    await expect(cloud.revokeAgentIdentity(id)).rejects.toThrow(/NoSuchEntity/);
  });

  it('secrets live under /ratchet/<opt>/<agent>/ and deleteSecrets is scoped', async () => {
    const { cloud } = mk();
    const r = await cloud.putSecret('o1', 'a1', 'KEY', 'v1');
    await cloud.putSecret('o1', 'a2', 'KEY', 'v2');
    expect(r.ref).toBe('/ratchet/o1/a1/KEY');
    await cloud.deleteSecrets('o1', 'a1');
    expect(cloud.secretNames()).toEqual(['/ratchet/o1/a2/KEY']);
  });

  it('launch requires an existing identity; instances boot after the delay', async () => {
    const { cloud, clock } = mk({ bootDelayMs: 5000 });
    const id = await cloud.createAgentIdentity('o1', 'a1');
    await cloud.deleteAgentIdentity(id);
    await expect(cloud.launchRunner(spec(id))).rejects.toThrow(/Invalid IAM Instance Profile/);
    const id2 = await cloud.createAgentIdentity('o1', 'a1');
    const inst = await cloud.launchRunner(spec(id2));
    expect(inst).toMatchObject({ role: 'agent_runner', agentId: 'a1', state: 'pending', launchedAt: 1000 });
    clock.advance(5000);
    expect((await cloud.listTagged('o1'))[0]!.state).toBe('running');
  });

  it('listTagged filters by optimization; terminate is idempotent and stamps terminatedAt', async () => {
    const { cloud, clock } = mk();
    const id = await cloud.createAgentIdentity('o1', 'a1');
    const r = await cloud.launchRunner(spec(id));
    const b = await cloud.launchBenchInstance({ optimizationId: 'o2', instanceType: 'c7i.2xlarge', imageRef: 'a', purpose: 'bench' });
    expect(b.agentId).toBeUndefined();
    expect((await cloud.listTagged('o2')).map((i) => i.id)).toEqual([b.id]);
    expect(await cloud.listTagged()).toHaveLength(2);
    clock.advance(10_000);
    await cloud.terminate([r.id, 'i-unknown']);
    clock.advance(10_000);
    await cloud.terminate([r.id]);
    const t = (await cloud.listTagged('o1'))[0]!;
    expect(t).toMatchObject({ state: 'terminated', terminatedAt: 11_000 });
  });

  it('runCommand delegates to the injected handler and needs a running instance', async () => {
    const seen: string[] = [];
    const { cloud } = mk({
      commandHandler: (inst: { id: string }, c: { script: string }) => {
        seen.push(`${inst.id}:${c.script}`);
        return { exitCode: 0, stdout: 'ok', stderr: '' };
      },
    });
    const id = await cloud.createAgentIdentity('o1', 'a1');
    const r = await cloud.launchRunner(spec(id));
    expect(await cloud.runCommand(r.id, { script: 'echo hi' })).toEqual({ exitCode: 0, stdout: 'ok', stderr: '' });
    expect(seen).toEqual([`${r.id}:echo hi`]);
    await cloud.terminate([r.id]);
    await expect(cloud.runCommand(r.id, { script: 'x' })).rejects.toThrow(/not running/);
    await expect(cloud.runCommand('i-nope', { script: 'x' })).rejects.toThrow(/InvalidInstanceId/);
  });

  it('default handler succeeds silently; helpers simulate experiments, shutdown and stopped', async () => {
    const { cloud, clock } = mk();
    const e = cloud.simulateExperiment('o1', 'a1', 'c7i.xlarge');
    expect(e.role).toBe('experiment');
    expect(await cloud.runCommand(e.id, { script: 'true' })).toMatchObject({ exitCode: 0 });
    clock.advance(1000);
    cloud.simulateStopped(e.id);
    expect((await cloud.listTagged())[0]).toMatchObject({ state: 'stopped', terminatedAt: 2000 });
    const f = cloud.simulateExperiment('o1', 'a1', 'c7i.xlarge');
    cloud.simulateShutdown(f.id);
    await Promise.resolve();
    expect((await cloud.listTagged()).find((i) => i.id === f.id)!.state).toBe('terminated');
  });
});
