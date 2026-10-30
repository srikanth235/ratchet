import { createHash } from 'node:crypto';
import { lstat, readFile, readdir } from 'node:fs/promises';
import { join, posix } from 'node:path';
import { globToRegExp, type GuardrailResult } from '@ratchet/core';
import { hashNamedFiles } from './deps.js';
import { BenchError } from './errors.js';

/**
 * The repo's container (`environment.image`) as the bench's toolchain.
 *
 * The bench instances are offline and generic (Docker engine, SSM, zstd, the users). The toolchain a repo needs (node, python,
 * k6, a compiler, ...) therefore arrives as a CONTAINER IMAGE that the repo describes with a Dockerfile or a
 * `devcontainer.json`. It is handled exactly like the dependency snapshot:
 *
 *  - built ONCE from the protected ref (`bench.ref`) by a job on an instance that has egress and applies no candidate input;
 *  - saved (`docker save | zstd`) to the artifacts bucket under a CONTENT-ADDRESSED key and described by a manifest that is
 *    written last (a manifest means a complete archive);
 *  - reused by every candidate and every optimization with the same key;
 *  - loaded offline on bench / confirm instances by root, after verifying its sha256 and its image id.
 *
 * A candidate that changes the Dockerfile / devcontainer or a file the build copies in is BLOCKED ("environment changes are
 * not allowed in v0"), consistent with the lockfile rule.
 */

/** Prefix in the artifacts bucket for saved images. Only the deps identity writes here; the bench identities read. */
export const IMAGE_PREFIX = 'bench-images/';
/** Bumped when build / save / restore change in a way that makes old images wrong: part of every key. */
export const IMAGE_REV = 1;
export const IMAGE_KEY_RE = /^img-[0-9a-f]{40}$/;
/** Tag the image gets in the docker daemon of the job's instance. Content-addressed, so a stale image can never be mistaken for the current one. */
export const imageTag = (key: string): string => `ratchet-env:${key}`;
/** `bench-images/<key>.json`: written LAST by the image job, so its presence means the archive is complete. */
export const imageManifestKey = (key: string): string => `${IMAGE_PREFIX}${key}.json`;

/** The part of `environment:` the bench reads. */
export interface EnvironmentSettings {
  /** Repo-relative path of a Dockerfile or a devcontainer.json. */
  image?: string;
  machine?: string;
  region?: string;
  network?: string[];
}

/**
 * Defensive read of `config.environment` (the schema key is added elsewhere): anything that is not the documented shape is
 * ignored, except an `image` that is present but not a safe repo-relative path, which is a loud configuration error because
 * silently falling back to the bare host would benchmark the wrong toolchain.
 */
export function readEnvironment(config: unknown): EnvironmentSettings {
  // TODO(schema): use the typed `config.environment` once packages/core declares it.
  const env = (config as { environment?: unknown } | null | undefined)?.environment;
  if (!env || typeof env !== 'object' || Array.isArray(env)) return {};
  const e = env as Record<string, unknown>;
  const out: EnvironmentSettings = {};
  if (e.image !== undefined && e.image !== null) {
    if (typeof e.image !== 'string' || e.image.trim() === '') throw new BenchError('environment.image must be a path (a Dockerfile or a devcontainer.json) inside the repository');
    out.image = normalizeRepoRelative(e.image, 'environment.image');
  }
  if (typeof e.machine === 'string' && e.machine !== '') out.machine = e.machine;
  if (typeof e.region === 'string' && e.region !== '') out.region = e.region;
  if (Array.isArray(e.network)) out.network = e.network.filter((x): x is string => typeof x === 'string');
  return out;
}

