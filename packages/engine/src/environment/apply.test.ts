import { describe, expect, it } from 'vitest';
import { applyEnvironmentToConfigYaml, DEFAULT_ENVIRONMENT_INFRA_DIR, environmentSandboxId, type SandboxInput } from '@ratchet/core';
import { CONFIG_YAML, makeHarness, type Harness } from '../testing/harness.js';
import { sampleEnvironment } from './samples.js';

/**
 * The approved environment is applied as the sandbox of the run. The engine reads it from the PROTECTED ref (never a candidate's
 * tree), validates it against the sandbox contract, hands it to the sandbox provider with the sandbox input, and the same
 * sandbox is destroyed (with the receipt) after the run.
 */

const MIN = 60_000;
const D = DEFAULT_ENVIRONMENT_INFRA_DIR;

/** A harness whose protected ref already holds an approved environment of `scope` (what approving a draft commits). */
function withEnvironment(scope: 'none' | 'single' | 'full', over: { skipFiles?: boolean; tweak?: (path: string, content: string) => string } = {}): Harness {
  const h = makeHarness({ mode: 'live', settings: { sandbox: { auto: true } } });
  const patch = scope === 'single' ? { scope, machine: 'c7i.xlarge', image: `${D}/Dockerfile` } : { scope };
  h.git.files['ratchet.yaml'] = applyEnvironmentToConfigYaml(CONFIG_YAML, patch);
  if (!over.skipFiles) {
    for (const f of sampleEnvironment(scope).files) {
      if (f.path === 'ratchet.yaml') continue;
      h.git.files[f.path] = over.tweak ? over.tweak(f.path, f.content) : f.content;
    }
  }
  h.bench.set('main-sha', 'baseline', { samples: [100, 102, 98], metrics: { peak_rss: 500 } });
  h.bench.set('main-sha', 'confirm', { samples: [100, 100, 100] });
  return h;
}

const create = (h: Harness, id = 'run-a') =>
  h.engine.createOptimization({ id, repoUrl: h.git.remoteUrl(), baseRef: 'main', agents: ['codex'], budgetUsd: 100, timeLimitSeconds: 8 * 3600, plateauEvals: 50 });

async function start(h: Harness, id = 'run-a'): Promise<SandboxInput> {
  let seen: SandboxInput | undefined;
  h.sandbox.onApply = (i) => (seen = i);
  await create(h, id);
  await h.engine.start(id);
  return seen!;
}

describe('the engine applies the approved environment as the sandbox', () => {
  it('full: the module files from the protected ref go into the sandbox input, hashed, tagged, in a sandbox of their own', async () => {
    const h = withEnvironment('full');
    const input = await start(h);
    const env = input.environment!;
    expect(env).toMatchObject({ kind: 'drafted', scope: 'full', path: D, commit: 'main-sha' });
    expect(env.files.map((f) => f.path)).toEqual(expect.arrayContaining(['main.tf', 'seed.sh', 'seed/orders.sql', 'scripts/api.sh', 'scripts/load.sh']));
    expect(env.hash).toMatch(/^[0-9a-f]{10}$/);
    expect(input.sandboxId).toBe(environmentSandboxId('us-east-1', env.hash));
    expect(input.tags).toMatchObject({ 'ratchet:managed': 'true', 'ratchet:env': env.hash });
    expect(h.ledger.getOptimization('run-a')).toMatchObject({ sandboxId: input.sandboxId, status: 'running' });
  });

  it('none: the standard sandbox, nothing extra', async () => {
    const h = withEnvironment('none');
    const input = await start(h);
    expect(input.environment).toBeUndefined();
    expect(input.sandboxId).toBe('ratchet-sandbox');
  });

  it('single without a drafted module: the standard sandbox (the pinned machine comes from ratchet.yaml)', async () => {
    const h = withEnvironment('single', { skipFiles: true });
    expect((await start(h)).environment).toBeUndefined();
  });

  it('is deterministic and shared: the same environment in the same region reuses one sandbox; a changed one gets its own', async () => {
    const h = withEnvironment('full');
    const a = await start(h, 'run-a');
    await create(h, 'run-b');
    await h.engine.start('run-b');
    expect(h.ledger.getOptimization('run-b')!.sandboxId).toBe(a.sandboxId);
    expect(h.log.filter((l) => l.startsWith('sandbox.apply'))).toHaveLength(1);

    const h2 = withEnvironment('full', { tweak: (p, c) => (p.endsWith('scripts/redis.sh') ? c + '# tuned\n' : c) });
    const changed = await start(h2);
    expect(changed.environment!.hash).not.toBe(a.environment!.hash);
    expect(changed.sandboxId).not.toBe(a.sandboxId);
  });

  it('reads the files at the commit the optimization was created on, not wherever the branch is later', async () => {
    const h = withEnvironment('full');
    h.git.resolved.set('main', 'sha-at-create');
    h.bench.set('sha-at-create', 'baseline', { samples: [100, 102, 98], metrics: { peak_rss: 500 } });
    h.bench.set('sha-at-create', 'confirm', { samples: [100, 100, 100] });
    const input = await start(h);
    expect(input.environment!.commit).toBe('sha-at-create');
  });

  it('refuses to start when the config asks for an environment the protected ref does not hold', async () => {
    const h = withEnvironment('full', { skipFiles: true });
    await create(h);
    await expect(h.engine.start('run-a')).rejects.toMatchObject({ code: 'invalid_config', message: expect.stringMatching(/infra\/ratchet.*does not exist|Approve the environment/s) });
    expect(h.log.filter((l) => l.startsWith('sandbox.apply'))).toEqual([]);
  });

  it.each([
    ['a security group opening the internet', (p: string, c: string) => (p === 'main.tf' ? c + '\nresource "aws_security_group" "open" {\n  egress {\n    cidr_blocks = ["0.0.0.0/0"]\n  }\n}\n' : c), /sandbox contract/],
    ['an unpinned provider block of its own', (p: string, c: string) => (p === 'main.tf' ? c + '\nprovider "aws" {\n  region = "eu-west-1"\n}\n' : c), /sandbox contract/],
    ['a module that does not use the base module', (p: string, c: string) => (p === 'main.tf' ? c.replace('./ratchet-sandbox', 'github.com/evil/module') : c), /sandbox contract/],
  ])('refuses %s before anything is created', async (_name, tweak, message) => {
    const h = withEnvironment('full', { tweak: (p, c) => tweak(p.slice(D.length + 1), c) });
    await create(h);
    await expect(h.engine.start('run-a')).rejects.toMatchObject({ code: 'invalid_config', message });
    expect(h.log.filter((l) => l.startsWith('sandbox.apply'))).toEqual([]);
  });
});

