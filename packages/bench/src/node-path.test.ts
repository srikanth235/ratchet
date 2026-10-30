import { execFileSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, dirname, join } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { LocalBenchRunner } from './local.js';
import { currentNodeRuntime, ensureNodeShim, nodeDirFor, nodeShimScript, resolveNodeOnPath, withNodeOnPath, type NodeRuntimeInfo } from './node-path.js';
import { makeConfig, makeFixtureRepo, MemoryStore, realClock, type FixtureRepo } from './testkit/repo.js';

let root: string;
let fakeElectron: string; // behaves like Electron: only acts as Node when ELECTRON_RUN_AS_NODE=1
let plainNode: string; // a binary literally called `node`, in its own dir
const electron = (): NodeRuntimeInfo => ({ execPath: fakeElectron, electron: '33.2.0', platform: 'linux' });
const sh = (script: string, env: Record<string, string>): string => execFileSync('/bin/sh', ['-c', script], { env, encoding: 'utf8' }).trim();

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'ratchet-nodepath-'));
  fakeElectron = join(root, 'Ratchet');
  writeFileSync(fakeElectron, `#!/bin/sh\n[ "$ELECTRON_RUN_AS_NODE" = 1 ] || { echo "not run as node" >&2; exit 97; }\nexec ${JSON.stringify(process.execPath)} "$@"\n`);
  chmodSync(fakeElectron, 0o755);
  const binDir = join(root, 'bin');
  execFileSync('mkdir', [binDir]);
  plainNode = join(binDir, 'node');
  symlinkSync(realpathSync(process.execPath), plainNode);
});
afterAll(() => rmSync(root, { recursive: true, force: true }));

describe('nodeShimScript / ensureNodeShim', () => {
  it('sets ELECTRON_RUN_AS_NODE only for Electron, and quotes the path', () => {
    expect(nodeShimScript("/App's/Ratchet", true)).toContain(`ELECTRON_RUN_AS_NODE=1 exec '/App'\\''s/Ratchet' "$@"`);
    expect(nodeShimScript('/usr/bin/node22', false)).not.toContain('ELECTRON_RUN_AS_NODE');
  });

  it('creates an executable node in a private dir, reuses it, and recreates it if it vanished', () => {
    const dir = ensureNodeShim(fakeElectron, true, root);
    expect(dirname(dir)).toBe(root);
    expect(statSync(join(dir, 'node')).mode & 0o111).not.toBe(0);
    expect(statSync(dir).mode & 0o077).toBe(0);
    expect(readFileSync(join(dir, 'node'), 'utf8')).toBe(nodeShimScript(fakeElectron, true));
    expect(ensureNodeShim(fakeElectron, true, root)).toBe(dir);
    expect(ensureNodeShim(fakeElectron, false, root)).not.toBe(dir);
    rmSync(dir, { recursive: true });
    const again = ensureNodeShim(fakeElectron, true, root);
    expect(statSync(join(again, 'node')).isFile()).toBe(true);
  });
});

describe('withNodeOnPath: Electron', () => {
  it('prepends a shim so `node` runs the Electron binary as Node, even with no node on PATH', () => {
    const env = withNodeOnPath({ PATH: '/usr/bin:/bin', CI: '1' }, { runtime: electron(), shimRoot: root });
    const [first, ...rest] = env.PATH!.split(delimiter);
    expect(rest.join(delimiter)).toBe('/usr/bin:/bin');
    expect(readFileSync(join(first!, 'node'), 'utf8')).toContain('ELECTRON_RUN_AS_NODE=1');
    expect(env.CI).toBe('1');
    expect(env.ELECTRON_RUN_AS_NODE).toBeUndefined(); // only the shim sets it; nothing else is turned into "Node"
    expect(sh('node -p "40 + 2"', env)).toBe('42');
    expect(sh('node -p "process.env.ELECTRON_RUN_AS_NODE"', env)).toBe('1'); // inherited by node's own children
  });

  it('wins over a real node that is already on PATH, and handles a missing PATH', () => {
    const pathWithNode = `${dirname(plainNode)}:/usr/bin:/bin`;
    const env = withNodeOnPath({ PATH: pathWithNode }, { runtime: electron(), shimRoot: root });
    expect(sh('node -p "1 + 1"', env)).toBe('2');
    expect(sh('command -v node', env)).not.toBe(plainNode);
    const bare = withNodeOnPath({}, { runtime: electron(), shimRoot: root, basePath: '' });
    expect(bare.PATH).toBe(dirname(sh('command -v node', { PATH: bare.PATH! })));
    expect(bare.PATH!.includes(delimiter)).toBe(false); // no trailing empty entry (which means "cwd")
  });

  it('does not mutate its input and drops undefined values', () => {
    const input: Record<string, string | undefined> = { PATH: '/bin', GONE: undefined };
    const out = withNodeOnPath(input, { runtime: electron(), shimRoot: root });
    expect(input).toEqual({ PATH: '/bin', GONE: undefined });
    expect('GONE' in out).toBe(false);
  });
});