/** `a/./b//c` -> `a/b/c`; rejects absolute paths, `..` and NUL. The result never starts with `./`. */
export function normalizeRepoRelative(p: string, what: string): string {
  if (p.includes('\0')) throw new BenchError(`${what} contains a NUL byte`);
  const slashed = p.replace(/\\/g, '/');
  if (slashed.startsWith('/')) throw new BenchError(`${what} must be a path inside the repository, not an absolute path (${JSON.stringify(p)})`);
  const n = posix.normalize(slashed).replace(/^(\.\/)+/, '').replace(/\/+$/, '');
  if (n === '..' || n.startsWith('../')) throw new BenchError(`${what} must stay inside the repository (${JSON.stringify(p)})`);
  return n === '' ? '.' : n;
}

// ---- JSONC and Dockerfile parsing ------------------------------------------------------------------------------------

/** Parses JSON with comments and trailing commas (devcontainer.json is JSONC). */
export function parseJsonc(text: string): unknown {
  let noComments = '';
  let inStr = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (inStr) {
      noComments += c;
      if (c === '\\') noComments += text[++i] ?? '';
      else if (c === '"') inStr = false;
      continue;
    }
    if (c === '"') {
      inStr = true;
      noComments += c;
    } else if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++;
      noComments += '\n';
    } else if (c === '/' && text[i + 1] === '*') {
      const end = text.indexOf('*/', i + 2);
      i = end < 0 ? text.length : end + 1;
    } else {
      noComments += c;
    }
  }
  let out = '';
  inStr = false;
  for (let i = 0; i < noComments.length; i++) {
    const c = noComments[i]!;
    if (inStr) {
      out += c;
      if (c === '\\') out += noComments[++i] ?? '';
      else if (c === '"') inStr = false;
    } else if (c === '"') {
      inStr = true;
      out += c;
    } else if (c === ',') {
      const rest = noComments.slice(i + 1).trimStart();
      if (!(rest.startsWith('}') || rest.startsWith(']'))) out += c;
    } else {
      out += c;
    }
  }
  return JSON.parse(out);
}

export interface DockerfileInfo {
  /** Base images of every FROM that names a registry image (not `scratch`, not an earlier stage), in order. */
  baseImages: string[];
  /** Sources of COPY / ADD instructions that read the BUILD CONTEXT (not `--from=`, not URLs, not heredocs). */
  copySources: string[];
}

/** Splits an instruction into words like a shell would for COPY (quotes grouped, no expansion). */
function words(s: string): string[] {
  const out: string[] = [];
  let cur = '';
  let q: string | null = null;
  let any = false;
  for (const c of s) {
    if (q) {
      if (c === q) q = null;
      else cur += c;
    } else if (c === '"' || c === "'") {
      q = c;
      any = true;
    } else if (/\s/.test(c)) {
      if (cur !== '' || any) out.push(cur);
      cur = '';
      any = false;
    } else cur += c;
  }
  if (cur !== '' || any) out.push(cur);
  return out;
}