describe('the environment is torn down after the run', () => {
  it('the sandbox that held it is destroyed after the idle window and the receipt says so', async () => {
    const h = withEnvironment('full');
    const input = await start(h);
    await h.engine.stop('run-a');
    h.clock.advance(31 * MIN);
    await h.engine.tick();
    await h.engine.settle();
    expect(h.log.filter((l) => l.startsWith('sandbox.destroy'))).toEqual([`sandbox.destroy:${input.sandboxId}`]);
    expect(h.sandbox.applied.size).toBe(0);
    expect(await h.engine.sandboxState(input.sandboxId)).toMatchObject({ status: 'destroyed' });
    const receipt = await h.engine.teardownReceipt('run-a');
    expect(receipt).not.toBeNull();
    expect(receipt!.sandboxId).toBe(input.sandboxId);
  });
});

describe('the approved environment is locked: candidates cannot change it', () => {
  it.each([
    ['infra/ratchet/main.tf'],
    ['infra/ratchet/scripts/load.sh'],
    ['infra/ratchet/seed/orders.sql'],
    ['ratchet.yaml'],
  ])('a candidate that touches %s is blocked without spending a bench run', async (path) => {
    const h = withEnvironment('full');
    await create(h);
    await h.engine.start('run-a');
    h.git.push('ratchet/run-a/codex', 'sha-env', ['src/a.ts', path]);
    h.git.push('ratchet/run-a/codex', 'sha-ok', ['src/a.ts']);
    h.bench.set('sha-ok', 'bench', { samples: [80, 80, 80], guardrails: [{ name: 'pytest -q', passed: true }] });
    h.bench.set('sha-ok', 'confirm', { samples: [80, 80, 80] });
    await h.until(() => h.ledger.candidatesWithResults('run-a').length === 2 && h.ledger.candidatesWithResults('run-a').every((c) => c.verdict !== 'pending'));
    const byId = (sha: string) => h.ledger.candidatesWithResults('run-a').find((c) => c.sha === sha)!;
    expect(byId('sha-env').verdict).toBe('blocked');
    expect(byId('sha-env').reasons[0]).toContain(`protected path changed: ${path}`);
    expect(byId('sha-env').results).toEqual([]);
    expect(byId('sha-ok').verdict).toBe('verified'); // a code-only change next to it is untouched
  });

  it('the protected set of the optimization lists the environment directory', async () => {
    const h = withEnvironment('full');
    await create(h);
    const p = await h.engine.protectedPaths('run-a');
    expect(p.globs).toContain(`${D}/**`);
    // the decision is locked whatever the scope: even `none` cannot be turned into a bigger environment by a candidate
    const none = withEnvironment('none');
    await create(none);
    expect((await none.engine.protectedPaths('run-a')).globs).toContain(`${D}/**`);
    // no environment decided yet (no scope in ratchet.yaml): nothing extra is protected
    const undecided = makeHarness({ mode: 'live', settings: { sandbox: { auto: true } } });
    await create(undecided);
    expect((await undecided.engine.protectedPaths('run-a')).globs).not.toContain(`${D}/**`);
  });
});
