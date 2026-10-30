import { createHash } from 'node:crypto';
import { lstat, readFile, readlink, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import type { DepsMode, GuardrailResult } from '@ratchet/core';
import { BenchError } from './errors.js';

/**
 * Dependencies for a bench that has no network.
 *
 * Candidate setup (`pnpm install --frozen-lockfile && pnpm build`) cannot download anything on a bench instance, so the
 * packages arrive as a SNAPSHOT: the package manager's own cache/store, built once per lockfile by a short-lived job on a
 * machine that does have egress (remote.ts / remote-deps.ts), stored in the artifacts bucket and restored read-only before
 * setup. Setup then installs from it with the network switched off (`npm_config_offline`).
 *
 * The snapshot is built from the lockfile at `bench.ref` only, never from the candidate. A candidate whose lockfile differs is
 * BLOCKED (see `lockfileChanges` / `dependencyChangeGuardrail`): dependency changes are not allowed in v0.
 *
 * A `DepsStrategy` is the small, package-manager-specific part. Everything else (keys, manifests, the job, the restore) is shared.
 */

/** Prefix in the artifacts bucket for snapshots. Only the deps identity writes here; the bench identities read. */
export const DEPS_PREFIX = 'bench-deps/';
/**
 * Name of the staged base-tree archive the snapshot job downloads (`bench-inputs/<opt>/<job>/deps-base.tgz`). The deps
 * identity may read exactly this basename under `bench-inputs/`, never the candidate's `code.tgz`.
 */
export const DEPS_BASE_ARCHIVE = 'deps-base.tgz';
/** Unprivileged OS user of the snapshot job (created and IMDS-blocked by the deps user-data; packages/aws `DEPS_OS_USER`). */
export const DEPS_USER = 'ratchet-deps';

export type DepsStrategyId = 'pnpm' | 'npm' | 'none';

export interface DepsStrategy {
  readonly id: DepsStrategyId;
  /** Bumped when fetch/prune/restore change in a way that makes old snapshots wrong: it is part of the snapshot key. */
  readonly rev: number;
  /**
   * Root-relative lockfile names. Whichever exist in a tree are hashed into the key; all of them are pinned to bench.ref's
   * version on the bench (they count as protected paths whether or not the user listed them) and a candidate that changes
   * one is blocked.
   */
  readonly lockfiles: readonly string[];
  /**
   * Shell, run by the snapshot job as the unprivileged user in the base tree. It must fill the store/cache named by
   * `fetchEnv` WITHOUT running lifecycle scripts of the packages (`pnpm fetch` only populates the store; `npm ci --ignore-scripts`).
   */
  readonly fetch: string;
  /** Shell that prints the package manager's version (recorded in the manifest and compared on restore). */
  readonly version: string;
  /** Environment of the fetch (store location, no update checks, no package-manager self-update). */
  fetchEnv(store: string): Record<string, string>;
  /** Root shell run on the store before it is packed (`$STORE`): drop per-project registrations and logs. */
  readonly prune: string;
  /** Root shell run on the restored store (`$STORE`, `$SCRATCH`): wire the few paths the package manager insists on writing. */
  readonly restore: string;
  /** Environment of setup and command guardrails on the bench: the restored store and offline semantics. */
  setupEnv(store: string, scratch: string): Record<string, string>;
}

const QUIET = { npm_config_update_notifier: 'false', npm_config_fund: 'false', npm_config_audit: 'false' } as const;

export const PNPM: DepsStrategy = {
  id: 'pnpm',
  rev: 1,
  lockfiles: ['pnpm-lock.yaml'],
  // `pnpm fetch` is lockfile-only (the manifests are ignored) and, unlike `pnpm install`, populates just the content-addressable
  // store: no node_modules, no lifecycle scripts. It has no --frozen-lockfile flag because it never resolves anything.
  fetch: 'pnpm fetch',
  version: 'pnpm --version',
  fetchEnv: (store) => ({
    npm_config_store_dir: store,
    PNPM_STORE_DIR: store,
    npm_config_ignore_scripts: 'true',
    // Never download another pnpm because `packageManager` names a different version: the bench has no network and the
    // image's pinned pnpm is the one that was verified.
    npm_config_manage_package_manager_versions: 'false',
    ...QUIET,
  }),
  // pnpm registers every project that uses a store under `<store>/vN/projects/`; the job's tree must not leak into the snapshot.
  prune: 'rm -rf "$STORE"/v*/projects "$STORE"/v*/tmp',
  // `pnpm install` insists on creating `<store>/vN/projects/<hash>`. The store itself stays read-only: `projects` becomes a
  // symlink to a directory the candidate user owns (verified: a fully read-only store fails with EACCES, this works).
  restore: 'for v in "$STORE"/v*; do [ -d "$v" ] || continue; rm -rf "$v/projects"; ln -s "$SCRATCH/pnpm-projects" "$v/projects"; done; mkdir -p "$SCRATCH/pnpm-projects"',
  setupEnv: (store) => ({
    npm_config_store_dir: store,
    PNPM_STORE_DIR: store,
    npm_config_offline: 'true',
    npm_config_manage_package_manager_versions: 'false',
    // Never hardlink out of the shared store (a hardlink would make a node_modules file and a store file one inode).
    npm_config_package_import_method: 'clone-or-copy',
    ...QUIET,
  }),
};

export const NPM: DepsStrategy = {
  id: 'npm',
  rev: 1,
  lockfiles: ['package-lock.json', 'npm-shrinkwrap.json'],
  // Installs into a throwaway node_modules (discarded with the tree); what is kept is the download cache.
  fetch: 'npm ci --ignore-scripts --no-audit --no-fund',
  version: 'npm --version',
  fetchEnv: (store) => ({ npm_config_cache: store, npm_config_ignore_scripts: 'true', ...QUIET }),
  prune: 'rm -rf "$STORE/_logs" "$STORE/_update-notifier-last-checked"',
  restore: 'mkdir -p "$SCRATCH/npm-logs"',
  setupEnv: (store, scratch) => ({
    npm_config_cache: store,
    npm_config_offline: 'true',
    npm_config_logs_dir: `${scratch}/npm-logs`,
    ...QUIET,
  }),
};

/** No lockfile, or the evaluator declared `deps: none`: nothing to snapshot, nothing pinned, nothing blocked. */
export const NO_DEPS: DepsStrategy = {
  id: 'none',
  rev: 1,
  lockfiles: [],
  fetch: 'true',
  version: 'true',
  fetchEnv: () => ({}),
  prune: 'true',
  restore: 'true',
  setupEnv: () => ({}),
};

export const DEPS_STRATEGIES: Readonly<Record<DepsStrategyId, DepsStrategy>> = { pnpm: PNPM, npm: NPM, none: NO_DEPS };

export interface ResolvedDeps {
  strategy: DepsStrategy;
  /** Human-readable note when the choice is worth a log line (e.g. an unsupported lockfile). */
  note?: string;
}

/**
 * Picks the strategy for `evaluator.deps` given the root-level file names of the BASE tree (`bench.ref`):
 * `auto` detects by lockfile (pnpm first), `pnpm` / `npm` demand their lockfile, `none` is none.
 */
export function resolveDepsStrategy(mode: DepsMode, rootFiles: Iterable<string>): ResolvedDeps {
  const files = new Set(rootFiles);
  const has = (s: DepsStrategy) => s.lockfiles.some((f) => files.has(f));
  switch (mode) {
    case 'none':
      return { strategy: NO_DEPS };
    case 'pnpm':
    case 'npm': {
      const strategy = DEPS_STRATEGIES[mode];
      if (!has(strategy)) {
        throw new BenchError(`evaluator.deps is "${mode}" but bench.ref has no ${strategy.lockfiles.join(' or ')} at its root; add the lockfile or set evaluator.deps to auto or none`);
      }
      return { strategy };
    }
    case 'auto': {
      if (has(PNPM)) return { strategy: PNPM };
      if (has(NPM)) return { strategy: NPM };
      return files.has('yarn.lock')
        ? { strategy: NO_DEPS, note: 'yarn.lock found: dependency snapshots support pnpm and npm only, so nothing is installed for the bench' }
        : { strategy: NO_DEPS };
    }
  }
}

const sha256 = (b: Buffer | string): string => createHash('sha256').update(b).digest('hex');

/**
 * sha256 of each existing file in `names` (root-relative) under `dir`. A symlink hashes its target text under a different
 * prefix, so a lockfile swapped for a link never equals a real one. Missing files are absent from the result.
 */
export async function hashNamedFiles(dir: string, names: readonly string[]): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (const name of names) {
    const p = join(dir, name);
    const st = await lstat(p).catch(() => undefined);
    if (!st) continue;
    out[name] = st.isSymbolicLink() ? `link:${sha256(await readlink(p))}` : st.isFile() ? sha256(await readFile(p)) : `other:${st.mode.toString(8)}`;
  }
  return out;
}

