import { mkdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { BenchError } from './errors.js';
import {
  environmentChangeGuardrail,
  environmentChanges,
  environmentFilesOf,
  imageKey,
  imageManifestKey,
  imageTag,
  normalizeRepoRelative,
  parseDockerfile,
  parseImageManifest,
  parseJsonc,
  readEnvironment,
  resolveImageSource,
  type ImageKeyInput,
} from './image.js';
import { rmrf, tmp } from './testkit/repo.js';

const DIGEST = 'sha256:' + 'a'.repeat(64);

describe('readEnvironment (defensive: the schema key is added elsewhere)', () => {
  it('returns nothing for configs without an environment', () => {
    expect(readEnvironment({})).toEqual({});
    expect(readEnvironment(undefined)).toEqual({});
    expect(readEnvironment({ environment: 'x' })).toEqual({});
    expect(readEnvironment({ environment: [] })).toEqual({});
  });
  it('reads image, machine, region and network, ignoring junk', () => {
    expect(readEnvironment({ environment: { image: './.devcontainer/Dockerfile', machine: 'c7i.2xlarge', region: 'eu-west-1', network: ['registry.npmjs.org', 3] } })).toEqual({
      image: '.devcontainer/Dockerfile',
      machine: 'c7i.2xlarge',
      region: 'eu-west-1',
      network: ['registry.npmjs.org'],
    });
  });
  it.each(['/etc/passwd', '../outside/Dockerfile', 'a/../../b', '', '  '])('refuses an image path that leaves the repository: %j', (p) => {
    expect(() => readEnvironment({ environment: { image: p } })).toThrow(BenchError);
  });
  it('refuses a non-string image', () => {
    expect(() => readEnvironment({ environment: { image: 3 } })).toThrow(/environment\.image must be a path/);
  });
  it('normalizes', () => {
    expect(normalizeRepoRelative('./a//b/./c/', 'x')).toBe('a/b/c');
    expect(normalizeRepoRelative('.', 'x')).toBe('.');
  });
});

describe('parseJsonc', () => {
  it('accepts comments and trailing commas but leaves strings alone', () => {
    const v = parseJsonc(`{
      // the toolchain
      "build": { "dockerfile": "Dockerfile", /* inline */ "args": { "URL": "http://x//y,}" }, },
      "list": [1, 2,],
    }`);
    expect(v).toEqual({ build: { dockerfile: 'Dockerfile', args: { URL: 'http://x//y,}' } }, list: [1, 2] });
  });
  it('throws on garbage', () => {
    expect(() => parseJsonc('{ nope')).toThrow();
  });
});

describe('parseDockerfile', () => {
  it('finds base images (not stages, not scratch) and the context sources that are copied', () => {
    const info = parseDockerfile(`# syntax=docker/dockerfile:1
FROM --platform=linux/amd64 node:22@${DIGEST} AS build
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
`);
    expect(info.baseImages).toEqual([`node:22@${DIGEST}`]);
    expect(info.copySources).toEqual(['package.json', 'pnpm-lock.yaml', 'tools/', 'a b.txt']);
  });
});

describe('resolveImageSource', () => {
  const dirs: string[] = [];
  const tree = (files: Record<string, string>): string => {
    const d = tmp('ratchet-image-');
    dirs.push(d);
    for (const [f, c] of Object.entries(files)) {
      mkdirSync(dirname(join(d, f)), { recursive: true });
      writeFileSync(join(d, f), c);
    }
    return d;
  };
  afterAll(() => dirs.forEach(rmrf));

  const DOCKERFILE = `FROM node:22@${DIGEST}\nCOPY tools.txt /opt/tools.txt\nCOPY scripts/ /opt/scripts/\n`;
  const FILES = {
    '.devcontainer/Dockerfile': DOCKERFILE,
    '.devcontainer/tools.txt': 'k6\n',
    '.devcontainer/scripts/a.sh': 'echo a\n',
    '.devcontainer/scripts/sub/b.sh': 'echo b\n',
    '.devcontainer/unrelated.md': 'not copied\n',
    'src/index.ts': 'export {}\n',
    'ratchet.yaml': 'version: 0\n',
  };

  it('hashes the Dockerfile and exactly the context files it copies, with the context next to the Dockerfile', async () => {
    const r = await resolveImageSource(tree(FILES), '.devcontainer/Dockerfile');
    expect(r.spec).toMatchObject({ kind: 'dockerfile', dockerfile: '.devcontainer/Dockerfile', context: '.devcontainer', baseDigests: [DIGEST], notes: [] });
    expect(Object.keys(r.files).sort()).toEqual(['.devcontainer/Dockerfile', '.devcontainer/scripts/a.sh', '.devcontainer/scripts/sub/b.sh', '.devcontainer/tools.txt']);
  });

  it('notes an unpinned base image (rebuilds are keyed on the Dockerfile only)', async () => {
    const r = await resolveImageSource(tree({ ...FILES, '.devcontainer/Dockerfile': 'FROM node:22\n' }), '.devcontainer/Dockerfile');
    expect(r.spec.baseDigests).toEqual([]);
    expect(r.spec.notes.join(' ')).toContain('not pinned by digest');
  });

  it('reads a devcontainer.json (JSONC): dockerfile and context relative to it, build args and target', async () => {
    const r = await resolveImageSource(
      tree({
        ...FILES,
        '.devcontainer/devcontainer.json': `{
          // toolchain
          "name": "x", "build": { "dockerfile": "Dockerfile", "context": "..", "args": { "NODE": "22" }, "target": "dev", },
          "features": { "ghcr.io/devcontainers/features/node:1": {} },
          "postCreateCommand": "pnpm i",
        }`,
        '.devcontainer/Dockerfile': 'FROM node:22\nCOPY .devcontainer/tools.txt /t\n',
      }),
      '.devcontainer/devcontainer.json',
    );
    expect(r.spec).toMatchObject({ kind: 'dockerfile', dockerfile: '.devcontainer/Dockerfile', context: '.', target: 'dev', args: { NODE: '22' } });
    expect(Object.keys(r.files).sort()).toEqual(['.devcontainer/Dockerfile', '.devcontainer/devcontainer.json', '.devcontainer/tools.txt']);
    expect(r.spec.notes.join(' ')).toContain('features, postCreateCommand');
  });

  it('supports a devcontainer that names a registry image instead of a build', async () => {
    const r = await resolveImageSource(tree({ '.devcontainer/devcontainer.json': '{ "image": "mcr.microsoft.com/devcontainers/base:ubuntu" }' }), '.devcontainer/devcontainer.json');
    expect(r.spec).toMatchObject({ kind: 'image', image: 'mcr.microsoft.com/devcontainers/base:ubuntu' });
    expect(r.spec.notes.join(' ')).toContain('not pinned by digest');
  });

  it('refuses what the bench cannot honor, with a reason', async () => {
    await expect(resolveImageSource(tree({ 'd.json': '{ "dockerComposeFile": "c.yml" }' }), 'd.json')).rejects.toThrow(/dockerComposeFile is not supported/);
    await expect(resolveImageSource(tree({ 'd.json': '{ "name": "x" }' }), 'd.json')).rejects.toThrow(/needs "build.dockerfile"/);
    await expect(resolveImageSource(tree({ 'd.json': '{ nope' }), 'd.json')).rejects.toThrow(/not valid JSON/);
    await expect(resolveImageSource(tree({}), 'Dockerfile')).rejects.toThrow(/does not exist/);
    await expect(resolveImageSource(tree({ 'Dockerfile': '# nothing\n' }), 'Dockerfile')).rejects.toThrow(/no FROM/);
    await expect(resolveImageSource(tree({ 'd.json': '{ "build": { "dockerfile": "D", "args": { "A": "x y;rm" } } }', D: 'FROM a\n' }), 'd.json')).rejects.toThrow(/not a plain build argument/);
  });

  it('refuses a Dockerfile that copies the repository (the code under optimization) into the image', async () => {
    const t = tree({ Dockerfile: 'FROM node:22\nCOPY . /app\n', 'ratchet.yaml': 'version: 0\n', 'src/a.ts': '1' });
    await expect(resolveImageSource(t, 'Dockerfile')).rejects.toThrow(/copies ratchet\.yaml \(or the whole repository\)/);
  });

  it('a root-level Dockerfile may copy a manifest (lockfile) without copying the repository', async () => {
    const r = await resolveImageSource(tree({ Dockerfile: 'FROM node:22\nCOPY package.json pnpm-lock.yaml ./\n', 'package.json': '{}', 'pnpm-lock.yaml': 'x', 'src/a.ts': '1', 'ratchet.yaml': 'v' }), 'Dockerfile');
    expect(Object.keys(r.files).sort()).toEqual(['Dockerfile', 'package.json', 'pnpm-lock.yaml']);
  });

  it('expands wildcard sources against the context', async () => {
    const r = await resolveImageSource(tree({ 'ctx/Dockerfile': 'FROM a\nCOPY *.cfg /etc/\n', 'ctx/a.cfg': '1', 'ctx/b.cfg': '2', 'ctx/c.txt': '3', 'ctx/sub/d.cfg': '4' }), 'ctx/Dockerfile');
    expect(Object.keys(r.files).sort()).toEqual(['ctx/Dockerfile', 'ctx/a.cfg', 'ctx/b.cfg']);
  });

  it('treats a variable COPY source conservatively: the whole (small) context', async () => {
    const r = await resolveImageSource(tree({ 'ctx/Dockerfile': 'FROM a\nARG F=x\nCOPY $F /x\n', 'ctx/x': '1', 'ctx/y': '2' }), 'ctx/Dockerfile');
    expect(Object.keys(r.files).sort()).toEqual(['ctx/Dockerfile', 'ctx/x', 'ctx/y']);
    expect(r.spec.notes.join(' ')).toContain('uses a variable');
  });

  it('a Dockerfile that is a symlink is refused (the hash must be of real bytes)', async () => {
    const t = tree({ real: 'FROM a\n' });
    symlinkSync('real', join(t, 'Dockerfile'));
    await expect(resolveImageSource(t, 'Dockerfile')).rejects.toThrow(/regular file/);
  });

  describe('environmentChanges (a candidate may not change the environment)', () => {
    it('detects a changed Dockerfile, a changed or added/removed copied file, and an unresolvable environment', async () => {
      const base = (await resolveImageSource(tree(FILES), '.devcontainer/Dockerfile')).files;
      const same = await environmentFilesOf(tree({ ...FILES, 'src/index.ts': 'export const x = 1\n', '.devcontainer/unrelated.md': 'changed but not copied\n' }), '.devcontainer/Dockerfile');
      expect(environmentChanges(base, same)).toEqual([]);

      const df = await environmentFilesOf(tree({ ...FILES, '.devcontainer/Dockerfile': DOCKERFILE + 'RUN curl evil\n' }), '.devcontainer/Dockerfile');
      expect(environmentChanges(base, df)).toEqual(['.devcontainer/Dockerfile']);

      const tools = await environmentFilesOf(tree({ ...FILES, '.devcontainer/tools.txt': 'k6\nevil\n' }), '.devcontainer/Dockerfile');
      expect(environmentChanges(base, tools)).toEqual(['.devcontainer/tools.txt']);

      const added = await environmentFilesOf(tree({ ...FILES, '.devcontainer/scripts/c.sh': 'echo c\n' }), '.devcontainer/Dockerfile');
      expect(environmentChanges(base, added)).toEqual(['.devcontainer/scripts/c.sh']);

      const { '.devcontainer/Dockerfile': _gone, ...rest } = FILES;
      void _gone;
      const deleted = await environmentFilesOf(tree(rest), '.devcontainer/Dockerfile');
      expect(environmentChanges(base, deleted)).toEqual(expect.arrayContaining(['the environment (Dockerfile / devcontainer) no longer resolves']));
    });

    it('the verdict text says environment changes are not allowed in v0, like the lockfile rule', () => {
      const g = environmentChangeGuardrail(['.devcontainer/Dockerfile'], 'ratchet/bench');
      expect(g).toMatchObject({ name: 'environment unchanged', passed: false });
      expect(g.detail).toContain('environment changes are not allowed in v0');
      expect(g.detail).toContain('.devcontainer/Dockerfile');
    });
  });
});

describe('imageKey and the manifest', () => {
  const spec = { kind: 'dockerfile' as const, path: 'Dockerfile', dockerfile: 'Dockerfile', context: '.', args: { A: '1', B: '2' }, baseImages: ['node:22'], baseDigests: [], notes: [] };
  const base: ImageKeyInput = { spec, files: { Dockerfile: 'h1', 'b.txt': 'h2' }, arch: 'x86_64' };

  it('is content addressed: insensitive to key order, sensitive to files, args, base digests and architecture', () => {
    const k = imageKey(base);
    expect(k).toMatch(/^img-[0-9a-f]{40}$/);
    expect(imageKey({ ...base, spec: { ...spec, args: { B: '2', A: '1' } }, files: { 'b.txt': 'h2', Dockerfile: 'h1' } })).toBe(k);
    expect(imageKey({ ...base, files: { ...base.files, Dockerfile: 'other' } })).not.toBe(k);
    expect(imageKey({ ...base, spec: { ...spec, args: { A: '9' } } })).not.toBe(k);
    expect(imageKey({ ...base, spec: { ...spec, baseDigests: [DIGEST] } })).not.toBe(k);
    expect(imageKey({ ...base, arch: 'arm64' })).not.toBe(k);
    expect(imageTag(k)).toBe(`ratchet-env:${k}`);
    expect(imageManifestKey(k)).toBe(`bench-images/${k}.json`);
  });

  const sha = 'b'.repeat(64);
  const good = (key: string) => ({
    v: 1,
    key,
    rev: 1,
    arch: 'x86_64',
    files: base.files,
    tag: imageTag(key),
    imageId: 'sha256:' + 'c'.repeat(64),
    compression: 'zstd',
    archive: `bench-images/${key}/${sha}.tar.zst`,
    sha256: sha,
    bytes: 10,
    createdAt: 1,
  });

  it('accepts a manifest that matches what the controller computed and rejects anything else', () => {
    const key = imageKey(base);
    const expect_ = { ...base, key };
    expect(parseImageManifest(JSON.stringify(good(key)), expect_).ok).toBe(true);
    const bad = (patch: Record<string, unknown>) => parseImageManifest(JSON.stringify({ ...good(key), ...patch }), expect_);
    expect(parseImageManifest('nope', expect_)).toMatchObject({ ok: false });
    expect(bad({ key: 'img-' + '0'.repeat(40) })).toMatchObject({ ok: false, error: expect.stringContaining('another key') });
    expect(bad({ arch: 'arm64' })).toMatchObject({ ok: false, error: expect.stringContaining('architecture') });
    expect(bad({ files: { Dockerfile: 'evil' } })).toMatchObject({ ok: false, error: expect.stringContaining('file hashes') });
    expect(bad({ tag: 'ratchet-env:other' })).toMatchObject({ ok: false });
    expect(bad({ imageId: 'latest' })).toMatchObject({ ok: false, error: expect.stringContaining('image id') });
    expect(bad({ archive: `bench-images/other/${sha}.tar.zst` })).toMatchObject({ ok: false, error: expect.stringContaining('outside its own prefix') });
    expect(bad({ archive: `bench-inputs/${key}/${sha}.tar.zst` })).toMatchObject({ ok: false });
    expect(bad({ compression: 'gzip' })).toMatchObject({ ok: false }); // the archive name says zstd
    expect(bad({ rev: 2 })).toMatchObject({ ok: false });
  });
});