/** The two things the bench needs from a Dockerfile: which images it builds on, and which context files it copies in. */
export function parseDockerfile(text: string): DockerfileInfo {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const instructions: string[] = [];
  let cur = '';
  for (const raw of lines) {
    const line = raw.trimEnd();
    if (cur === '' && (line.trimStart().startsWith('#') || line.trim() === '')) continue;
    if (cur !== '' && line.trimStart().startsWith('#')) continue; // a comment line inside a continued instruction
    if (line.endsWith('\\')) {
      cur += line.slice(0, -1) + ' ';
    } else {
      instructions.push((cur + line).trim());
      cur = '';
    }
  }
  if (cur.trim() !== '') instructions.push(cur.trim());

  const stages = new Set<string>();
  const baseImages: string[] = [];
  const copySources: string[] = [];
  for (const ins of instructions) {
    const m = /^(\S+)\s*(.*)$/s.exec(ins);
    if (!m) continue;
    const op = m[1]!.toUpperCase();
    const rest = m[2] ?? '';
    if (op === 'FROM') {
      const w = words(rest).filter((x) => !x.startsWith('--'));
      const image = w[0];
      if (image && image.toLowerCase() !== 'scratch' && !stages.has(image.toLowerCase())) baseImages.push(image);
      const as = w.findIndex((x) => x.toUpperCase() === 'AS');
      if (as >= 0 && w[as + 1]) stages.add(w[as + 1]!.toLowerCase());
    } else if (op === 'COPY' || op === 'ADD') {
      const flags = words(rest).filter((x) => x.startsWith('--'));
      if (flags.some((f) => f.toLowerCase().startsWith('--from='))) continue;
      let args: string[];
      if (rest.trimStart().startsWith('[')) {
        try {
          const parsed = JSON.parse(rest.trimStart().replace(/^(--\S+\s+)+/, '')) as unknown;
          args = Array.isArray(parsed) ? parsed.map(String) : [];
        } catch {
          args = words(rest).filter((x) => !x.startsWith('--'));
        }
      } else {
        args = words(rest).filter((x) => !x.startsWith('--'));
      }
      if (args.some((a) => a.startsWith('<<'))) continue; // heredoc: the content is in the Dockerfile itself
      for (const src of args.slice(0, -1)) {
        if (/^[a-z][a-z0-9+.-]*:\/\//i.test(src) || /^git@/.test(src)) continue; // ADD <url>: fetched at build time
        copySources.push(src);
      }
    }
  }
  return { baseImages, copySources };
}

/** Image references pinned by digest (`name@sha256:...`). */
export const pinnedDigest = (ref: string): string | undefined => /@(sha256:[0-9a-f]{64})$/.exec(ref)?.[1];

// ---- resolving `environment.image` against a tree ------------------------------------------------------------------------

/** Cap on the number of files that make up the "environment" of one repo. A Dockerfile that copies the whole repo in is refused long before this. */
export const MAX_ENV_FILES = 2000;
const BUILD_ARG_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const BUILD_ARG_VALUE = /^[A-Za-z0-9._\/:@+=,~-]{0,200}$/;
const IMAGE_REF = /^[A-Za-z0-9][A-Za-z0-9._\/:@-]{0,255}$/;
const TARGET_RE = /^[A-Za-z0-9._-]{1,64}$/;

export interface ImageSpec {
  kind: 'dockerfile' | 'image';
  /** `environment.image` as the repo wrote it (normalized). */
  path: string;
  /** Root-relative Dockerfile (kind `dockerfile`). */
  dockerfile?: string;
  /** Root-relative build context directory (kind `dockerfile`). */
  context?: string;
  target?: string;
  args: Record<string, string>;
  /** A prebuilt registry image to pull instead of building (devcontainer `image`, kind `image`). */
  image?: string;
  /** Registry images the build starts from, with the digest-pinned ones identified. */
  baseImages: string[];
  baseDigests: string[];
  /** Plain-language remarks for the log: unpinned base images, ignored devcontainer properties. */
  notes: string[];
}

export interface ResolvedImage {
  spec: ImageSpec;
  /**
   * sha256 per root-relative file that can influence the built image: the Dockerfile, the devcontainer.json, the context's
   * .dockerignore and every context file the Dockerfile COPYs / ADDs. A candidate whose map differs is blocked.
   */
  files: Record<string, string>;
}

async function readRegular(tree: string, rel: string, what: string): Promise<string> {
  const p = join(tree, rel);
  const st = await lstat(p).catch(() => undefined);
  if (!st) throw new BenchError(`${what} ${rel} does not exist in the repository`);
  if (!st.isFile()) throw new BenchError(`${what} ${rel} must be a regular file (not a symlink or directory)`);
  return readFile(p, 'utf8');
}

/** All regular files and symlinks under `rel` (a file, or a directory walked without following links), `.git` excluded. */
async function filesUnder(tree: string, rel: string, budget: { left: number }): Promise<string[]> {
  const st = await lstat(join(tree, rel)).catch(() => undefined);
  if (!st) return [];
  if (!st.isDirectory()) return [rel];
  const out: string[] = [];
  const walk = async (dir: string): Promise<void> => {
    for (const e of await readdir(join(tree, dir), { withFileTypes: true })) {
      if (e.name === '.git' && dir === '.') continue;
      const r = dir === '.' ? e.name : `${dir}/${e.name}`;
      if (e.isDirectory()) await walk(r);
      else {
        if (--budget.left < 0) throw new BenchError(`the Dockerfile copies more than ${MAX_ENV_FILES} files from the build context; keep the environment small (a toolchain-only context such as .devcontainer/)`);
        out.push(r);
      }
    }
  };
  await walk(rel);
  return out;
}

function buildStringMap(raw: unknown, what: string): Record<string, string> {
  if (raw === undefined) return {};
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new BenchError(`${what} must be an object of strings`);
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (!BUILD_ARG_NAME.test(k) || typeof v !== 'string' || !BUILD_ARG_VALUE.test(v)) {
      throw new BenchError(`${what}.${k} is not a plain build argument (names [A-Za-z_][A-Za-z0-9_]*, values of letters, digits and . _ / : @ + = , ~ -)`);
    }
    out[k] = v;
  }
  return out;
}

