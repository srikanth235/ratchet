import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BenchJob } from '@ratchet/core';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { archFromInstanceType, depsKey, lockfileChanges, manifestKey, parseDepsManifest, PNPM, type DepsManifest } from './deps.js';
import { BenchAbortedError, BenchError } from './errors.js';
import { LocalBenchRunner } from './local.js';
import { RemoteBenchRunner, type RemoteBenchRunnerOptions } from './remote.js';
import { LocalFakeCompute } from './testkit/compute.js';
import { DirStore, instantClock, makeConfig, makeFixtureRepo, MemoryStore, realClock, type FixtureRepo } from './testkit/repo.js';

/**
 * Dependency snapshots end to end with the REAL generated scripts run by local bash (the same approach as runners.test.ts):
 * `aws s3 cp` is a directory copy, `pnpm`/`npm` are shims that behave like the real ones where it matters (fetch fills a store,
 * install refuses to run unless the network is switched off and every package is in the store).
 */

const BUCKET = 'ratchet-test-artifacts';
const LOCK = 'lockfileVersion: 9\ndep:left-pad\ndep:is-odd\n';
const FILES = { 'pnpm-lock.yaml': LOCK, 'package.json': '{"name":"fx"}\n' };
const SETUP = 'pnpm install --frozen-lockfile && echo 77 > src/p95.txt';

const pnpmShim = (log: string, flags: string) => `
case "\${1:-}" in
  --version) echo 10.99.0; exit 0;;
  fetch)
    echo "fetch cwd=$PWD store=\${npm_config_store_dir:-} scripts=\${npm_config_ignore_scripts:-} aws=\${AWS_ACCESS_KEY_ID:-none} proxy=\${HTTPS_PROXY:-none} user=$(id -un) home=$HOME" >> ${log}
    [ -e ${flags}/slow-fetch ] && sleep 30
    [ -e ${flags}/fail-fetch ] && { echo "ERR_PNPM_FETCH_FAIL registry unreachable" >&2; exit 1; }
    [ -f pnpm-lock.yaml ] || { echo "no lockfile" >&2; exit 1; }
    s="$npm_config_store_dir/v10"; mkdir -p "$s/files" "$s/projects"
    while read -r l; do n="\${l#dep:}"; echo "pkg $n" > "$s/files/$n"; done < <(grep '^dep:' pnpm-lock.yaml)
    ln -s "$PWD" "$s/projects/job";;
  install)
    echo "install cwd=$PWD store=\${npm_config_store_dir:-} offline=\${npm_config_offline:-unset} import=\${npm_config_package_import_method:-} pm_versions=\${npm_config_manage_package_manager_versions:-}" >> ${log}
    [ "\${npm_config_offline:-}" = true ] || { echo "network needed (offline is not set)" >&2; exit 1; }
    if grep -q newdep package.json 2>/dev/null; then echo "ERR_PNPM_OUTDATED_LOCKFILE specifiers in the lockfile do not match package.json" >&2; exit 1; fi
    s="$npm_config_store_dir/v10"
    ln -s "$PWD" "$s/projects/cand-$$" || { echo "EACCES cannot register the project in the store" >&2; exit 1; }
    mkdir -p node_modules
    while read -r l; do n="\${l#dep:}"; [ -f "$s/files/$n" ] || { echo "ERR_PNPM_NO_OFFLINE_TARBALL $n" >&2; exit 1; }; cp "$s/files/$n" "node_modules/$n"; done < <(grep '^dep:' pnpm-lock.yaml);;
  *) echo "unexpected pnpm $*" >&2; exit 2;;
esac`;

const npmShim = (log: string) => `
case "\${1:-}" in
  --version) echo 10.9.9; exit 0;;
  ci)
    echo "ci cwd=$PWD cache=\${npm_config_cache:-} offline=\${npm_config_offline:-unset} logs=\${npm_config_logs_dir:-} ignore_scripts=\${npm_config_ignore_scripts:-}" >> ${log}
    c="$npm_config_cache/_cacache/content"
    if [ "\${npm_config_offline:-}" = true ]; then
      [ -f "$c/left-pad" ] || { echo "ENOTCACHED left-pad" >&2; exit 1; }
      mkdir -p "$npm_config_logs_dir" node_modules && cp "$c/left-pad" node_modules/left-pad
    else
      mkdir -p "$c" "$npm_config_cache/_logs" && echo "pkg left-pad" > "$c/left-pad" && echo log > "$npm_config_cache/_logs/1.log"
      echo x > "$npm_config_cache/_update-notifier-last-checked"
    fi;;
  *) echo "unexpected npm $*" >&2; exit 2;;
esac`;

const job = (sha: string, over: Partial<BenchJob> = {}): BenchJob => ({ optimizationId: 'opt1', candidateId: 'cand1', sha, phase: 'bench', repeats: 1, ...over });
const cfg = (o: Parameters<typeof makeConfig>[0] = {}) => makeConfig({ guardrails: [], ...o });
let seq = 0;