/** Names of the entries directly under `dir` (used to detect the strategy from the base tree's root). */
export async function rootEntries(dir: string): Promise<string[]> {
  return readdir(dir);
}

/** Lockfiles whose hash differs between the base tree and the candidate (a lockfile present on only one side counts). Sorted. */
export function lockfileChanges(base: Readonly<Record<string, string>>, candidate: Readonly<Record<string, string>>): string[] {
  const names = new Set([...Object.keys(base), ...Object.keys(candidate)]);
  return [...names].filter((n) => base[n] !== candidate[n]).sort();
}

/** The reason a candidate is blocked when its lockfile differs. Same text for the local and the remote runner. */
export function dependencyChangeGuardrail(changed: readonly string[], benchRef: string): GuardrailResult {
  return {
    name: 'dependencies unchanged',
    passed: false,
    detail:
      `dependency changes are not allowed in v0: the bench has no network (${changed.join(', ')} differs from ${benchRef}). ` +
      'Revert the lockfile (and the dependency changes in package.json); the dependencies are those of the base ref.',
  };
}

/** Hint appended to a failed setup when the bench installs from a snapshot (the usual cause: package.json asks for something the lockfile does not have). */
export function offlineSetupHint(strategy: DepsStrategy, benchRef: string): string {
  return (
    `the bench installs from a snapshot of ${strategy.lockfiles.join(' / ')} at ${benchRef} with no network; ` +
    'a setup that needs anything else (changed package.json dependencies, another registry, git or url dependencies) fails here, and dependency changes are not allowed in v0'
  );
}