/**
 * Reads `environment.image` from a checked-out tree (the base ref on the controller, or the candidate's tree to compare) and
 * returns what to build and the hashes of everything that influences the result. Throws BenchError for configurations the
 * bench cannot honor.
 */
export async function resolveImageSource(tree: string, imagePath: string): Promise<ResolvedImage> {
  const path = normalizeRepoRelative(imagePath, 'environment.image');
  const base = posix.basename(path);
  const isDevcontainer = /\.json$/i.test(base);
  const notes: string[] = [];
  const args: Record<string, string> = {};
  let dockerfile: string;
  let context: string;
  let target: string | undefined;
  const hashNames: string[] = [path];

  if (isDevcontainer) {
    const text = await readRegular(tree, path, 'environment.image');
    let dc: Record<string, unknown>;
    try {
      const parsed = parseJsonc(text);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('not an object');
      dc = parsed as Record<string, unknown>;
    } catch (err) {
      throw new BenchError(`${path} is not valid JSON (comments and trailing commas are allowed): ${(err as Error).message}`);
    }
    if (dc.dockerComposeFile !== undefined) throw new BenchError(`${path}: dockerComposeFile is not supported; point environment.image at a Dockerfile or a devcontainer with "build" or "image"`);
    const dir = posix.dirname(path);
    const join_ = (p: string, what: string) => normalizeRepoRelative(posix.join(dir === '.' ? '' : dir, p), what);
    const build = dc.build && typeof dc.build === 'object' && !Array.isArray(dc.build) ? (dc.build as Record<string, unknown>) : {};
    const df = typeof build.dockerfile === 'string' ? build.dockerfile : typeof dc.dockerFile === 'string' ? dc.dockerFile : undefined;
    if (df === undefined) {
      const image = dc.image;
      if (typeof image !== 'string' || !IMAGE_REF.test(image)) {
        throw new BenchError(`${path} needs "build.dockerfile" (a Dockerfile in the repository) or "image" (a registry image) to describe the toolchain`);
      }
      const digest = pinnedDigest(image);
      if (!digest) notes.push(`image ${image} is not pinned by digest: the saved image is that tag as pulled when it was built (rebuilt only when ${path} changes)`);
      ignoredNote(dc, notes);
      return {
        spec: { kind: 'image', path, args: {}, image, baseImages: [image], baseDigests: digest ? [digest] : [], notes },
        files: await hashNamedFiles(tree, hashNames),
      };
    }
    dockerfile = join_(df, 'build.dockerfile');
    context = join_(typeof build.context === 'string' ? build.context : '.', 'build.context');
    Object.assign(args, buildStringMap(build.args, 'build.args'));
    if (build.target !== undefined) {
      if (typeof build.target !== 'string' || !TARGET_RE.test(build.target)) throw new BenchError('build.target must be a plain stage name');
      target = build.target;
    }
    ignoredNote(dc, notes);
  } else {
    dockerfile = path;
    // Same as `docker build -f <path> <dir of the Dockerfile>`; use a devcontainer.json to choose another context.
    context = posix.dirname(path);
  }

  const df = await readRegular(tree, dockerfile, 'the Dockerfile');
  if (dockerfile !== path) hashNames.push(dockerfile);
  const info = parseDockerfile(df);
  if (info.baseImages.length === 0 && !/^\s*FROM\s+scratch/im.test(df)) throw new BenchError(`${dockerfile} has no FROM instruction`);
  for (const ref of info.baseImages) {
    // `FROM ${BASE}` and friends are resolved by the build; they cannot be pinned or checked here.
    if (!ref.includes('$') && !IMAGE_REF.test(ref)) throw new BenchError(`${dockerfile}: base image ${JSON.stringify(ref)} is not a plain image reference`);
  }
  const baseDigests = info.baseImages.map(pinnedDigest).filter((d): d is string => d !== undefined);
  const unpinned = info.baseImages.filter((r) => !pinnedDigest(r));
  if (unpinned.length > 0) notes.push(`base image ${unpinned.join(', ')} is not pinned by digest: rebuilds are keyed on the Dockerfile only, so the saved image can lag behind upstream (pin with @sha256:... for reproducibility)`);

  // .dockerignore changes what COPY sees.
  for (const ig of [`${dockerfile}.dockerignore`, context === '.' ? '.dockerignore' : `${context}/.dockerignore`]) hashNames.push(ig);

  // Every context file the Dockerfile copies in. `COPY . /app` would put the candidate's own source into the image: refused.
  const budget = { left: MAX_ENV_FILES };
  const wholeContext = info.copySources.some((s) => s.includes('$'));
  const sources = wholeContext ? ['.'] : info.copySources;
  if (wholeContext) notes.push('a COPY source uses a variable, so every file of the build context is treated as part of the environment');
  const ctxPrefix = context === '.' ? '' : `${context}/`;
  let listing: string[] | undefined;
  for (const src of sources) {
    const rel = normalizeRepoRelative(src.replace(/^\/+/, ''), `COPY source ${JSON.stringify(src)}`);
    const full = normalizeRepoRelative(`${ctxPrefix}${rel === '.' ? '' : rel}` || '.', 'COPY source');
    if (!/[*?[{]/.test(rel)) {
      // A literal path: a file, or a directory (all of it). A missing source fails the build, which reports it.
      hashNames.push(...(await filesUnder(tree, full, budget)));
    } else {
      listing ??= await filesUnder(tree, context, { left: 200_000 });
      const re = globToRegExp(rel);
      let hits = 0;
      for (const f of listing) {
        const parts = (ctxPrefix ? f.slice(ctxPrefix.length) : f).split('/');
        for (let n = 1; n <= parts.length; n++) {
          if (re.test(parts.slice(0, n).join('/'))) {
            hashNames.push(f);
            hits++;
            break;
          }
        }
      }
      if (hits > MAX_ENV_FILES) throw new BenchError(`COPY ${src} matches more than ${MAX_ENV_FILES} files; keep the environment small`);
    }
  }
  const names = [...new Set(hashNames)].sort();
  if (names.some((n) => n === 'ratchet.yaml')) {
    throw new BenchError(`${dockerfile} copies ratchet.yaml (or the whole repository) into the image; the environment must not contain the code under optimization. Keep the build context to toolchain files (for example .devcontainer/), or COPY only a manifest such as a lockfile`);
  }
  if (names.length > MAX_ENV_FILES) throw new BenchError(`the environment would consist of more than ${MAX_ENV_FILES} files; keep the build context small`);
  const files = await hashNamedFiles(tree, names);
  if (!(dockerfile in files)) throw new BenchError(`${dockerfile} is not a readable file`);
  return { spec: { kind: 'dockerfile', path, dockerfile, context, ...(target ? { target } : {}), args, baseImages: info.baseImages, baseDigests, notes }, files };
}

/** devcontainer.json properties the bench does not honor (the image is built, nothing else runs in it). */
function ignoredNote(dc: Record<string, unknown>, notes: string[]): void {
  const keys = ['features', 'postCreateCommand', 'onCreateCommand', 'updateContentCommand', 'postStartCommand', 'postAttachCommand', 'initializeCommand', 'containerEnv', 'remoteEnv', 'mounts', 'runArgs', 'privileged', 'capAdd', 'securityOpt', 'forwardPorts', 'remoteUser', 'containerUser', 'workspaceMount', 'workspaceFolder'];
  const present = keys.filter((k) => dc[k] !== undefined);
  if (present.length > 0) notes.push(`devcontainer properties ignored by the bench (it builds the image only, runs offline and unprivileged): ${present.join(', ')}`);
}

// ---- keys, manifests ------------------------------------------------------------------------------------------------------

const sha256 = (s: string): string => createHash('sha256').update(s).digest('hex');
const sortObj = <T>(o: Record<string, T>): Record<string, T> => Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : 1)));