interface Rig {
  runner: RemoteBenchRunner;
  compute: LocalFakeCompute;
  store: MemoryStore;
  s3: string;
  /** launch:<purpose> / terminate:<id> in the order they happened */
  timeline: string[];
  warnings: string[];
  shimLog: () => string;
  flag: (name: string) => void;
  unflag: (name: string) => void;
  /** Objects in the artifacts bucket. */
  objects: (prefix: string) => string[];
}

/** What the snapshot builder will pick on this machine (it uses zstd when present). */
const HOST_COMPRESSION: 'zstd' | 'gzip' = spawnSync('sh', ['-c', 'command -v zstd']).status === 0 ? 'zstd' : 'gzip';
const listArchive = (path: string): string =>
  execFileSync('sh', ['-c', HOST_COMPRESSION === 'zstd' ? 'zstd -dc -- "$1" | tar -tf -' : 'tar -tzf "$1"', 'sh', path], { encoding: 'utf8' });

function rig(fx: FixtureRepo, over: Partial<RemoteBenchRunnerOptions> = {}, shims: Record<string, string> = {}): Rig {
  const n = ++seq;
  const s3 = join(fx.dir, `s3-${n}`);
  const flags = join(fx.dir, `flags-${n}`);
  mkdirSync(flags, { recursive: true });
  const log = join(fx.dir, `shim-${n}.log`);
  const compute = new LocalFakeCompute(join(fx.dir, `ssm-${n}`), {}, s3);
  compute.addShim('pnpm', pnpmShim(log, flags));
  compute.addShim('npm', npmShim(log));
  for (const [name, body] of Object.entries(shims)) compute.addShim(name, body);
  const timeline: string[] = [];
  const launch = compute.launchBenchInstance.bind(compute);
  compute.launchBenchInstance = async (spec) => {
    timeline.push(`launch:${spec.purpose}`);
    return launch(spec);
  };
  const terminate = compute.terminate.bind(compute);
  compute.terminate = async (ids) => {
    timeline.push(...ids.map((i) => `terminate:${i}`));
    return terminate(ids);
  };
  const store = new MemoryStore();
  const warnings: string[] = [];
  const runner = new RemoteBenchRunner({
    compute,
    git: fx.host,
    clock: instantClock,
    imageRef: 'ami-test',
    artifacts: store,
    stagingBucket: BUCKET,
    stagingStore: new DirStore(join(s3, BUCKET)),
    isolation: 'none',
    workRoot: join(fx.dir, `work-${n}`),
    setup: SETUP,
    onWarning: (w) => warnings.push(w),
    ...(over.isolation === 'user' ? { candidateLauncher: compute.shimPath('ratchet-candidate') } : {}),
    ...over,
  });
  const walk = (dir: string, rel = ''): string[] =>
    !existsSync(dir) ? [] : readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(dir, e.name), `${rel}${e.name}/`) : [`${rel}${e.name}`]));
  return {
    runner,
    compute,
    store,
    s3,
    timeline,
    warnings,
    shimLog: () => (existsSync(log) ? readFileSync(log, 'utf8') : ''),
    flag: (name) => writeFileSync(join(flags, name), ''),
    unflag: (name) => rmSync(join(flags, name), { force: true }),
    objects: (prefix) => walk(join(s3, BUCKET)).filter((k) => k.startsWith(prefix)),
  };
}

const keyFor = (lock: string, arch = 'x86_64', image = 'ami-test') => depsKey({ strategy: PNPM, lockfiles: { 'pnpm-lock.yaml': sha256(lock) }, image, arch });
const sha256 = (s: string) => execFileSync('sha256sum', { input: s, encoding: 'utf8' }).split(' ')[0]!;