/** Graviton instance families (`c7g`, `m6gd`, `t4g`, `im4gn`, `is4gen`, `hpc7g`, `a1`) are arm64; everything else is treated as x86_64. */
export function archFromInstanceType(instanceType: string): 'arm64' | 'x86_64' {
  const family = instanceType.split('.')[0] ?? '';
  return /^(a1|[a-z]+\d+[a-z]*g[a-z]*)$/.test(family) ? 'arm64' : 'x86_64';
}

export interface DepsKeyInput {
  strategy: DepsStrategy;
  /** Hashes of the BASE tree's lockfiles. */
  lockfiles: Readonly<Record<string, string>>;
  /** The evaluator image (AMI id): it pins node, the package manager and the OS. */
  image: string;
  /** `archFromInstanceType` of the bench hardware: native optional dependencies differ per architecture. */
  arch: string;
}

/**
 * One snapshot per key: strategy + its revision, the lockfile hashes at the base ref, the image and the architecture.
 * Reused by every candidate and every optimization that shares them.
 */
export function depsKey(i: DepsKeyInput): string {
  const lockfiles = Object.fromEntries(Object.entries(i.lockfiles).sort(([a], [b]) => (a < b ? -1 : 1)));
  const canon = JSON.stringify({ v: 1, strategy: i.strategy.id, rev: i.strategy.rev, lockfiles, image: i.image, arch: i.arch });
  return `${i.strategy.id}-${sha256(canon).slice(0, 40)}`;
}

export const DEPS_KEY_RE = /^(pnpm|npm)-[0-9a-f]{40}$/;