export interface ImageKeyInput {
  spec: ImageSpec;
  files: Readonly<Record<string, string>>;
  /** `archFromInstanceType` of the bench hardware. */
  arch: string;
}

/**
 * One image per key: what is built (Dockerfile / devcontainer fields), the hashes of everything it copies in, the pinned base
 * digests and the architecture. Reused by every candidate and every optimization that shares them.
 */
export function imageKey(i: ImageKeyInput): string {
  const s = i.spec;
  const canon = JSON.stringify({
    v: 1,
    rev: IMAGE_REV,
    kind: s.kind,
    dockerfile: s.dockerfile ?? null,
    context: s.context ?? null,
    target: s.target ?? null,
    args: sortObj(s.args),
    image: s.image ?? null,
    files: sortObj({ ...i.files }),
    baseDigests: [...s.baseDigests].sort(),
    arch: i.arch,
  });
  return `img-${sha256(canon).slice(0, 40)}`;
}

/** What the image job records about its archive. `archive` is content-addressed, like the dependency snapshots'. */
export interface ImageManifest {
  v: 1;
  key: string;
  rev: number;
  kind: ImageSpec['kind'];
  path: string;
  arch: string;
  files: Record<string, string>;
  baseDigests: string[];
  tag: string;
  /** `docker image inspect --format '{{.Id}}'` on the builder; the bench compares it after `docker load`. */
  imageId: string;
  compression: 'zstd' | 'gzip';
  archive: string;
  sha256: string;
  bytes: number;
  createdAt: number;
}