describe('dependency snapshots (RemoteBenchRunner)', () => {
  let fx: FixtureRepo;
  beforeAll(() => {
    fx = makeFixtureRepo(FILES);
  });
  afterAll(() => fx.cleanup());

  describe('the first job builds the snapshot on a separate instance with egress, then runs offline from it', () => {
    it('launches the deps instance first, terminates it before the bench instance exists, and installs from the snapshot offline', async () => {
      const r = rig(fx);
      const progress: string[] = [];
      const out = await r.runner.run(job(fx.base), cfg(), null, undefined, (p) => progress.push(p.step));
      expect(out.samples).toEqual([77]); // setup ran `pnpm install` from the store, then wrote the number
      expect(out.guardrails).toEqual([]);
      expect(r.timeline).toEqual(['launch:deps', 'terminate:i-fake1', 'launch:bench', 'terminate:i-fake2']);
      expect(r.compute.launched.map((l) => [l.purpose, l.optimizationId, l.imageRef, l.instanceType])).toEqual([
        ['deps', 'opt1', 'ami-test', 'c7i.2xlarge'],
        ['bench', 'opt1', 'ami-test', 'c7i.2xlarge'],
      ]);
      expect(progress[0]).toBe('deps'); // before checkout, so the UI can show "dependencies"
      expect(progress.slice(1, 3)).toEqual(['checkout', 'setup']);
    });

    it('stores the archive under its key and the manifest last; the manifest matches what the controller computes', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const key = keyFor(LOCK);
      const manifestText = readFileSync(join(r.s3, BUCKET, manifestKey(key)), 'utf8');
      const parsed = parseDepsManifest(manifestText, { strategy: PNPM, lockfiles: { 'pnpm-lock.yaml': sha256(LOCK) }, image: 'ami-test', arch: 'x86_64', key });
      expect(parsed.ok).toBe(true);
      const m = (parsed as { manifest: DepsManifest }).manifest;
      // The builder packs with zstd when the machine has it (CI runners do), else gzip; the shimmed zstd test below covers both.
      expect(m.compression).toBe(HOST_COMPRESSION);
      expect(r.objects(`bench-deps/${key}/`)).toEqual([`bench-deps/${key}/${m.sha256}.${HOST_COMPRESSION === 'zstd' ? 'tar.zst' : 'tgz'}`]);
      expect(m.node).toBe(process.version);
      expect(m.pm).toBe('10.99.0');
    });

    it('the archive holds the fetched packages but not the job tree registration', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const key = keyFor(LOCK);
      const archive = r.objects(`bench-deps/${key}/`)[0]!;
      const listing = listArchive(join(r.s3, BUCKET, archive));
      expect(listing).toContain('v10/files/left-pad');
      expect(listing).toContain('v10/files/is-odd');
      expect(listing).not.toContain('projects/');
    });

    it('runs the fetch as the snapshot job intends: only `fetch`, scripts off, no credentials, nothing from the candidate', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const fetch = r.shimLog().split('\n').find((l) => l.startsWith('fetch '))!;
      expect(fetch).toContain('scripts=true');
      expect(fetch).toContain('aws=none');
      expect(fetch).toMatch(/store=.*\/store /);
      // the only pnpm invocations before setup are the version probes and the fetch
      expect(r.shimLog().split('\n').filter((l) => l.startsWith('fetch ')).length).toBe(1);
    });

    it('never stages or downloads the candidate for the snapshot job; only deps-base.tgz', async () => {
      const r = rig(fx);
      const sha = fx.candidate({ 'src/p95.txt': '55' });
      await r.runner.run(job(sha), cfg(), null);
      const depsScript = r.compute.commands.map((c) => c.script).find((s) => s.includes('pnpm fetch'))!;
      expect(depsScript).toContain('deps-base.tgz');
      expect(depsScript).not.toContain('code.tgz');
      expect(depsScript).not.toContain(sha);
      const inputs = r.objects('bench-inputs/');
      expect(inputs.filter((k) => k.endsWith('deps-base.tgz'))).toHaveLength(1);
      expect(inputs.filter((k) => k.endsWith('code.tgz'))).toHaveLength(1);
    });

    it('the snapshot job runs BEFORE the bench instance is even launched (so before the baseline), in its own instance', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base, { phase: 'baseline', candidateId: undefined, repeats: 3 }), cfg(), null);
      expect(r.timeline.slice(0, 3)).toEqual(['launch:deps', 'terminate:i-fake1', 'launch:baseline']);
    });

    it('setup and the guardrail commands get the restored store and the package manager offline; the bench harness does not', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg({ guardrails: ['- run: "true"'] }), null);
      const scripts = r.compute.commands.map((c) => c.script);
      const setup = scripts.find((s) => s.includes('pnpm install --frozen-lockfile'))!;
      expect(setup).toMatch(/export npm_config_offline='true'/);
      expect(setup).toMatch(/export npm_config_store_dir='[^']*\/deps\/store'/);
      expect(setup).toMatch(/export PNPM_STORE_DIR='[^']*\/deps\/store'/);
      const guard = scripts.find((s) => s.includes("bash -c 'true'"))!;
      expect(guard).toMatch(/export npm_config_offline='true'/);
      const bench = scripts.find((s) => s.includes('bench/bench.mjs'))!;
      expect(bench).not.toContain('npm_config_offline');
      expect(bench).not.toContain('npm_config_store_dir');
      const install = r.shimLog().split('\n').find((l) => l.startsWith('install '))!;
      expect(install).toContain('offline=true');
      expect(install).toContain('import=clone-or-copy');
      expect(install).toContain('pm_versions=false');
    });
  });

  describe('later jobs reuse the snapshot', () => {
    it('does not launch a deps instance again for the same lockfile, in this process or a fresh one sharing the bucket', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      r.timeline.length = 0;
      const progress: string[] = [];
      const out = await r.runner.run(job(fx.base), cfg(), null, undefined, (p) => progress.push(p.step));
      expect(out.samples).toEqual([77]);
      expect(r.timeline).toEqual(['launch:bench', 'terminate:i-fake3']);
      expect(progress).not.toContain('deps');
      expect(r.store.files.get('bench/opt1/cand1/bench.log')).toContain('reusing snapshot');

      // a new runner (a restarted app) finds the manifest in the bucket
      const again = new RemoteBenchRunner({ compute: r.compute, git: fx.host, clock: instantClock, imageRef: 'ami-test', stagingBucket: BUCKET, stagingStore: new DirStore(join(r.s3, BUCKET)), isolation: 'none', workRoot: join(fx.dir, 'work-again'), setup: SETUP });
      r.timeline.length = 0;
      await again.run(job(fx.base), cfg(), null);
      expect(r.timeline.filter((t) => t === 'launch:deps')).toHaveLength(0);
    });

    it('jobs that need the same snapshot at the same time share one build', async () => {
      const r = rig(fx);
      const sha = fx.candidate({ 'src/p95.txt': '60' });
      const [a, b] = await Promise.all([r.runner.run(job(fx.base, { candidateId: 'c1' }), cfg(), null), r.runner.run(job(sha, { candidateId: 'c2' }), cfg(), null)]);
      expect(a.samples).toEqual([77]);
      expect(b.samples).toEqual([77]);
      expect(r.timeline.filter((t) => t === 'launch:deps')).toHaveLength(1);
      expect(r.timeline.filter((t) => t === 'launch:bench')).toHaveLength(2);
    });

    it('a different lockfile at the base ref (or another image / architecture) is another snapshot', async () => {
      const other = makeFixtureRepo({ ...FILES, 'pnpm-lock.yaml': LOCK + 'dep:lodash\n' });
      try {
        const r = rig(other);
        await r.runner.run(job(other.base), cfg(), null);
        expect(r.objects('bench-deps/').filter((k) => k.endsWith('.json'))).toEqual([manifestKey(keyFor(LOCK + 'dep:lodash\n'))]);
        expect(keyFor(LOCK)).not.toBe(keyFor(LOCK + 'dep:lodash\n'));
        expect(keyFor(LOCK, 'arm64')).not.toBe(keyFor(LOCK));
        expect(keyFor(LOCK, 'x86_64', 'ami-other')).not.toBe(keyFor(LOCK));
        expect(archFromInstanceType('c7i.2xlarge')).toBe('x86_64');
      } finally {
        other.cleanup();
      }
    });

    it('rebuilds when the manifest is for something else, or when its archive has expired', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const key = keyFor(LOCK);
      const manifestPath = join(r.s3, BUCKET, manifestKey(key));

      writeFileSync(manifestPath, JSON.stringify({ v: 1, key: 'pnpm-' + '0'.repeat(40) }));
      r.timeline.length = 0;
      await r.runner.run(job(fx.base), cfg(), null);
      expect(r.timeline.filter((t) => t === 'launch:deps')).toHaveLength(1);
      expect(r.store.files.get('bench/opt1/cand1/bench.log')).toContain('ignoring bench-deps/');

      const m = JSON.parse(readFileSync(manifestPath, 'utf8')) as DepsManifest;
      execFileSync('rm', ['-f', join(r.s3, BUCKET, m.archive)]);
      r.timeline.length = 0;
      await r.runner.run(job(fx.base), cfg(), null);
      expect(r.timeline.filter((t) => t === 'launch:deps')).toHaveLength(1);
      expect(r.store.files.get('bench/opt1/cand1/bench.log')).toContain('is gone from the bucket');
    });
  });

  describe('instance discipline (the same terminate-in-finally as a bench instance)', () => {
    it('terminates the deps instance and launches no bench instance when the fetch fails; the error says why and where to look', async () => {
      const r = rig(fx);
      r.flag('fail-fetch');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/building the pnpm dependency snapshot failed \(exit 50\).*package fetch \(pnpm\) exited with status 1/s);
      expect(r.timeline).toEqual(['launch:deps', 'terminate:i-fake1']);
      expect(r.objects('bench-deps/')).toEqual([]);
      expect(r.store.files.get('bench/opt1/cand1/bench.log')).toContain('agent subnet');
    });

    it('failure is an infrastructure error (BenchError), not a verdict about the candidate', async () => {
      const r = rig(fx);
      r.flag('fail-fetch');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toBeInstanceOf(BenchError);
    });

    it('aborting during the snapshot build stops waiting, terminates the deps instance and launches nothing else', async () => {
      const r = rig(fx);
      r.flag('slow-fetch');
      const ac = new AbortController();
      const p = r.runner.run(job(fx.base), cfg(), null, ac.signal);
      const t0 = Date.now();
      while (!r.compute.commands.some((c) => c.script.includes('pnpm fetch'))) {
        if (Date.now() - t0 > 20_000) throw new Error('the snapshot job never started');
        await new Promise((res) => setTimeout(res, 25));
      }
      ac.abort();
      await expect(p).rejects.toBeInstanceOf(BenchAbortedError);
      expect(r.timeline).toEqual(['launch:deps', 'terminate:i-fake1']);
    });

    it('a job whose build was aborted by another job does not fail with it: it builds itself', async () => {
      const r = rig(fx);
      r.flag('slow-fetch');
      const ac = new AbortController();
      const first = r.runner.run(job(fx.base, { candidateId: 'c1' }), cfg(), null, ac.signal);
      const t0 = Date.now();
      while (!r.compute.commands.some((c) => c.script.includes('pnpm fetch'))) {
        if (Date.now() - t0 > 20_000) throw new Error('the snapshot job never started');
        await new Promise((res) => setTimeout(res, 25));
      }
      const second = r.runner.run(job(fx.base, { candidateId: 'c2' }), cfg(), null); // waits for the build in flight
      await new Promise((res) => setTimeout(res, 300));
      r.unflag('slow-fetch'); // the second build is fast
      ac.abort();
      await expect(first).rejects.toBeInstanceOf(BenchAbortedError);
      await expect(second).resolves.toMatchObject({ samples: [77] });
      expect(r.timeline.filter((t) => t === 'launch:deps')).toHaveLength(2);
    });

    it('reports (but survives) a deps instance that cannot be terminated; the reaper is the backstop', async () => {
      const r = rig(fx);
      r.compute.failTerminate = true;
      const out = await r.runner.run(job(fx.base), cfg(), null);
      expect(out.samples).toEqual([77]);
      expect(r.warnings.some((w) => /FAILED to terminate dependency snapshot instance i-fake1/.test(w))).toBe(true);
    });

    it('turns a failed launch into a clear error (for example a sandbox without the deps identity)', async () => {
      const r = rig(fx);
      r.compute.launchBenchInstance = async (spec) => {
        if (spec.purpose === 'deps') throw new Error('depsInstanceProfile is not configured (deps_identity_ref)');
        throw new Error('unexpected');
      };
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/cannot launch the dependency snapshot instance: depsInstanceProfile is not configured/);
    });
  });

  describe('a lockfile change is blocked, not retried with network access', () => {
    it('blocks a candidate whose lockfile differs before the instance runs anything', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null); // baseline builds the snapshot
      const sha = fx.candidate({ 'pnpm-lock.yaml': LOCK + 'dep:evil-package\n', 'package.json': '{"name":"fx","dependencies":{"evil-package":"1.0.0"}}\n' });
      r.timeline.length = 0;
      r.compute.commands.length = 0;
      const out = await r.runner.run(job(sha), cfg(), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails).toHaveLength(1);
      expect(out.guardrails[0]).toMatchObject({ name: 'dependencies unchanged', passed: false });
      expect(out.guardrails[0]!.detail).toContain('dependency changes are not allowed in v0: the bench has no network');
      expect(out.guardrails[0]!.detail).toContain('pnpm-lock.yaml');
      // nothing ran on the instance: no ping, no checkout, no setup, no deps rebuild
      expect(r.compute.commands).toEqual([]);
      expect(r.timeline).toEqual(['launch:bench', expect.stringMatching(/^terminate:/)]);
      expect(r.store.files.get('bench/opt1/cand1/bench.log')).toContain('blocked before running anything');
    });

    it('blocks a candidate that only deletes the lockfile, or swaps it for a symlink', async () => {
      const r = rig(fx);
      const gone = fx.candidate({ 'pnpm-lock.yaml': null });
      const outGone = await r.runner.run(job(gone), cfg(), null);
      expect(outGone.guardrails[0]).toMatchObject({ name: 'dependencies unchanged', passed: false });
      const link = fx.candidate({ 'pnpm-lock.yaml': { symlink: '/etc/hostname' } });
      const outLink = await r.runner.run(job(link), cfg(), null);
      expect(outLink.guardrails[0]).toMatchObject({ name: 'dependencies unchanged', passed: false });
    });

    it('blocks a candidate that adds or edits install-steering config (.npmrc, pnpm-workspace.yaml)', async () => {
      const r = rig(fx);
      for (const files of [{ '.npmrc': 'registry=https://evil.example/\n' }, { 'pnpm-workspace.yaml': 'onlyBuiltDependencies: [evil]\n' }]) {
        const out = await r.runner.run(job(fx.candidate(files)), cfg(), null);
        expect(out.guardrails[0]).toMatchObject({ name: 'dependencies unchanged', passed: false });
        expect(out.guardrails[0]!.detail).toContain(Object.keys(files)[0]!);
      }
    });

    it('lets a candidate through that changes everything except the dependencies', async () => {
      const r = rig(fx);
      const sha = fx.candidate({ 'src/p95.txt': '10', 'package.json': '{"name":"fx","scripts":{"build":"true"}}\n' });
      const out = await r.runner.run(job(sha), cfg(), null);
      expect(out.guardrails).toEqual([]);
      expect(out.samples).toEqual([77]);
    });

    it('explains a failed offline install: package.json asked for something the lockfile does not have', async () => {
      const r = rig(fx);
      const sha = fx.candidate({ 'package.json': '{"name":"fx","dependencies":{"newdep":"1.0.0"}}\n' });
      const out = await r.runner.run(job(sha), cfg(), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails).toHaveLength(1);
      expect(out.guardrails[0]).toMatchObject({ name: 'setup', passed: false });
      expect(out.guardrails[0]!.detail).toContain('exited with status 1');
      expect(out.guardrails[0]!.detail).toContain('with no network');
      expect(out.guardrails[0]!.detail).toContain('dependency changes are not allowed in v0');
      expect(r.store.files.get('bench/opt1/cand1/bench.log')).toContain('ERR_PNPM_OUTDATED_LOCKFILE');
    });

    it('a setup that rewrites the lockfile is caught: lockfiles are protected without being listed in `protected`', async () => {
      const r = rig(fx, { setup: `${SETUP} && echo dep:sneaky >> pnpm-lock.yaml` });
      const out = await r.runner.run(job(fx.base), makeConfig({ guardrails: ['- run: "true"'] }), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails[0]).toMatchObject({ name: 'protected files intact', passed: false });
      expect(out.guardrails[0]!.detail).toContain('pnpm-lock.yaml');
    });

    it('the user protecting nothing is still an error: the pinned lockfile does not stand in for a protected bench', async () => {
      const r = rig(fx);
      await expect(r.runner.run(job(fx.base), cfg({ protectedGlobs: ['nothing/**'] }), null)).rejects.toThrow(/contains no files matching the protected globs/);
    });
  });

  describe('when there is nothing to snapshot', () => {
    it('without setup nothing is installed, so nothing is snapshotted and nothing is blocked', async () => {
      const r = rig(fx, { setup: undefined });
      const sha = fx.candidate({ 'pnpm-lock.yaml': LOCK + 'dep:other\n' });
      const out = await r.runner.run(job(sha), cfg(), null);
      expect(out.guardrails).toEqual([]);
      expect(r.timeline).toEqual(['launch:bench', 'terminate:i-fake1']);
      expect(r.objects('bench-deps/')).toEqual([]);
    });

    it('evaluator.deps: none disables it even with a lockfile (the image provides whatever setup needs)', async () => {
      const r = rig(fx, { setup: 'echo 66 > src/p95.txt' });
      const sha = fx.candidate({ 'pnpm-lock.yaml': LOCK + 'dep:other\n' });
      const out = await r.runner.run(job(sha), cfg({ deps: 'none' }), null);
      expect(out.samples).toEqual([66]);
      expect(r.timeline).toEqual(['launch:bench', 'terminate:i-fake1']);
    });

    it('a repository without a lockfile (deps: auto) runs setup as before', async () => {
      const plain = makeFixtureRepo();
      try {
        const r = rig(plain, { setup: 'echo 66 > src/p95.txt' });
        const out = await r.runner.run(job(plain.base), cfg(), null);
        expect(out.samples).toEqual([66]);
        expect(r.timeline).toEqual(['launch:bench', 'terminate:i-fake1']);
      } finally {
        plain.cleanup();
      }
    });

    it('an explicit package manager without its lockfile at the base ref is a loud configuration error', async () => {
      const r = rig(fx);
      await expect(r.runner.run(job(fx.base), cfg({ deps: 'npm' }), null)).rejects.toThrow(/evaluator\.deps is "npm" but bench\.ref has no package-lock\.json/);
      expect(r.timeline).toEqual([]); // nothing was launched for a misconfiguration
    });
  });

  describe('integrity on the way back', () => {
    it('refuses a snapshot whose bytes changed after the manifest was written (sha256 on the instance)', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const m = JSON.parse(readFileSync(join(r.s3, BUCKET, manifestKey(keyFor(LOCK))), 'utf8')) as DepsManifest;
      writeFileSync(join(r.s3, BUCKET, m.archive), 'not the archive');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/sha256 mismatch for the dependency snapshot/);
      expect(r.compute.terminated.length).toBe(r.compute.launched.length);
    });

    it('refuses to restore on an image with another node than the builder had', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const p = join(r.s3, BUCKET, manifestKey(keyFor(LOCK)));
      writeFileSync(p, readFileSync(p, 'utf8').replace(`"node":"${process.version}"`, '"node":"v1.2.3"'));
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/built with node v1\.2\.3/);
    });

    it('refuses a snapshot that contains symlinks', async () => {
      const r = rig(fx);
      // a builder that was tricked into leaving a symlink in the store: simulate by editing the archive and the manifest consistently
      await r.runner.run(job(fx.base), cfg(), null);
      const key = keyFor(LOCK);
      const mp = join(r.s3, BUCKET, manifestKey(key));
      const m = JSON.parse(readFileSync(mp, 'utf8')) as DepsManifest;
      const work = join(fx.dir, `evil-${seq}`);
      mkdirSync(join(work, 'v10', 'files'), { recursive: true });
      execFileSync('ln', ['-s', '/etc', join(work, 'v10', 'files', 'x')]);
      const evil = join(fx.dir, `evil-${seq}.tgz`);
      execFileSync('tar', ['-czf', evil, '-C', work, '.']);
      const bytes = readFileSync(evil);
      const sum = sha256Buf(bytes);
      const archive = `bench-deps/${key}/${sum}.tgz`;
      mkdirSync(join(r.s3, BUCKET, `bench-deps/${key}`), { recursive: true });
      writeFileSync(join(r.s3, BUCKET, archive), bytes);
      writeFileSync(mp, JSON.stringify({ ...m, compression: 'gzip', archive, sha256: sum, bytes: bytes.length }));
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/contains symlinks/);
    });
  });

  describe('compression', () => {
    it('uses zstd when the image has it (shimmed here), and the restore uses it too', async () => {
      const zlog = join(fx.dir, `zstd-${++seq}.log`);
      const r = rig(fx, {}, {
        zstd: `echo "zstd $*" >> ${zlog}\nif [ "$1" = -dc ]; then shift; [ "$1" = -- ] && shift; exec gzip -dc "$1"; fi\nout=; while [ $# -gt 0 ]; do [ "$1" = -o ] && out="$2"; shift; done\ngzip -c > "$out"`,
      });
      await r.runner.run(job(fx.base), cfg(), null);
      const key = keyFor(LOCK);
      const m = JSON.parse(readFileSync(join(r.s3, BUCKET, manifestKey(key)), 'utf8')) as DepsManifest;
      expect(m.compression).toBe('zstd');
      expect(m.archive).toMatch(/\.tar\.zst$/);
      r.timeline.length = 0;
      const out = await r.runner.run(job(fx.base), cfg(), null);
      expect(out.samples).toEqual([77]);
      expect(readFileSync(zlog, 'utf8')).toMatch(/zstd -T0 -3 -q -o .*\nzstd -dc -- /);
    });
  });

  describe('npm', () => {
    it('snapshots the npm cache (ci --ignore-scripts into a throwaway tree) and installs offline from it', async () => {
      const npmFx = makeFixtureRepo({ 'package-lock.json': '{"lockfileVersion":3}\n', 'package.json': '{"name":"fx"}\n' });
      try {
        const r = rig(npmFx, { setup: 'npm ci && echo 77 > src/p95.txt' });
        const out = await r.runner.run(job(npmFx.base), cfg(), null);
        expect(out.samples).toEqual([77]);
        expect(r.timeline.slice(0, 3)).toEqual(['launch:deps', 'terminate:i-fake1', 'launch:bench']);
        const log = r.shimLog();
        expect(log).toMatch(/ci cwd=.*cache=.*\/store offline=unset .*ignore_scripts=true/);
        expect(log).toMatch(/ci cwd=.* offline=true logs=.*\.ratchet-deps\/npm-logs/);
        const key = r.objects('bench-deps/').find((k) => k.endsWith('.json'))!;
        expect(key).toMatch(/^bench-deps\/npm-[0-9a-f]{40}\.json$/);
        const m = JSON.parse(readFileSync(join(r.s3, BUCKET, key), 'utf8')) as DepsManifest;
        const listing = listArchive(join(r.s3, BUCKET, m.archive));
        expect(listing).toContain('_cacache/content/left-pad');
        expect(listing).not.toContain('_logs');
        expect(listing).not.toContain('_update-notifier-last-checked');
        // and a changed package-lock.json is blocked the same way
        const sha = npmFx.candidate({ 'package-lock.json': '{"lockfileVersion":3,"x":1}\n' });
        const blocked = await r.runner.run(job(sha), cfg(), null);
        expect(blocked.guardrails[0]).toMatchObject({ name: 'dependencies unchanged', passed: false });
        expect(blocked.guardrails[0]!.detail).toContain('package-lock.json');
      } finally {
        npmFx.cleanup();
      }
    });
  });

  describe("isolation: 'user' (OS tools shimmed)", () => {
    const userShims = (l: string): Record<string, string> => ({
      id: `case "$2" in ratchet-bench|ratchet-holdout|ratchet-deps) [ "$1" = -u ] && exit 0;; esac; exec /usr/bin/id "$@"`,
      iptables: `echo "iptables $*" >> ${l}; exit 0`,
      setpriv: `echo "setpriv $*" >> ${l}; if [[ "$*" == *"/dev/tcp/169.254.169.254"* ]]; then exit 1; fi; if [[ "$*" == *"--reuid=ratchet-bench"* && "$*" == *"holdout/.probe"* ]]; then exit 1; fi; while [ "$1" != -- ]; do shift; done; shift; exec "$@"`,
      pkill: 'exit 0',
      chown: `echo "chown $*" >> ${l}; exit 0`,
      'ratchet-candidate': `if [ "$*" = "id -un" ]; then echo ratchet-bench; exit 0; fi; exec env "$@"`,
    });

    it('builds as ratchet-deps (IMDS rule checked first) and restores as root before the candidate user runs setup', async () => {
      const l = join(fx.dir, `userlog-${++seq}.log`);
      const r = rig(fx, { isolation: 'user' }, userShims(l));
      const out = await r.runner.run(job(fx.base), cfg(), null);
      expect(out.samples).toEqual([77]);
      const text = readFileSync(l, 'utf8');
      expect(text).toContain('iptables -C OUTPUT -m owner --uid-owner ratchet-deps -d 169.254.169.254 -j REJECT');
      expect(text).toMatch(/setpriv --reuid=ratchet-deps --regid=ratchet-deps --clear-groups --no-new-privs -- env -i PATH=/);
      expect(text).toMatch(/chown -hR root:root .*\/deps/); // the restored store belongs to root
      expect(text).toMatch(/chown -R ratchet-bench: .*\.ratchet-deps/); // the candidate user owns only its scratch dir
      // the fetch (as ratchet-deps) happened before the restore (root) which happened before setup (ratchet-bench)
      const fetchAt = text.indexOf('--reuid=ratchet-deps');
      const restoreAt = text.indexOf('chown -hR root:root');
      const setupAt = text.indexOf('--reuid=ratchet-bench --regid=ratchet-bench --clear-groups --no-new-privs -- env -u AWS_ACCESS_KEY_ID', restoreAt);
      expect(fetchAt).toBeGreaterThan(-1);
      expect(restoreAt).toBeGreaterThan(fetchAt);
      expect(setupAt).toBeGreaterThan(restoreAt);
    });

    it('refuses to build when the image has no deps user or the IMDS rule is missing', async () => {
      const bad = rig(fx, { isolation: 'user' }, { id: 'exit 1' });
      await expect(bad.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/snapshot user ratchet-deps does not exist/);
      expect(bad.timeline).toEqual(['launch:deps', 'terminate:i-fake1']);
      const noRule = rig(fx, { isolation: 'user' }, { id: 'exit 0', iptables: 'exit 1' });
      await expect(noRule.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/IMDS is not blocked for ratchet-deps/);
    });
  });
});

