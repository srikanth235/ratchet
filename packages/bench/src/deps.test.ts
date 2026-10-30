import { mkdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import {
  archFromInstanceType,
  dependencyChangeGuardrail,
  depsKey,
  hashNamedFiles,
  lockfileChanges,
  manifestKey,
  NO_DEPS,
  NPM,
  parseDepsManifest,
  PNPM,
  resolveDepsStrategy,
  rootEntries,
  type DepsManifest,
} from './deps.js';
import { depsBuildScript, type DepsBuildInput } from './deps-scripts.js';
import { BenchError } from './errors.js';
import { checkoutScript, type DepsRestore, type StagedArchive } from './remote-scripts.js';
import { rmrf, tmp } from './testkit/repo.js';

const sha = (c: string) => c.repeat(64).slice(0, 64);
const archive = (name: string, files?: Record<string, string>): StagedArchive => ({
  uri: `s3://ratchet-test-artifacts/bench-inputs/opt1/job1/${name}`,
  sha256: sha('a'),
  commit: 'c'.repeat(40),
  ...(files ? { files } : {}),
});

describe('resolveDepsStrategy', () => {
  it('auto: pnpm when there is a pnpm lockfile (it wins over an npm one), npm for package-lock / shrinkwrap, otherwise none', () => {
    expect(resolveDepsStrategy('auto', ['package.json', 'pnpm-lock.yaml']).strategy).toBe(PNPM);
    expect(resolveDepsStrategy('auto', ['pnpm-lock.yaml', 'package-lock.json']).strategy).toBe(PNPM);
    expect(resolveDepsStrategy('auto', ['package.json', 'package-lock.json']).strategy).toBe(NPM);
    expect(resolveDepsStrategy('auto', ['npm-shrinkwrap.json']).strategy).toBe(NPM);
    expect(resolveDepsStrategy('auto', ['package.json', 'src']).strategy).toBe(NO_DEPS);
    expect(resolveDepsStrategy('auto', []).strategy).toBe(NO_DEPS);
  });
  it('auto: yarn.lock is not supported, which is said instead of silently ignored', () => {
    const r = resolveDepsStrategy('auto', ['yarn.lock']);
    expect(r.strategy).toBe(NO_DEPS);
    expect(r.note).toMatch(/yarn\.lock.*pnpm and npm only/);
  });
  it('none always means none, even with a lockfile', () => {
    expect(resolveDepsStrategy('none', ['pnpm-lock.yaml']).strategy).toBe(NO_DEPS);
  });
  it('an explicit package manager demands its lockfile at the base ref (misconfiguration is loud)', () => {
    expect(resolveDepsStrategy('npm', ['package-lock.json']).strategy).toBe(NPM);
    expect(() => resolveDepsStrategy('pnpm', ['package-lock.json'])).toThrow(BenchError);
    expect(() => resolveDepsStrategy('pnpm', ['package-lock.json'])).toThrow(/bench\.ref has no pnpm-lock\.yaml/);
    expect(() => resolveDepsStrategy('npm', [])).toThrow(/package-lock\.json or npm-shrinkwrap\.json/);
  });
  it('the no-op strategy pins nothing, fetches nothing and sets no environment', () => {
    expect(NO_DEPS.lockfiles).toEqual([]);
    expect(NO_DEPS.setupEnv('/s', '/x')).toEqual({});
    expect(NO_DEPS.fetchEnv('/s')).toEqual({});
  });
});

describe('strategies: the package manager runs offline and never from a writable store', () => {
  it('pnpm fetches into the store only (no install, no lifecycle scripts) and refuses to switch pnpm versions', () => {
    expect(PNPM.fetch).toBe('pnpm fetch');
    expect(PNPM.fetchEnv('/w/store')).toMatchObject({ npm_config_store_dir: '/w/store', PNPM_STORE_DIR: '/w/store', npm_config_ignore_scripts: 'true', npm_config_manage_package_manager_versions: 'false' });
  });
  it('pnpm setup gets the restored store, offline semantics, and never hardlinks out of the shared store', () => {
    const env = PNPM.setupEnv('/w/deps/store', '/w/home/.ratchet-deps');
    expect(env).toMatchObject({ npm_config_store_dir: '/w/deps/store', PNPM_STORE_DIR: '/w/deps/store', npm_config_offline: 'true', npm_config_package_import_method: 'clone-or-copy' });
    expect(env).not.toHaveProperty('npm_config_ignore_scripts'); // the candidate's own install scripts do run in setup
  });
  it('pnpm: per-project registrations stay out of the snapshot and become a symlink into candidate-owned scratch on restore', () => {
    expect(PNPM.prune).toContain('projects');
    expect(PNPM.restore).toContain('ln -s "$SCRATCH/pnpm-projects"');
  });
  it('npm installs with --ignore-scripts into a throwaway tree and keeps only the cache; setup is offline with logs in scratch', () => {
    expect(NPM.fetch).toBe('npm ci --ignore-scripts --no-audit --no-fund');
    expect(NPM.fetchEnv('/w/store')).toMatchObject({ npm_config_cache: '/w/store' });
    expect(NPM.setupEnv('/w/deps/store', '/s')).toMatchObject({ npm_config_cache: '/w/deps/store', npm_config_offline: 'true', npm_config_logs_dir: '/s/npm-logs' });
  });
});

describe('hashing and comparing lockfiles', () => {
  const dirs: string[] = [];
  const mk = () => {
    const d = tmp('ratchet-deps-unit-');
    dirs.push(d);
    return d;
  };
  afterAll(() => dirs.forEach(rmrf));

  it('hashes the files that exist, by content', async () => {
    const a = mk();
    writeFileSync(join(a, 'pnpm-lock.yaml'), 'x');
    const h = await hashNamedFiles(a, ['pnpm-lock.yaml', 'package-lock.json']);
    expect(Object.keys(h)).toEqual(['pnpm-lock.yaml']);
    expect(h['pnpm-lock.yaml']).toMatch(/^[0-9a-f]{64}$/);
  });
  it('a lockfile that is a symlink never equals a real file with the same text', async () => {
    const a = mk();
    const b = mk();
    writeFileSync(join(a, 'real'), 'pnpm-lock.yaml');
    writeFileSync(join(a, 'pnpm-lock.yaml'), 'content');
    symlinkSync('/etc/passwd', join(b, 'pnpm-lock.yaml'));
    const ha = await hashNamedFiles(a, ['pnpm-lock.yaml']);
    const hb = await hashNamedFiles(b, ['pnpm-lock.yaml']);
    expect(lockfileChanges(ha, hb)).toEqual(['pnpm-lock.yaml']);
    expect(hb['pnpm-lock.yaml']).toMatch(/^link:/);
  });
  it('rootEntries lists the root of a tree (strategy detection input)', async () => {
    const a = mk();
    mkdirSync(join(a, 'src'));
    writeFileSync(join(a, 'package-lock.json'), '{}');
    expect((await rootEntries(a)).sort()).toEqual(['package-lock.json', 'src']);
  });
  it('lockfileChanges: equal -> none; changed, added and removed all count; result is sorted', () => {
    expect(lockfileChanges({ a: '1' }, { a: '1' })).toEqual([]);
    expect(lockfileChanges({ a: '1' }, { a: '2' })).toEqual(['a']);
    expect(lockfileChanges({ a: '1' }, {})).toEqual(['a']);
    expect(lockfileChanges({}, { b: '1' })).toEqual(['b']);
    expect(lockfileChanges({ z: '1', a: '1' }, { z: '2', a: '2' })).toEqual(['a', 'z']);
  });
  it('the blocking reason says why, and how to fix it', () => {
    const g = dependencyChangeGuardrail(['pnpm-lock.yaml'], 'ratchet/bench');
    expect(g).toMatchObject({ name: 'dependencies unchanged', passed: false });
    expect(g.detail).toContain('dependency changes are not allowed in v0: the bench has no network');
    expect(g.detail).toContain('pnpm-lock.yaml differs from ratchet/bench');
  });
});

describe('snapshot key', () => {
  const base = { strategy: PNPM, lockfiles: { 'pnpm-lock.yaml': 'a'.repeat(64) }, image: 'ami-1', arch: 'x86_64' };
  it('is <strategy>-<40 hex> and stable', () => {
    expect(depsKey(base)).toMatch(/^pnpm-[0-9a-f]{40}$/);
    expect(depsKey(base)).toBe(depsKey({ ...base }));
    expect(manifestKey(depsKey(base))).toBe(`bench-deps/${depsKey(base)}.json`);
  });
  it('depends on the lockfile content, the image, the architecture and the strategy (and its revision)', () => {
    const k = depsKey(base);
    expect(depsKey({ ...base, lockfiles: { 'pnpm-lock.yaml': 'b'.repeat(64) } })).not.toBe(k);
    expect(depsKey({ ...base, image: 'ami-2' })).not.toBe(k);
    expect(depsKey({ ...base, arch: 'arm64' })).not.toBe(k);
    expect(depsKey({ ...base, strategy: { ...PNPM, rev: PNPM.rev + 1 } })).not.toBe(k);
    expect(depsKey({ ...base, strategy: NPM, lockfiles: { 'package-lock.json': 'a'.repeat(64) } })).toMatch(/^npm-/);
  });
  it('does not depend on the order lockfiles were listed in', () => {
    const l1 = { 'package-lock.json': 'a'.repeat(64), 'npm-shrinkwrap.json': 'b'.repeat(64) };
    const l2 = { 'npm-shrinkwrap.json': 'b'.repeat(64), 'package-lock.json': 'a'.repeat(64) };
    expect(depsKey({ ...base, strategy: NPM, lockfiles: l1 })).toBe(depsKey({ ...base, strategy: NPM, lockfiles: l2 }));
  });
  it.each([
    ['c7i.2xlarge', 'x86_64'],
    ['m7i.large', 'x86_64'],
    ['t3.medium', 'x86_64'],
    ['g5.xlarge', 'x86_64'],
    ['c7g.large', 'arm64'],
    ['m6gd.xlarge', 'arm64'],
    ['t4g.nano', 'arm64'],
    ['im4gn.large', 'arm64'],
    ['is4gen.medium', 'arm64'],
    ['hpc7g.4xlarge', 'arm64'],
    ['a1.large', 'arm64'],
  ])('%s is %s', (type, arch) => {
    expect(archFromInstanceType(type)).toBe(arch);
  });
});

describe('manifest validation (the controller trusts its own hashes, not the bucket)', () => {
  const lockfiles = { 'pnpm-lock.yaml': 'a'.repeat(64) };
  const input = { strategy: PNPM, lockfiles, image: 'ami-1', arch: 'x86_64' };
  const key = depsKey(input);
  const digest = 'd'.repeat(64);
  const good: DepsManifest = {
    v: 1,
    key,
    strategy: 'pnpm',
    rev: PNPM.rev,
    lockfiles,
    image: 'ami-1',
    arch: 'x86_64',
    node: 'v22.22.0',
    pm: '10.28.0',
    compression: 'zstd',
    archive: `bench-deps/${key}/${digest}.tar.zst`,
    sha256: digest,
    bytes: 1234,
    createdAt: 1,
  };
  const check = (m: Partial<Record<keyof DepsManifest, unknown>> | string) =>
    parseDepsManifest(typeof m === 'string' ? m : JSON.stringify({ ...good, ...m }), { ...input, key });

  it('accepts what the snapshot job writes', () => {
    expect(check({})).toEqual({ ok: true, manifest: good });
    expect(check({ compression: 'gzip', archive: `bench-deps/${key}/${digest}.tgz` }).ok).toBe(true);
  });
  it.each([
    ['not json', 'nope', /not JSON/],
    ['another key', { key: 'pnpm-' + '0'.repeat(40) }, /another key/],
    ['other lockfile hashes', { lockfiles: { 'pnpm-lock.yaml': 'b'.repeat(64) } }, /different lockfile hashes/],
    ['an extra lockfile', { lockfiles: { ...lockfiles, 'x.yaml': 'c'.repeat(64) } }, /different lockfile hashes/],
    ['another image', { image: 'ami-2' }, /another image/],
    ['another arch', { arch: 'arm64' }, /another image or architecture/],
    ['another strategy revision', { rev: PNPM.rev + 1 }, /strategy revision/],
    ['an archive outside its prefix', { archive: `bench-deps/other/${digest}.tar.zst` }, /outside its own prefix/],
    ['a traversing archive', { archive: `bench-deps/${key}/../../x/${digest}.tar.zst` }, /outside its own prefix/],
    ['an archive of another digest', { archive: `bench-deps/${key}/${'e'.repeat(64)}.tar.zst` }, /outside its own prefix/],
    ['an extension that does not match the compression', { archive: `bench-deps/${key}/${digest}.tgz` }, /outside its own prefix/],
    ['a bad digest', { sha256: 'xyz' }, /sha256/],
    ['a shell-looking node version', { node: 'v22; rm -rf /' }, /tool versions/],
    ['an unknown compression', { compression: 'xz' }, /compression/],
    ['a negative size', { bytes: -1 }, /size/],
  ] as const)('rejects %s', (_name, m, error) => {
    const r = check(m as never);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatch(error);
  });
});

describe('the dependency-snapshot job script', () => {
  const key = 'pnpm-' + 'f'.repeat(40);
  const input = (over: Partial<DepsBuildInput> = {}): DepsBuildInput => ({
    layout: { work: '/var/tmp/ratchet-bench/deps-opt1-1234' },
    strategy: PNPM,
    base: archive('deps-base.tgz'),
    key,
    lockfiles: { 'pnpm-lock.yaml': 'a'.repeat(64) },
    image: 'ami-1',
    arch: 'x86_64',
    bucket: 'ratchet-test-artifacts',
    timeoutMs: 600_000,
    isolation: 'user',
    ...over,
  });
  const script = depsBuildScript(input());

  it('only ever downloads the staged BASE tree: no candidate archive, no git, no other bucket prefix', () => {
    expect(script.match(/aws s3 cp --only-show-errors 's3:\/\/[^']+' "\$W\/in\/base\.tgz"/g)).toHaveLength(1);
    expect(script).toContain("s3://ratchet-test-artifacts/bench-inputs/opt1/job1/deps-base.tgz");
    for (const needle of ['code.tgz', 'bench.tgz', 'git ', 'bench-results', 'sealed', 'holdout', 'RATCHET_TRACE']) expect(script).not.toContain(needle);
  });
  it('refuses to be built for the candidate archive or anything but the deps-base basename', () => {
    expect(() => depsBuildScript(input({ base: archive('code.tgz') }))).toThrow(/ending in deps-base\.tgz/);
    expect(() => depsBuildScript(input({ base: { ...archive('deps-base.tgz'), uri: 's3://b/../deps-base.tgz' } }))).toThrow(BenchError);
  });
  it('verifies the digest before unpacking anything', () => {
    expect(script.indexOf('sha256sum')).toBeGreaterThan(-1);
    expect(script.indexOf('sha256sum')).toBeLessThan(script.indexOf('tar -xzf'));
  });
  it('runs the fetch as the unprivileged user with no new privileges and a clean environment', () => {
    expect(script).toContain('DU=\'ratchet-deps\'');
    expect(script).toContain('setpriv --reuid="$DU" --regid="$DU" --clear-groups --no-new-privs -- env -i PATH="$PATH"');
    expect(script).not.toMatch(/AWS_/); // credentials never reach the unprivileged user (env -i; and the shell holds none)
    // only the proxy variables are passed through
    expect(script).toContain('for v in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy NO_PROXY no_proxy NODE_USE_ENV_PROXY');
  });
  it('checks the user exists and cannot reach instance metadata BEFORE it runs anything as that user', () => {
    const idx = (s: string) => {
      const i = script.indexOf(s);
      expect(i, s).toBeGreaterThan(-1);
      return i;
    };
    const run = idx('setpriv --reuid="$DU" --regid="$DU" --clear-groups --no-new-privs -- env -i');
    expect(idx('id -u "$DU"')).toBeLessThan(run);
    expect(idx('iptables -C OUTPUT -m owner --uid-owner "$DU" -d 169.254.169.254 -j REJECT')).toBeLessThan(run);
    expect(idx('/dev/tcp/169.254.169.254/80')).toBeLessThan(run);
    expect(script).toContain('exit 13');
    expect(script).toContain('exit 14');
  });
  it('executes only the strategy fetch (pnpm fetch): no install, no run, no candidate command', () => {
    expect(script).toContain("bash -c 'pnpm fetch'");
    expect(script).not.toMatch(/pnpm (install|i|run|build|exec|dlx)/);
    expect(script).toContain("npm_config_ignore_scripts='true'");
    expect(script).toContain("npm_config_store_dir='/var/tmp/ratchet-bench/deps-opt1-1234/store'");
    expect(depsBuildScript(input({ strategy: NPM, key: 'npm-' + 'e'.repeat(40), lockfiles: { 'package-lock.json': 'a'.repeat(64) } }))).toContain("bash -c 'npm ci --ignore-scripts --no-audit --no-fund'");
  });
  it('packs and uploads as root only after the fetch succeeded, archive first and manifest last', () => {
    const fetchDone = script.indexOf('the package fetch (pnpm) exited with status');
    const pack = script.indexOf('tar -C "$STORE"');
    const uploadArchive = script.indexOf('"s3://ratchet-test-artifacts/$ARCHIVE"');
    const uploadManifest = script.indexOf(`s3://ratchet-test-artifacts/bench-deps/${key}.json`);
    expect(fetchDone).toBeGreaterThan(-1);
    expect(fetchDone).toBeLessThan(pack);
    expect(pack).toBeLessThan(uploadArchive);
    expect(uploadArchive).toBeLessThan(uploadManifest);
    // the archive path is content-addressed under its own key prefix
    expect(script).toContain(`ARCHIVE='bench-deps/${key}'/"$SHA.$EXT"`);
  });
  it('prunes per-project state and refuses symlinks before packing', () => {
    expect(script.indexOf(PNPM.prune)).toBeLessThan(script.indexOf('find "$STORE" -type l'));
    expect(script.indexOf('find "$STORE" -type l')).toBeLessThan(script.indexOf('tar -C "$STORE"'));
  });
  it('writes a manifest that carries the controller-computed key material', () => {
    expect(script).toContain(JSON.stringify({ v: 1, key, strategy: 'pnpm', rev: PNPM.rev, lockfiles: { 'pnpm-lock.yaml': 'a'.repeat(64) }, image: 'ami-1', arch: 'x86_64' }).slice(0, -1));
  });
  it('rejects keys, buckets and strategies that could change what is written', () => {
    expect(() => depsBuildScript(input({ key: 'pnpm-../../x' }))).toThrow(/not a plain key/);
    expect(() => depsBuildScript(input({ bucket: 'Bad_Bucket;x' }))).toThrow(/valid bucket/);
    expect(() => depsBuildScript(input({ strategy: NO_DEPS }))).toThrow(/nothing to snapshot/);
  });
  it('isolation none (tests only) skips the OS-user checks and still runs the fetch', () => {
    const none = depsBuildScript(input({ isolation: 'none' }));
    expect(none).not.toContain('setpriv');
    expect(none).not.toContain('iptables');
    expect(none).toContain("bash -c 'pnpm fetch'");
  });
});

describe('the snapshot restore inside the checkout script', () => {
  const restore: DepsRestore = {
    strategy: PNPM,
    uri: `s3://ratchet-test-artifacts/bench-deps/${'pnpm-' + 'f'.repeat(40)}/${'d'.repeat(64)}.tar.zst`,
    sha256: 'd'.repeat(64),
    compression: 'zstd',
    node: 'v22.22.0',
    pm: '10.28.0',
  };
  const checkout = (over: Partial<Parameters<typeof checkoutScript>[0]> = {}) =>
    checkoutScript({
      layout: { work: '/var/tmp/ratchet-bench/opt1-1234' },
      code: archive('code.tgz'),
      bench: archive('bench.tgz'),
      globs: ['bench/**'],
      pinned: PNPM.lockfiles,
      deps: restore,
      prune: true,
      isolation: 'user',
      ...over,
    });
  const script = checkout();
  const at = (s: string) => {
    const i = script.indexOf(s);
    expect(i, s).toBeGreaterThan(-1);
    return i;
  };

  it('happens in the root checkout script: after the isolation checks, before the script reports, and never through setpriv', () => {
    const fragment = script.slice(at('# ---- dependency snapshot (root)'), at('emit checkout'));
    expect(fragment).not.toContain('setpriv');
    expect(fragment).not.toContain('sudo');
    expect(at('# ---- dependency snapshot (root)')).toBeGreaterThan(at('the holdout directory is readable by'));
    expect(at('# ---- dependency snapshot (root)')).toBeGreaterThan(at('apply init'));
    expect(at('# ---- dependency snapshot (root)')).toBeLessThan(at('emit checkout'));
  });
  it('verifies the digest and the tool versions before extracting, and refuses symlinks after', () => {
    expect(at('sha256sum -- "$D/in/snapshot"')).toBeLessThan(at('zstd -dc'));
    expect(at('node --version')).toBeLessThan(at('zstd -dc'));
    expect(at('pnpm --version')).toBeLessThan(at('zstd -dc'));
    expect(at('zstd -dc')).toBeLessThan(at('find "$STORE" -type l'));
    expect(script).toContain("= 'v22.22.0'");
    expect(script).toContain("= '10.28.0'");
  });
  it('hands the store to root and removes group/other write; the only thing the bench user owns is scratch outside the store', () => {
    expect(script).toContain('chown -R "$BU:" "$SCRATCH"');
    expect(script).toContain('chown -hR root:root "$D"');
    expect(script).toContain('chmod -R go-w,a+rX "$D"');
    expect(script).toContain('SCRATCH="$W/home/.ratchet-deps"');
    expect(script).toContain('D="$W/deps"; STORE="$D/store"');
    expect(at('chown -hR root:root "$D"')).toBeLessThan(at('chmod -R go-w,a+rX "$D"'));
    // the root chown covers the store, scratch is not under it
    expect(script).not.toContain('chown -R "$BU:" "$D"');
  });
  it('wires the strategy restore step before locking the store down', () => {
    expect(at(PNPM.restore)).toBeLessThan(at('chown -hR root:root "$D"'));
  });
  it('the lockfile is pinned like a protected path but does not count as the user protecting something', () => {
    expect(script).toContain('RE_USER=');
    expect(script).toContain('matched=$(');
    expect(script).toContain('\\"matched\\":$matched');
    const withPinned = script.match(/^RE='([^']*)'; RE_USER='([^']*)'/m)!;
    expect(withPinned[1]).toContain('pnpm-lock');
    expect(withPinned[2]).not.toContain('pnpm-lock');
  });
  it('a gzip snapshot is extracted without zstd; a zstd one demands it with a clear error', () => {
    const gz = checkout({ deps: { ...restore, compression: 'gzip', uri: restore.uri.replace('.tar.zst', '.tgz') } });
    expect(gz).toContain('tar -xzf "$D/in/snapshot"');
    expect(gz).not.toContain('zstd');
    expect(script).toContain('zstd is missing on this image');
  });
  it('is absent without a snapshot (no deps, no change to the script)', () => {
    const none = checkout({ deps: undefined, pinned: undefined });
    expect(none).not.toContain('dependency snapshot');
    expect(none).not.toContain('"$D/in/snapshot"');
  });
  it.each([
    ['a uri outside bench-deps/', { uri: 's3://ratchet-test-artifacts/bench-inputs/x/code.tgz' }],
    ['a traversing uri', { uri: 's3://b/bench-deps/../bench-inputs/x.tgz' }],
    ['a bad digest', { sha256: 'nope' }],
    ['a shell-looking version', { node: "v22'; rm -rf /; '" }],
  ])('rejects %s', (_n, over) => {
    expect(() => checkout({ deps: { ...restore, ...over } })).toThrow(BenchError);
  });
});