export type ImageManifestCheck = { ok: true; manifest: ImageManifest } | { ok: false; error: string };

/** Validates a manifest read from the artifacts bucket against what the controller computed itself from the base ref. */
export function parseImageManifest(text: string, expect: ImageKeyInput & { key: string }): ImageManifestCheck {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: 'manifest is not JSON' };
  }
  if (!raw || typeof raw !== 'object') return { ok: false, error: 'manifest is not an object' };
  const m = raw as Record<string, unknown>;
  const bad = (what: string): ImageManifestCheck => ({ ok: false, error: `manifest ${what}` });
  if (m.v !== 1) return bad('has an unknown version');
  if (m.key !== expect.key) return bad('is for another key');
  if (m.rev !== IMAGE_REV) return bad('was built by another image-job revision');
  if (m.arch !== expect.arch) return bad('was built for another architecture');
  if (JSON.stringify(sortObj((m.files ?? {}) as Record<string, string>)) !== JSON.stringify(sortObj({ ...expect.files }))) return bad('has different environment file hashes');
  if (m.tag !== imageTag(expect.key)) return bad('has an unexpected tag');
  if (typeof m.imageId !== 'string' || !/^sha256:[0-9a-f]{64}$/.test(m.imageId)) return bad('has no valid image id');
  if (m.compression !== 'zstd' && m.compression !== 'gzip') return bad('has an unknown compression');
  if (typeof m.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(m.sha256)) return bad('has no valid sha256');
  const ext = m.compression === 'zstd' ? 'tar\\.zst' : 'tar\\.gz';
  if (typeof m.archive !== 'string' || !new RegExp(`^${IMAGE_PREFIX}${expect.key}/${m.sha256}\\.${ext}$`).test(m.archive)) return bad('points outside its own prefix');
  if (typeof m.bytes !== 'number' || !Number.isFinite(m.bytes) || m.bytes < 0) return bad('has no valid size');
  return {
    ok: true,
    manifest: {
      v: 1,
      key: m.key,
      rev: IMAGE_REV,
      kind: expect.spec.kind,
      path: expect.spec.path,
      arch: expect.arch,
      files: { ...expect.files },
      baseDigests: [...expect.spec.baseDigests],
      tag: m.tag,
      imageId: m.imageId,
      compression: m.compression,
      archive: m.archive,
      sha256: m.sha256,
      bytes: m.bytes,
      createdAt: typeof m.createdAt === 'number' ? m.createdAt : 0,
    },
  };
}