function sha256Buf(b: Buffer): string {
  return execFileSync('sha256sum', { input: b, encoding: 'utf8' }).split(' ')[0]!;
}

describe('dependency changes in the LocalBenchRunner (parity with the real bench)', () => {
  let fx: FixtureRepo;
  beforeAll(() => {
    fx = makeFixtureRepo(FILES);
  });
  afterAll(() => fx.cleanup());

  const local = (over: { setup?: string | undefined } = { setup: 'echo 66 > src/p95.txt' }) => {
    const store = new MemoryStore();
    const marker = join(fx.dir, `local-marker-${++seq}`);
    const runner = new LocalBenchRunner({
      git: fx.host,
      scratchRoot: join(fx.dir, `local-scratch-${seq}`),
      clock: realClock,
      artifacts: store,
      ...(over.setup ? { setup: `touch ${marker}; ${over.setup}` } : {}),
    });
    return { runner, store, marker };
  };

  it('blocks a candidate whose lockfile differs from bench.ref, without running setup', async () => {
    const { runner, marker } = local();
    const sha = fx.candidate({ 'pnpm-lock.yaml': LOCK + 'dep:evil\n' });
    const out = await runner.run(job(sha), cfg(), null);
    expect(out.runs).toEqual([]);
    expect(out.guardrails).toEqual([
      {
        name: 'dependencies unchanged',
        passed: false,
        detail: expect.stringContaining('dependency changes are not allowed in v0: the bench has no network (pnpm-lock.yaml differs from ratchet/bench)'),
      },
    ]);
    expect(existsSync(marker)).toBe(false);
  });

  it('gives the same verdict text as the remote runner', async () => {
    const l = local();
    const sha = fx.candidate({ 'pnpm-lock.yaml': 'lockfileVersion: 9\n' });
    const a = await l.runner.run(job(sha), cfg(), null);
    const r = rig(fx);
    const b = await r.runner.run(job(sha), cfg(), null);
    expect(a.guardrails).toEqual(b.guardrails);
  });

  it('lets a candidate through that leaves the lockfile alone', async () => {
    const { runner, marker } = local();
    const sha = fx.candidate({ 'src/p95.txt': '1' });
    const out = await runner.run(job(sha), cfg(), null);
    expect(out.guardrails).toEqual([]);
    expect(out.samples).toEqual([66]);
    expect(existsSync(marker)).toBe(true);
  });

  it('applies only when there is a setup (nothing is installed otherwise), and not with deps: none', async () => {
    const sha = fx.candidate({ 'pnpm-lock.yaml': LOCK + 'dep:evil\n' });
    const noSetup = await local({ setup: undefined }).runner.run(job(sha), cfg(), null);
    expect(noSetup.guardrails).toEqual([]);
    const none = await local().runner.run(job(sha), cfg({ deps: 'none' }), null);
    expect(none.guardrails).toEqual([]);
    expect(none.samples).toEqual([66]);
  });

  it('treats the lockfile as protected: a setup that rewrites it is caught and the file is restored', async () => {
    const { runner } = local({ setup: 'echo dep:sneaky >> pnpm-lock.yaml' });
    const out = await runner.run(job(fx.base), cfg({ guardrails: ['- run: "true"'] }), null);
    expect(out.guardrails[0]).toMatchObject({ name: 'protected files intact', passed: false });
    expect(out.guardrails[0]!.detail).toContain('pnpm-lock.yaml');
  });

  it('an explicit strategy without its lockfile at bench.ref is a configuration error here too', async () => {
    const { runner } = local();
    await expect(runner.run(job(fx.base), cfg({ deps: 'npm' }), null)).rejects.toThrow(/evaluator\.deps is "npm"/);
  });

  it('lockfileChanges is what both runners use', () => {
    expect(lockfileChanges({ 'pnpm-lock.yaml': 'a' }, { 'pnpm-lock.yaml': 'b' })).toEqual(['pnpm-lock.yaml']);
  });
});

