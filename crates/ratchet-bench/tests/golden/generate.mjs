// Generates the golden fixtures in this directory from the TypeScript implementation (the executable spec).
//
// The Rust tests (`tests/golden.rs`) read the JSON files and assert that `ratchet-bench` produces exactly the same text:
// every generated shell script byte for byte (a handful in full, the matrix by sha256), keys and manifests, Dockerfile /
// JSONC parsing, the PCRE a glob list compiles to, ... Regenerate only when the TypeScript behaviour is meant to change:
//
//   cd packages/bench && RATCHET_TS_SRC=$PWD/src ../../node_modules/.bin/tsx \
//     ../../crates/ratchet-bench/tests/golden/generate.mjs
//
// (`tsx` and the packages' node_modules come from `pnpm install` at the repository root.) Inputs come from a seeded
// generator, so a re-run produces identical files.
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const src = process.env.RATCHET_TS_SRC;
if (!src) throw new Error('set RATCHET_TS_SRC to packages/bench/src');
const load = (f) => import(pathToFileURL(join(src, f)).href);
const [shell, rs, is, ds, deps, image, nodePath, pipeline] = await Promise.all([
  load('shell.ts'),
  load('remote-scripts.ts'),
  load('image-scripts.ts'),
  load('deps-scripts.ts'),
  load('deps.ts'),
  load('image.ts'),
  load('node-path.ts'),
  load('pipeline.ts'),
]);