/** What a bench job needs to load the image and what it compares the candidate against. */
export interface ImagePlan {
  key: string;
  tag: string;
  spec: ImageSpec;
  /** Hashes of the environment files at the BASE commit. */
  files: Record<string, string>;
  /** Root-relative paths that are pinned to bench.ref on the bench (the environment files). */
  pinned: string[];
  baseCommit: string;
  manifest: ImageManifest;
  /** True when the image already existed (no instance was launched for it). */
  reused: boolean;
}

// ---- the candidate may not change the environment ------------------------------------------------------------------------

/**
 * Hashes the environment of a candidate tree the way the base was hashed. A tree whose `environment.image` can no longer be
 * resolved (Dockerfile deleted or turned into a symlink) yields a marker entry, so it never equals the base.
 */
export async function environmentFilesOf(tree: string, imagePath: string): Promise<Record<string, string>> {
  try {
    return (await resolveImageSource(tree, imagePath)).files;
  } catch (err) {
    return { '!environment': `unresolvable:${sha256((err as Error).message)}` };
  }
}

/** Same comparison as the lockfiles: files whose hash differs, or that exist on one side only. Sorted. */
export function environmentChanges(base: Readonly<Record<string, string>>, candidate: Readonly<Record<string, string>>): string[] {
  const names = new Set([...Object.keys(base), ...Object.keys(candidate)]);
  return [...names].filter((n) => base[n] !== candidate[n]).map((n) => (n === '!environment' ? 'the environment (Dockerfile / devcontainer) no longer resolves' : n)).sort();
}

/** The reason a candidate is blocked when it changes the environment. Same text for every runner. */
export function environmentChangeGuardrail(changed: readonly string[], benchRef: string): GuardrailResult {
  return {
    name: 'environment unchanged',
    passed: false,
    detail:
      `environment changes are not allowed in v0: the container image is built once from ${benchRef} and the bench has no network (${changed.slice(0, 10).join(', ')}${changed.length > 10 ? ', ...' : ''} differs from ${benchRef}). ` +
      'Revert the Dockerfile / devcontainer and the files it copies; the toolchain is that of the base ref.',
  };
}

/** Hint appended to a failed setup when it ran in the repo's container. */
export function containerSetupHint(benchRef: string): string {
  return `setup ran inside the container built from ${benchRef} with no network and a read-only root filesystem; tools missing from that image, writes outside the checkout, home and /tmp, and downloads fail here, and environment changes are not allowed in v0`;
}