describe('withNodeOnPath: plain Node', () => {
  const plain = (): NodeRuntimeInfo => ({ execPath: plainNode, platform: 'linux' });

  it('prepends dirname(execPath) when PATH has no node', () => {
    const env = withNodeOnPath({ PATH: '/usr/bin:/bin' }, { runtime: plain() });
    expect(env.PATH).toBe(`${dirname(plainNode)}:/usr/bin:/bin`);
    expect(sh('node -p "6 * 7"', { PATH: `${env.PATH}` })).toBe('42');
  });

  it('prepends dirname(execPath) when a different node would win', () => {
    const other = join(root, 'other');
    execFileSync('mkdir', [other]);
    symlinkSync('/bin/true', join(other, 'node'));
    expect(resolveNodeOnPath(`${other}:${dirname(plainNode)}`)).toBe(realpathSync('/bin/true'));
    const env = withNodeOnPath({ PATH: `${other}:/usr/bin` }, { runtime: plain() });
    expect(env.PATH!.startsWith(`${dirname(plainNode)}${delimiter}`)).toBe(true);
  });

  it('leaves PATH alone when the first node on PATH already is this runtime', () => {
    const p = dirname(plainNode);
    expect(nodeDirFor(plain(), p)).toBeUndefined();
    expect(withNodeOnPath({ PATH: p }, { runtime: plain() }).PATH).toBe(p);
  });

  it('uses a shim (without ELECTRON_RUN_AS_NODE) when the binary is not called node', () => {
    const odd = join(root, 'bin', 'node22');
    symlinkSync(realpathSync(process.execPath), odd);
    const env = withNodeOnPath({ PATH: '/usr/bin:/bin' }, { runtime: { execPath: odd, platform: 'linux' }, shimRoot: root });
    expect(readFileSync(join(env.PATH!.split(delimiter)[0]!, 'node'), 'utf8')).not.toContain('ELECTRON_RUN_AS_NODE');
    expect(sh('node -p "1 + 2"', env)).toBe('3');
  });

  it('is a no-op on Windows, and the real runtime works in a PATH-less environment', () => {
    expect(withNodeOnPath({ PATH: 'C:\\x' }, { runtime: { execPath: 'C:\\node.exe', platform: 'win32' } }).PATH).toBe('C:\\x');
    const env = withNodeOnPath({ PATH: '/nonexistent' }, { runtime: currentNodeRuntime(), shimRoot: root });
    expect(sh('node -p "process.versions.node.split(\'.\')[0] >= 22"', env)).toBe('true');
  });
});

describe('LocalBenchRunner under (simulated) Electron', () => {
  let fx: FixtureRepo;
  beforeAll(() => {
    fx = makeFixtureRepo();
  });
  afterAll(() => fx.cleanup());

  const job = (sha: string) => ({ optimizationId: 'o', candidateId: 'c', sha, phase: 'bench' as const, repeats: 1 });
  const runner = (nodeRuntime: NodeRuntimeInfo | false, env: Record<string, string>, setup?: string) =>
    new LocalBenchRunner({
      git: fx.host,
      scratchRoot: join(fx.dir, 'scratch'),
      clock: realClock,
      artifacts: new MemoryStore(),
      env,
      nodeRuntime,
      ...(setup ? { setup } : {}),
    });

  it('runs setup, command guardrails and the evaluator (`node ...` through the shell) with no node on PATH', async () => {
    const sha = fx.candidate({ 'src/p95.txt': '60' });
    // a PATH with no node and no way to find the Electron binary except through the shim
    const env = { PATH: '/usr/bin:/bin' };
    const out = await runner(electron(), env, 'node -e "process.exit(0)"').run(job(sha), makeConfig(), { metrics: { peak_rss_mb: 100 } });
    expect(out.samples).toEqual([60]);
    expect(out.guardrails.every((g) => g.passed)).toBe(true);
  });

  it('nodeRuntime: false leaves PATH alone (and then the evaluator cannot find node)', async () => {
    const sha = fx.candidate({ 'src/p95.txt': '61' });
    const out = await runner(false, { PATH: '/usr/bin:/bin' }).run(job(sha), makeConfig({ guardrails: [] }), null);
    expect(out.samples).toEqual([]);
    expect(out.guardrails.some((g) => !g.passed)).toBe(true);
  });
});