function mulberry32(a) {
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rnd = mulberry32(20261031);
const pick = (xs) => xs[Math.floor(rnd() * xs.length)];
const int = (lo, hi) => lo + Math.floor(rnd() * (hi - lo + 1));
const write = (name, value) => writeFileSync(join(here, name), JSON.stringify(value) + '\n');
const sha = (s) => createHash('sha256').update(s).digest('hex');

/** Run `f`; a thrown error is recorded by message. */
const run = (f) => {
  try {
    return { output: f() };
  } catch (e) {
    return { error: String(e.message) };
  }
};
/** Full text for the first `FULL` cases, a digest for the rest. */
let fullBudget = 0;
const text = (r) => {
  if (r.error !== undefined) return r;
  const out = { sha256: sha(r.output), length: r.output.length };
  if (fullBudget > 0) {
    fullBudget--;
    out.full = r.output;
  }
  return out;
};

const hex = (c) => c.repeat(64).slice(0, 64);
const staged = (n, files, env) => ({
  uri: `s3://ratchet-test-artifacts/bench-inputs/opt1/job1/${n}`,
  sha256: hex('a'),
  commit: 'c'.repeat(40),
  ...(files ? { files } : {}),
  ...(env ? { env } : {}),
});
const KEY_IMG = 'img-' + 'f'.repeat(40);
const RT_DEFAULT = { tag: `ratchet-env:${KEY_IMG}` };
const RT_CUSTOM = { tag: `ratchet-env:${KEY_IMG}`, readOnlyRoot: false, pidsLimit: 128, tmpfsSize: '512m', helperPath: '/opt/x/launcher', verifyHelperOwner: false, daemonWaitAttempts: 5 };
const restoreImg = (rt, compression = 'zstd') => ({
  uri: `s3://ratchet-test-artifacts/bench-images/${KEY_IMG}/${hex('d')}.${compression === 'zstd' ? 'tar.zst' : 'tar.gz'}`,
  sha256: hex('d'),
  compression,
  imageId: 'sha256:' + hex('e'),
  rt,
});
const STRATEGIES = { pnpm: deps.PNPM, npm: deps.NPM, none: deps.NO_DEPS };
const restoreDeps = (strategy, compression) => ({
  strategy,
  uri: `s3://ratchet-test-artifacts/bench-deps/${strategy}-${'f'.repeat(40)}/${hex('d')}.${compression === 'zstd' ? 'tar.zst' : 'tgz'}`,
  sha256: hex('d'),
  compression,
  node: 'v22.22.0',
  pm: '10.28.0',
});
const toTs = (o) => {
  // JSON inputs carry the strategy as an id; the TypeScript wants the object
  if (o.deps) o = { ...o, deps: { ...o.deps, strategy: STRATEGIES[o.deps.strategy] } };
  return o;
};

// ------------------------------------------------------------------------------------------------ shell helpers
{
  const strings = ['', 'plain', "it's", `a'b"c$d\`e\\f`, "'", "''", 'a b', 'ünï', '😀', 'line\nbreak', '$(rm -rf /)', "'; touch /pwned; '"];
  const wraps = ['x', 'echo hi\nexit 3', 'a\n__RATCHET_STEP_EOF__\nb', ''];
  const refs = ['abc123', '--upload-pack=evil', 'a..b', 'refs/heads/feat-1', 'HEAD~1', 'x y', 'ratchet/bench', '', '.hidden', 'a{b}', 'v1.2.3+meta'];
  write('shell.json', {
    sq: strings.map((s) => ({ input: s, output: shell.sq(s) })),
    wrapStep: wraps.map((b) => ({ input: b, ...run(() => rs.wrapStep(b)) })),
    assertSafeRef: refs.map((r) => ({ input: r, ...run(() => shell.assertSafeRef(r, 'ref')) })),
    localTracePath: ['s3://b/k/trace.json', 's3://b/k/trace', '/x/y/trace.jsonl?sig=1', 'file:///a.b.c/t.tar.gz', 's3://b/k/trace.j$on'].map((u) => ({
      input: u,
      output: rs.localTracePath({ work: '/var/tmp/w/job' }, u),
    })),
    nodeShim: [
      ['/usr/bin/node22', false],
      ["/App's/Ratchet", true],
    ].map(([p, e]) => ({ input: [p, e], output: nodePath.nodeShimScript(p, e) })),
    sentinels: (() => {
      const out = [
        'plain\n@@RATCHET@@ step {"exit":0,"ms":3,"timedOut":false}\nmore\n@@RATCHET@@ run {"exit":1}\n@@RATCHET@@ bad {not json\n@@RATCHET@@ noslot\n@@RATCHET@@ arr [1,2]\n@@RATCHET@@ str "x"',
        '',
        '@@RATCHET@@ checkout {"matched":3}',
      ];
      return out.map((s) => ({ input: s, parsed: rs.parseSentinels(s), stripped: rs.stripSentinels(s) }));
    })(),
  });
}

// ------------------------------------------------------------------------------------------------ protectedPcre
{
  const cases = [];
  const curated = [
    ['bench/**', 'ratchet.yaml', 'tests/contract/**', '**/*.snap', 'docs/*.md', 'src/{a,b}/x?.ts', './lead/', 'SOLUTIONS.md', '[ab]c/**'],
    [],
    ['bench/**'],
    ['[!a-c]x/**', '[a/b]/**', '[z-a]', 'a[', '[]', '[]]', 'x{a,b', 'x{a,b}{c,d}', '{a,b}/**/c', '**/', '**', '*', '?', 'a\\b', '.\\x', '\\\\', '/', './', ''],
    ['ünï/**', '😀/*', 'a b/c', 'a+b', 'a.b', 'a(b)', 'a|b', 'a^b', 'a$b', 'a{b}', 'a}b', 'a]b'],
  ];
  for (const globs of curated) cases.push({ input: globs, ...run(() => rs.protectedPcre(globs)) });
  const alphabet = ['a', 'b', 'c', '/', '/', '*', '*', '?', '[', ']', '{', '}', ',', '!', '.', '\\', '-', '_', ' ', '1', '+', '(', ')', '|', '^', '$', 'é', '😀'];
  for (let i = 0; i < 1500; i++) {
    const globs = [];
    for (let g = int(1, 3); g > 0; g--) {
      let s = '';
      for (let n = int(1, 9); n > 0; n--) s += pick(alphabet);
      globs.push(s);
    }
    cases.push({ input: globs, ...run(() => rs.protectedPcre(globs)) });
  }
  write('pcre.json', cases);
}

// ------------------------------------------------------------------------------------------------ containerRun
{
  const base = { rt: RT_DEFAULT, name: '"$CN"', user: '"$BUID:$BGID"', noNewPrivileges: true, network: 'none', workdir: '"$CODE"', mounts: ['$CODE:$CODE'], command: "'true'" };
  const cases = [
    base,
    { ...base, rt: RT_CUSTOM },
    { ...base, rt: { ...RT_DEFAULT, readOnlyRoot: false } },
    { ...base, network: 'bridge', noNewPrivileges: false, capAdd: ['SETUID', 'SETGID'] },
    { ...base, passEnv: ['A', 'B'], env: ['HOME=$W/home', 'X=$Y'], extra: ['-e', "'K=v'", '${DENV[@]+"${DENV[@]}"}'], mounts: ['$H:$H:ro', '$W/out:$W/out'] },
    { ...base, rt: { tag: 'x' } },
  ];
  write('container_run.json', cases.map((c) => ({ input: c, output: is.containerRun(c) })));
}

// ------------------------------------------------------------------------------------------------ checkout / step scripts
const globsets = [['bench/**'], ['bench/**', 'ratchet.yaml', 'tests/**'], ['**/*.snap', 'docs/*.md', 'src/{a,b}/x?.ts', "it's/**"]];
{
  fullBudget = 26;
  const cases = [];
  const add = (input) => cases.push({ input, ...text(run(() => rs.checkoutScript(toTs(input)))) });
  const mk = (over = {}) => ({
    layout: { work: '/var/tmp/ratchet-bench/opt1-1234' },
    code: staged('code.tgz'),
    bench: staged('bench.tgz'),
    globs: ['bench/**'],
    prune: true,
    isolation: 'user',
    ...over,
  });
  // curated, in full
  add(mk());
  add(mk({ isolation: 'none' }));
  add(mk({ globs: globsets[1], pinned: ['pnpm-lock.yaml', 'pnpm-workspace.yaml', '.npmrc'], prune: false }));
  add(mk({ deps: restoreDeps('pnpm', 'zstd'), pinned: ['pnpm-lock.yaml'] }));
  add(mk({ deps: restoreDeps('npm', 'gzip'), pinned: ['package-lock.json'], isolation: 'none' }));
  add(mk({ image: restoreImg(RT_DEFAULT) }));
  add(mk({ image: restoreImg(RT_CUSTOM, 'gzip'), isolation: 'none' }));
  add(mk({ image: restoreImg(RT_DEFAULT), deps: restoreDeps('pnpm', 'zstd'), pinned: ['pnpm-lock.yaml', '.devcontainer/Dockerfile'] }));
  add(mk({ candidateLauncher: '/x/launcher' }));
  add(mk({ candidateLauncher: "/x'; touch /pwned2; '" }));
  // the matrix
  for (const globs of globsets)
    for (const pinned of [undefined, ['pnpm-lock.yaml', 'pnpm-workspace.yaml']])
      for (const prune of [true, false])
        for (const isolation of ['user', 'none'])
          for (const d of [undefined, restoreDeps('pnpm', 'zstd'), restoreDeps('npm', 'gzip')])
            for (const img of [undefined, restoreImg(RT_DEFAULT), restoreImg(RT_CUSTOM, 'gzip')])
              if (rnd() < 0.5) add(mk({ globs, ...(pinned ? { pinned } : {}), prune, isolation, ...(d ? { deps: d } : {}), ...(img ? { image: img } : {}), ...(rnd() < 0.3 ? { candidateLauncher: '/opt/launcher' } : {}) }));
  // refusals
  add(mk({ code: { ...staged('code.tgz'), uri: 's3://b/../x' } }));
  add(mk({ code: { ...staged('code.tgz'), uri: 'https://example.com/x' } }));
  add(mk({ bench: { ...staged('bench.tgz'), sha256: 'nope' } }));
  add(mk({ bench: { ...staged('bench.tgz'), commit: '--upload-pack=evil' } }));
  add(mk({ deps: { ...restoreDeps('pnpm', 'zstd'), uri: 's3://b/bench-deps/../bench-inputs/x.tgz' } }));
  add(mk({ deps: { ...restoreDeps('pnpm', 'zstd'), sha256: 'nope' } }));
  add(mk({ deps: { ...restoreDeps('pnpm', 'zstd'), node: "v22'; rm -rf /; '" } }));
  add(mk({ image: { ...restoreImg(RT_DEFAULT), uri: 's3://b/bench-inputs/x' } }));
  add(mk({ image: { ...restoreImg(RT_DEFAULT), imageId: 'latest' } }));
  add(mk({ image: restoreImg({ ...RT_DEFAULT, tag: 'ratchet-env:x' }) }));
  add(mk({ image: restoreImg({ ...RT_DEFAULT, tmpfsSize: '2 g' }) }));
  add(mk({ image: restoreImg({ ...RT_DEFAULT, pidsLimit: 10 }) }));
  add(mk({ image: restoreImg({ ...RT_DEFAULT, helperPath: 'rel/path' }) }));
  add(mk({ layout: { work: '/var/tmp/a b' }, image: restoreImg(RT_DEFAULT) }));
  write('checkout.json', cases);
}
{
  fullBudget = 26;
  const cases = [];
  const base = { layout: { work: '/var/tmp/work/job1' }, name: 'bench run 1/3', command: 'k6 run bench/k6/search.js', timeoutMs: 60_000, env: { RATCHET_PHASE: 'confirm', RATCHET_RUN: '1' } };
  const add = (input) => cases.push({ input, ...text(run(() => rs.stepScript(input))) });
  // curated, in full
  add({ ...base, kind: 'bench', holdout: { uri: 's3://sealed/holdout/trace.json' } });
  add({ ...base, kind: 'bench' });
  add({ ...base, kind: 'setup', command: 'pnpm build', name: 'setup' });
  add({ ...base, kind: 'guardrail', name: 'guardrail 1/2: node tests/check.mjs', isolation: 'none' });
  add({ ...base, kind: 'bench', isolation: 'none', holdout: { uri: '/local/trace' } });
  add({ ...base, kind: 'bench', holdout: { uri: 's3://b/k/trace.json' }, container: RT_DEFAULT });
  add({ ...base, kind: 'bench', container: RT_CUSTOM, isolation: 'none' });
  add({ ...base, kind: 'setup', container: RT_DEFAULT, depsMounted: true, env: { npm_config_offline: 'true', A: "it's" } });
  add({ ...base, kind: 'guardrail', container: RT_CUSTOM, isolation: 'none', depsMounted: true });
  add({ ...base, kind: 'bench', holdout: { uri: "s3://b/k'; touch /pwned; '.json" }, candidateLauncher: "/x'; touch /pwned2; '" });
  // the matrix
  for (const kind of ['setup', 'guardrail', 'bench'])
    for (const isolation of [undefined, 'user', 'none'])
      for (const container of [undefined, RT_DEFAULT, RT_CUSTOM])
        for (const holdout of kind === 'bench' ? [undefined, { uri: 's3://sealed/holdout/trace.json' }, { uri: '/x/trace' }] : [undefined])
          for (const depsMounted of [undefined, true])
            for (const name of ['bench run 2/3', 'guardrail 1/1: pnpm test && echo ünï', 'x'.repeat(80)])
              if (rnd() < 0.35)
                add({ ...base, kind, name, ...(isolation ? { isolation } : {}), ...(container ? { container } : {}), ...(holdout ? { holdout } : {}), ...(depsMounted ? { depsMounted } : {}), env: rnd() < 0.5 ? base.env : { Z: '1', A: "b'c", RATCHET_SHA: 'abc' }, timeoutMs: pick([1, 999, 1000, 1001, 60_000, 1_800_000]) });
  // refusals
  add({ ...base, kind: 'setup', holdout: { uri: 's3://b/k' } });
  add({ ...base, kind: 'guardrail', holdout: { uri: 's3://b/k' } });
  add({ ...base, kind: 'bench', env: { RATCHET_TRACE: '/x' } });
  add({ ...base, kind: 'bench', env: { 'BAD NAME': '1' } });
  add({ ...base, kind: 'bench', env: { '1X': '1' } });
  add({ ...base, kind: 'bench', container: { ...RT_DEFAULT, tag: 'nope' } });
  write('step.json', cases);
}

// ------------------------------------------------------------------------------------------------ deps / image jobs
{
  fullBudget = 8;
  const cases = [];
  const mk = (over = {}) => ({
    layout: { work: '/var/tmp/ratchet-bench/deps-opt1-1234' },
    strategy: 'pnpm',
    base: staged('deps-base.tgz'),
    key: 'pnpm-' + 'f'.repeat(40),
    lockfiles: { 'pnpm-lock.yaml': 'a'.repeat(64) },
    image: 'ami-1',
    arch: 'x86_64',
    bucket: 'ratchet-test-artifacts',
    timeoutMs: 600_000,
    isolation: 'user',
    ...over,
  });
  const add = (input) => cases.push({ input, ...text(run(() => ds.depsBuildScript({ ...input, strategy: STRATEGIES[input.strategy] })) ) });
  add(mk());
  add(mk({ isolation: 'none' }));
  add(mk({ strategy: 'npm', key: 'npm-' + 'e'.repeat(40), lockfiles: { 'package-lock.json': 'a'.repeat(64), 'npm-shrinkwrap.json': 'b'.repeat(64) } }));
  add(mk({ container: restoreImg(RT_DEFAULT), key: 'pnpm-' + '1'.repeat(40) }));
  add(mk({ container: restoreImg(RT_CUSTOM, 'gzip'), isolation: 'none' }));
  add(mk({ timeoutMs: 1 }));
  add(mk({ timeoutMs: 1500 }));
  add(mk({ image: "it's", arch: 'arm64' }));
  add(mk({ bucket: 'other.bucket-1' }));
  // refusals
  add(mk({ strategy: 'none' }));
  add(mk({ key: 'pnpm-../../x' }));
  add(mk({ bucket: 'Bad_Bucket;x' }));
  add(mk({ base: staged('code.tgz') }));
  add(mk({ base: { ...staged('deps-base.tgz'), uri: 's3://b/../deps-base.tgz' } }));
  add(mk({ base: { ...staged('deps-base.tgz'), sha256: 'x' } }));
  add(mk({ container: restoreImg({ ...RT_DEFAULT, tag: 'bad' }) }));
  write('deps_job.json', cases);
}
{
  fullBudget = 6;
  const cases = [];
  const spec = (over = {}) => ({ kind: 'dockerfile', path: '.devcontainer/Dockerfile', dockerfile: '.devcontainer/Dockerfile', context: '.devcontainer', args: {}, baseImages: ['node:22'], baseDigests: [], notes: [], ...over });
  const mk = (over = {}) => ({
    work: '/var/tmp/ratchet-bench/image-opt1-1234',
    base: staged('deps-base.tgz'),
    key: KEY_IMG,
    tag: `ratchet-env:${KEY_IMG}`,
    spec: spec(),
    files: { '.devcontainer/Dockerfile': 'h1', '.devcontainer/tools.txt': 'h2' },
    arch: 'x86_64',
    bucket: 'ratchet-test-artifacts',
    timeoutMs: 2_700_000,
    ...over,
  });
  const add = (input) => cases.push({ input, ...text(run(() => is.imageBuildScript(input))) });
  add(mk());
  add(mk({ spec: spec({ args: { NODE: '22', B: 'x/y:z' }, target: 'dev', context: '.', dockerfile: 'ci/Dockerfile', baseDigests: ['sha256:' + 'a'.repeat(64)] }) }));
  add(mk({ spec: { kind: 'image', path: '.devcontainer/devcontainer.json', args: {}, image: 'mcr.microsoft.com/devcontainers/base:ubuntu', baseImages: ['mcr.microsoft.com/devcontainers/base:ubuntu'], baseDigests: [], notes: [] }, arch: 'arm64' }));
  add(mk({ timeoutMs: 999 }));
  add(mk({ spec: spec({ args: { A: "it's" } }) }));
  add(mk({ files: {} }));
  // refusals
  add(mk({ key: 'img-xyz' }));
  add(mk({ tag: 'ratchet-env:img-' + '0'.repeat(40) }));
  add(mk({ bucket: 'A_B' }));
  add(mk({ work: 'relative/path' }));
  add(mk({ spec: spec({ dockerfile: '../escape/Dockerfile' }) }));
  add(mk({ spec: spec({ context: 'a;b' }) }));
  add(mk({ base: staged('code.tgz') }));
  write('image_job.json', cases);

  fullBudget = 0;
  const frag = [];
  for (const rt of [RT_DEFAULT, RT_CUSTOM])
    for (const isolated of [true, false])
      for (const compression of ['zstd', 'gzip']) {
        const input = { restore: restoreImg(rt, compression), work: '/var/tmp/ratchet-bench/opt1-1234', isolated };
        frag.push({ input, ...text(run(() => is.imageRestoreFragment(input.restore, { work: input.work, isolated }))) });
      }
  frag.push({ input: { restore: { ...restoreImg(RT_DEFAULT), uri: 's3://b/x' }, work: '/w', isolated: true }, ...run(() => is.imageRestoreFragment({ ...restoreImg(RT_DEFAULT), uri: 's3://b/x' }, { work: '/w', isolated: true })) });
  fullBudget = 2;
  const ensure = [RT_DEFAULT, RT_CUSTOM].map((rt) => ({ input: restoreImg(rt), ...text(run(() => is.ensureImageLoaded(restoreImg(rt)))) }));
  const fragments = {
    ctr: [RT_DEFAULT, RT_CUSTOM].map((rt) => ({ input: rt, output: is.ctrFunction(rt) })),
    ids: [true, false].map((b) => ({ input: b, output: is.idsFragment(b) })),
    launcherConf: is.launcherConfFragment(),
    helperMounts: [RT_DEFAULT, RT_CUSTOM].map((rt) => ({ input: rt, output: is.helperMounts(rt) })),
  };
  write('image_restore.json', { restore: frag, ensure, fragments });
}

// ------------------------------------------------------------------------------------------------ keys, manifests, parsing
{
  const lock = (n) => ({ 'pnpm-lock.yaml': 'a'.repeat(63) + n });
  const keyCases = [];
  for (const [strategy, lockfiles] of [
    ['pnpm', lock('1')],
    ['pnpm', { ...lock('2'), '.npmrc': 'b'.repeat(64) }],
    ['pnpm', { '.npmrc': 'b'.repeat(64), ...lock('2') }],
    ['npm', { 'package-lock.json': 'a'.repeat(64), 'npm-shrinkwrap.json': 'b'.repeat(64) }],
    ['npm', { 'npm-shrinkwrap.json': 'b'.repeat(64), 'package-lock.json': 'a'.repeat(64) }],
    ['pnpm', {}],
  ])
    for (const image of ['ami-1', KEY_IMG, "it's"])
      for (const arch of ['x86_64', 'arm64'])
        for (const rev of [1, 2]) {
          const input = { strategy, rev, lockfiles, image, arch };
          keyCases.push({ input, output: deps.depsKey({ ...input, strategy: { ...STRATEGIES[strategy], rev } }) });
        }
  const imgCases = [];
  const specs = [
    { kind: 'dockerfile', path: 'Dockerfile', dockerfile: 'Dockerfile', context: '.', args: { A: '1', B: '2' }, baseImages: ['node:22'], baseDigests: [], notes: [] },
    { kind: 'dockerfile', path: 'Dockerfile', dockerfile: 'Dockerfile', context: '.', args: { B: '2', A: '1' }, baseImages: ['node:22'], baseDigests: [], notes: [] },
    { kind: 'dockerfile', path: 'a/Dockerfile', dockerfile: 'a/Dockerfile', context: 'a', target: 'dev', args: {}, baseImages: [], baseDigests: ['sha256:' + 'b'.repeat(64), 'sha256:' + 'a'.repeat(64)], notes: [] },
    { kind: 'image', path: 'd.json', args: {}, image: 'x/y:1', baseImages: ['x/y:1'], baseDigests: [], notes: ['n'] },
  ];
  for (const spec of specs)
    for (const files of [{ Dockerfile: 'h1', 'b.txt': 'h2' }, { 'b.txt': 'h2', Dockerfile: 'h1' }, {}])
      for (const arch of ['x86_64', 'arm64']) imgCases.push({ input: { spec, files, arch }, output: image.imageKey({ spec, files, arch }) });

  // manifests: valid ones, then mutations of the valid ones
  const lockfiles = { 'pnpm-lock.yaml': 'a'.repeat(64) };
  const dInput = { strategy: deps.PNPM, lockfiles, image: 'ami-1', arch: 'x86_64' };
  const dKey = deps.depsKey(dInput);
  const dGood = { v: 1, key: dKey, strategy: 'pnpm', rev: 1, lockfiles, image: 'ami-1', arch: 'x86_64', node: 'v22.22.0', pm: '10.28.0', compression: 'zstd', archive: `bench-deps/${dKey}/${hex('d')}.tar.zst`, sha256: hex('d'), bytes: 1234, createdAt: 1 };
  const dMut = [
    {}, { compression: 'gzip', archive: `bench-deps/${dKey}/${hex('d')}.tgz` }, { v: 2 }, { v: '1' }, { key: 'pnpm-' + '0'.repeat(40) }, { strategy: 'npm' }, { rev: 2 },
    { image: 'ami-2' }, { arch: 'arm64' }, { lockfiles: { 'pnpm-lock.yaml': 'b'.repeat(64) } }, { lockfiles: { ...lockfiles, 'x.yaml': 'c'.repeat(64) } }, { lockfiles: [] }, { lockfiles: null }, { lockfiles: undefined },
    { node: 'v22; rm -rf /' }, { pm: 5 }, { node: undefined }, { compression: 'xz' }, { sha256: 'xyz' }, { sha256: 'D'.repeat(64) },
    { archive: `bench-deps/other/${hex('d')}.tar.zst` }, { archive: `bench-deps/${dKey}/../../x/${hex('d')}.tar.zst` }, { archive: `bench-deps/${dKey}/${'e'.repeat(64)}.tar.zst` }, { archive: `bench-deps/${dKey}/${hex('d')}.tgz` }, { archive: 7 },
    { bytes: -1 }, { bytes: 'x' }, { bytes: 0 }, { bytes: 1e30 }, { createdAt: 'x' }, { createdAt: undefined }, { createdAt: 12.5 },
  ];
  const depsManifests = [
    ...dMut.map((m) => ({ text: JSON.stringify({ ...dGood, ...m }) })),
    { text: 'nope' }, { text: '[]' }, { text: 'null' }, { text: '7' }, { text: '"x"' }, { text: 'true' }, { text: '{}' }, { text: '' },
  ].map((c) => ({ input: { text: c.text, strategy: 'pnpm', lockfiles, image: 'ami-1', arch: 'x86_64', key: dKey }, parsed: deps.parseDepsManifest(c.text, { ...dInput, key: dKey }) }));

  const iSpec = specs[0];
  const iFiles = { Dockerfile: 'h1', 'b.txt': 'h2' };
  const iInput = { spec: iSpec, files: iFiles, arch: 'x86_64' };
  const iKey = image.imageKey(iInput);
  const iGood = { v: 1, key: iKey, rev: 1, arch: 'x86_64', files: iFiles, tag: image.imageTag(iKey), imageId: 'sha256:' + 'c'.repeat(64), compression: 'zstd', archive: `bench-images/${iKey}/${hex('b')}.tar.zst`, sha256: hex('b'), bytes: 10, createdAt: 1 };
  const iMut = [
    {}, { compression: 'gzip', archive: `bench-images/${iKey}/${hex('b')}.tar.gz` }, { compression: 'gzip' }, { v: 2 }, { key: 'img-' + '0'.repeat(40) }, { arch: 'arm64' }, { rev: 2 },
    { files: { Dockerfile: 'evil' } }, { files: undefined }, { files: [] }, { files: 'x' }, { tag: 'ratchet-env:other' }, { imageId: 'latest' }, { imageId: undefined },
    { archive: `bench-images/other/${hex('b')}.tar.zst` }, { archive: `bench-inputs/${iKey}/${hex('b')}.tar.zst` }, { sha256: 'xyz' }, { bytes: -1 }, { bytes: null }, { createdAt: 'x' },
  ];
  const imageManifests = [
    ...iMut.map((m) => JSON.stringify({ ...iGood, ...m })),
    'nope', '[]', 'null', '{}',
  ].map((t) => ({ input: { text: t, ...iInput, key: iKey }, parsed: image.parseImageManifest(t, { ...iInput, key: iKey }) }));
  // an empty expected file set accepts a manifest without `files`
  const emptyIn = { spec: iSpec, files: {}, arch: 'x86_64' };
  const emptyKey = image.imageKey(emptyIn);
  const emptyGood = { ...iGood, key: emptyKey, tag: image.imageTag(emptyKey), archive: `bench-images/${emptyKey}/${hex('b')}.tar.zst`, files: undefined };
  imageManifests.push({ input: { text: JSON.stringify(emptyGood), ...emptyIn, key: emptyKey }, parsed: image.parseImageManifest(JSON.stringify(emptyGood), { ...emptyIn, key: emptyKey }) });

  write('keys.json', {
    depsKeys: keyCases,
    imageKeys: imgCases,
    depsManifests,
    imageManifests,
    arch: ['c7i.2xlarge', 'm7i.large', 't3.medium', 'g5.xlarge', 'c7g.large', 'm6gd.xlarge', 't4g.nano', 'im4gn.large', 'is4gen.medium', 'hpc7g.4xlarge', 'a1.large', '', 'c7gn.large', 'x2gd.large', 'a1', 'mac2.metal'].map((t) => ({ input: t, output: deps.archFromInstanceType(t) })),
    pinned: ['pnpm', 'npm', 'none'].map((s) => ({ input: s, output: deps.pinnedDepsFiles(STRATEGIES[s]) })),
    strategyEnv: ['pnpm', 'npm', 'none'].map((s) => ({ input: s, fetchEnv: STRATEGIES[s].fetchEnv('/w/store'), setupEnv: STRATEGIES[s].setupEnv('/w/deps/store', '/w/home/.ratchet-deps'), fetch: STRATEGIES[s].fetch, version: STRATEGIES[s].version, prune: STRATEGIES[s].prune, restore: STRATEGIES[s].restore })),
    guardrails: {
      deps: deps.dependencyChangeGuardrail(['a', 'b'], 'ratchet/bench'),
      hint: deps.offlineSetupHint(deps.PNPM, 'ratchet/bench'),
      hintNpm: deps.offlineSetupHint(deps.NPM, 'main'),
      env: image.environmentChangeGuardrail(['a'], 'ratchet/bench'),
      envMany: image.environmentChangeGuardrail(Array.from({ length: 12 }, (_, i) => `f${i}`), 'r'),
      container: image.containerSetupHint('ratchet/bench'),
    },
    logKeys: [
      { optimizationId: 'o', candidateId: 'c1', sha: 's', phase: 'confirm', repeats: 1 },
      { optimizationId: 'o', sha: 'abc', phase: 'bench', repeats: 1 },
      { optimizationId: 'o', candidateId: 'x', sha: 's', phase: 'baseline', repeats: 1 },
      { optimizationId: '../o', candidateId: '../../x', sha: 's', phase: 'bench', repeats: 1 },
      { optimizationId: 'ünï😀', candidateId: 'a b', sha: 's', phase: 'bench', repeats: 1 },
    ].map((j) => ({ input: j, output: pipeline.logKey(j) })),
  });
}
{
  const dockerfiles = [
    `# syntax=docker/dockerfile:1
FROM --platform=linux/amd64 node:22@sha256:${'a'.repeat(64)} AS build
COPY package.json pnpm-lock.yaml ./
COPY --chown=1000:1000 tools/ /opt/tools/
RUN pnpm install \\
  # a comment inside a continued line
  --frozen-lockfile
FROM build AS test
FROM scratch
COPY --from=build /app /app
ADD https://example.com/x.tgz /x.tgz
COPY ["a b.txt", "/dst/"]
COPY <<EOF /etc/motd
hello
EOF
`,
    '',
    'FROM a\r\nCOPY x y\r\n',
    'FROM a AS B\nFROM b\nFROM B\nCOPY --from=B /x /y\nCOPY "q r" s\nCOPY \'q2\' s\nADD git@github.com:x/y.git /z\n',
    'FROM scratch\n',
    'FROM a\nCOPY [\n',
    'FROM a\nCOPY --chown=1:1 ["x", "y"] /d\nCOPY --link --chmod=755 ["s1", 2, null, true, {"a":1}] /d\n',
    'from A\ncopy A/b c\nADD ftp://h/x /x\n',
    'FROM a\nCOPY a \\\n  b \\\n  c/\n',
    '  FROM   a  \n\tCOPY\tx\ty  \n',
  ];
  const jsonc = ['{ "a": 1, // x\n "b": [1,2,], /* c */ "c": "http://x//y,}" , }', '{ nope', '', '{"a":"\\"//\\""}', '{/* unterminated', '[1,]', '"x"'];
  const norm = ['./a//b/./c/', '.', '', 'a/../b', '/abs', '../x', 'a/../../b', 'a\\b\\c', 'a\0b', './', '..', 'a/..', '.devcontainer/Dockerfile', '././a'];
  write('parse.json', {
    dockerfiles: dockerfiles.map((t) => ({ input: t, output: image.parseDockerfile(t) })),
    jsonc: jsonc.map((t) => ({ input: t, ...run(() => image.parseJsonc(t)) })),
    normalize: norm.map((p) => ({ input: p, ...run(() => image.normalizeRepoRelative(p, 'x')) })),
  });
}

// ------------------------------------------------------------------------------------------------ resolveImageSource
{
  const root = mkdtempSync(join(tmpdir(), 'ratchet-golden-'));
  const DIGEST = 'sha256:' + 'a'.repeat(64);
  const trees = [
    { files: { '.devcontainer/Dockerfile': `FROM node:22@${DIGEST}\nCOPY tools.txt /opt/tools.txt\nCOPY scripts/ /opt/scripts/\n`, '.devcontainer/tools.txt': 'k6\n', '.devcontainer/scripts/a.sh': 'echo a\n', '.devcontainer/scripts/sub/b.sh': 'echo b\n', '.devcontainer/unrelated.md': 'x\n', 'src/index.ts': 'x\n', 'ratchet.yaml': 'v\n' }, path: '.devcontainer/Dockerfile' },
    { files: { '.devcontainer/Dockerfile': 'FROM node:22\n', 'ratchet.yaml': 'v\n' }, path: './.devcontainer//Dockerfile' },
    { files: { '.devcontainer/devcontainer.json': '{ // t\n "name": "x", "build": { "dockerfile": "Dockerfile", "context": "..", "args": { "NODE": "22" }, "target": "dev", },\n "features": {}, "postCreateCommand": "pnpm i",\n}', '.devcontainer/Dockerfile': 'FROM node:22\nCOPY .devcontainer/tools.txt /t\n', '.devcontainer/tools.txt': 'k6\n' }, path: '.devcontainer/devcontainer.json' },
    { files: { '.devcontainer/devcontainer.json': '{ "image": "mcr.microsoft.com/devcontainers/base:ubuntu" }' }, path: '.devcontainer/devcontainer.json' },
    { files: { 'd.json': '{ "dockerComposeFile": "c.yml" }' }, path: 'd.json' },
    { files: { 'd.json': '{ "name": "x" }' }, path: 'd.json' },
    { files: { 'd.json': '{ nope' }, path: 'd.json' },
    { files: {}, path: 'Dockerfile' },
    { files: { Dockerfile: '# nothing\n' }, path: 'Dockerfile' },
    { files: { 'd.json': '{ "build": { "dockerfile": "D", "args": { "A": "x y;rm" } } }', D: 'FROM a\n' }, path: 'd.json' },
    { files: { Dockerfile: 'FROM node:22\nCOPY . /app\n', 'ratchet.yaml': 'version: 0\n', 'src/a.ts': '1' }, path: 'Dockerfile' },
    { files: { Dockerfile: 'FROM node:22\nCOPY package.json pnpm-lock.yaml ./\n', 'package.json': '{}', 'pnpm-lock.yaml': 'x', 'src/a.ts': '1', 'ratchet.yaml': 'v' }, path: 'Dockerfile' },
    { files: { 'ctx/Dockerfile': 'FROM a\nCOPY *.cfg /etc/\n', 'ctx/a.cfg': '1', 'ctx/b.cfg': '2', 'ctx/c.txt': '3', 'ctx/sub/d.cfg': '4' }, path: 'ctx/Dockerfile' },
    { files: { 'ctx/Dockerfile': 'FROM a\nARG F=x\nCOPY $F /x\n', 'ctx/x': '1', 'ctx/y': '2' }, path: 'ctx/Dockerfile' },
    { files: { 'ctx/Dockerfile': 'FROM a\nCOPY **/*.sh /s/\nCOPY sub/{a,b}.txt /t/\nCOPY [xy].z /z/\n', 'ctx/p.sh': '1', 'ctx/q/r.sh': '2', 'ctx/sub/a.txt': '3', 'ctx/sub/b.txt': '4', 'ctx/sub/c.txt': '5', 'ctx/x.z': '6', 'ctx/.dockerignore': 'node_modules\n' }, path: 'ctx/Dockerfile' },
    { files: { Dockerfile: 'FROM ${BASE}\nCOPY a /a\n', a: '1' }, path: 'Dockerfile' },
    { files: { Dockerfile: 'FROM bad image\nCOPY a /a\n', a: '1' }, path: 'Dockerfile' },
    { files: { Dockerfile: 'FROM a\nCOPY missing /a\n' }, path: 'Dockerfile' },
    { files: { Dockerfile: 'FROM a\n' }, path: '/etc/Dockerfile' },
    { files: { Dockerfile: 'FROM a\n' }, path: '../Dockerfile' },
    { files: { Dockerfile: 'FROM a\n', 'Dockerfile.dockerignore': 'x\n', '.dockerignore': 'y\n' }, path: 'Dockerfile' },
    { files: { 'e/Dockerfile': 'FROM a AS b\nFROM b\nCOPY ../up /u\n', up: '1' }, path: 'e/Dockerfile' },
    { files: { 'e/Dockerfile': 'FROM a\nCOPY /abs/x /u\n', 'e/abs/x': '1' }, path: 'e/Dockerfile' },
  ];
  const out = [];
  let n = 0;
  for (const t of trees) {
    const dir = join(root, `t${n++}`);
    mkdirSync(dir, { recursive: true });
    for (const [f, c] of Object.entries(t.files)) {
      mkdirSync(dirname(join(dir, f)), { recursive: true });
      writeFileSync(join(dir, f), c);
    }
    const r = await image.resolveImageSource(dir, t.path).then(
      (v) => ({ output: v }),
      (e) => ({ error: String(e.message) }),
    );
    // environmentFilesOf on the same tree (the marker hash of a failure contains its message)
    const env = await image.environmentFilesOf(dir, t.path);
    out.push({ input: t, ...r, env });
  }
  // a Dockerfile that is a symlink
  const sl = join(root, 'symlink');
  mkdirSync(sl);
  writeFileSync(join(sl, 'real'), 'FROM a\n');
  symlinkSync('real', join(sl, 'Dockerfile'));
  const r = await image.resolveImageSource(sl, 'Dockerfile').then(
    (v) => ({ output: v }),
    (e) => ({ error: String(e.message) }),
  );
  out.push({ input: { files: { real: 'FROM a\n' }, symlink: { Dockerfile: 'real' }, path: 'Dockerfile' }, ...r, env: await image.environmentFilesOf(sl, 'Dockerfile') });
  rmSync(root, { recursive: true, force: true });
  write('resolve_image.json', out);
}
console.log('golden fixtures written');