/** `bench-deps/<key>.json`: written LAST by the snapshot job, so its presence means the archive is complete. */
export const manifestKey = (key: string): string => `${DEPS_PREFIX}${key}.json`;

/**
 * What the snapshot job records about its archive. `archive` is content-addressed (`bench-deps/<key>/<sha256>.<ext>`), so two
 * racing builders can never leave a manifest pointing at someone else's bytes.
 */
export interface DepsManifest {
  v: 1;
  key: string;
  strategy: DepsStrategyId;
  rev: number;
  lockfiles: Record<string, string>;
  image: string;
  arch: string;
  /** `node --version` and the package manager's version on the builder; the bench refuses to restore on a mismatch. */
  node: string;
  pm: string;
  compression: 'zstd' | 'gzip';
  archive: string;
  sha256: string;
  bytes: number;
  createdAt: number;
}

export type ManifestCheck = { ok: true; manifest: DepsManifest } | { ok: false; error: string };

const VERSION_RE = /^[A-Za-z0-9._+-]{1,64}$/;

/**
 * Parses and validates a manifest read from the artifacts bucket against what the controller computed itself from the base ref.
 * Anything that does not match is rejected (the caller rebuilds): the controller trusts the lockfile hashes it computed, not the
 * ones written to the bucket.
 */
export function parseDepsManifest(text: string, expect: DepsKeyInput & { key: string }): ManifestCheck {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: 'manifest is not JSON' };
  }
  if (!raw || typeof raw !== 'object') return { ok: false, error: 'manifest is not an object' };
  const m = raw as Record<string, unknown>;
  const bad = (what: string): ManifestCheck => ({ ok: false, error: `manifest ${what}` });
  if (m.v !== 1) return bad('has an unknown version');
  if (m.key !== expect.key) return bad('is for another key');
  if (m.strategy !== expect.strategy.id || m.rev !== expect.strategy.rev) return bad('was built by another strategy revision');
  if (m.image !== expect.image || m.arch !== expect.arch) return bad('was built for another image or architecture');
  if (JSON.stringify(sortKeys(m.lockfiles)) !== JSON.stringify(sortKeys(expect.lockfiles))) return bad('has different lockfile hashes');
  if (typeof m.node !== 'string' || !VERSION_RE.test(m.node) || typeof m.pm !== 'string' || !VERSION_RE.test(m.pm)) return bad('has no valid tool versions');
  if (m.compression !== 'zstd' && m.compression !== 'gzip') return bad('has an unknown compression');
  if (typeof m.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(m.sha256)) return bad('has no valid sha256');
  const ext = m.compression === 'zstd' ? 'tar\\.zst' : 'tgz';
  if (typeof m.archive !== 'string' || !new RegExp(`^${DEPS_PREFIX}${expect.key}/${m.sha256}\\.${ext}$`).test(m.archive)) return bad('points outside its own prefix');
  if (typeof m.bytes !== 'number' || !Number.isFinite(m.bytes) || m.bytes < 0) return bad('has no valid size');
  return {
    ok: true,
    manifest: {
      v: 1,
      key: m.key,
      strategy: expect.strategy.id,
      rev: expect.strategy.rev,
      lockfiles: { ...expect.lockfiles },
      image: expect.image,
      arch: expect.arch,
      node: m.node,
      pm: m.pm,
      compression: m.compression,
      archive: m.archive,
      sha256: m.sha256,
      bytes: m.bytes,
      createdAt: typeof m.createdAt === 'number' ? m.createdAt : 0,
    },
  };
}

function sortKeys(v: unknown): unknown {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return v;
  return Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : 1)));
}

/** What a bench job needs to restore a snapshot and what it compares the candidate against. */
export interface DepsPlan {
  strategy: DepsStrategy;
  key: string;
  /** Commit of bench.ref the lockfiles (and the snapshot) are from; the job stages exactly this commit. */
  baseCommit: string;
  lockfiles: Record<string, string>;
  manifest: DepsManifest;
  /** True when the snapshot already existed (no deps instance was launched for this job). */
  reused: boolean;
}
