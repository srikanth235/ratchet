import { chmod, copyFile, lstat, mkdir, readdir, readFile, readlink, rm, symlink } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { matchesGlob } from '@ratchet/core';

/**
 * Protected-path overlay (local runner).
 *
 * Rule: a path is *protected* iff it matches one of the config's `protected` globs (core's
 * `matchesGlob`). Protected paths ALWAYS come from `bench.ref`, never from the candidate:
 *
 *  1. every file / symlink in the bench.ref tree that is protected is copied over the candidate
 *     checkout, replacing whatever the candidate has at that path (files, symlinks, directories);
 *  2. with `prune` (default), every protected file / symlink the candidate has that bench.ref does
 *     not is deleted (so a candidate cannot add `bench/extra.js` for a directory-scanning bench).
 *     This assumes bench.ref is a full snapshot (the baseline plus the trusted bench).
 *
 * The copy never follows symlinks: any ancestor of a target that is a symlink or a regular file in
 * the candidate tree is removed and replaced by a real directory first. `.git` and `node_modules`
 * are never walked. Mode bits are preserved, ownership is not.
 */

const SKIP_DIRS = new Set(['.git', 'node_modules']);

export const isProtected = (path: string, globs: readonly string[]): boolean => globs.some((g) => matchesGlob(path, g));

/** Relative POSIX paths of every file and symlink under `root` (directories are not listed, symlinks are not followed). */
export async function listFiles(root: string): Promise<string[]> {
  const out: string[] = [];
  const walk = async (rel: string): Promise<void> => {
    const entries = await readdir(join(root, rel), { withFileTypes: true });
    for (const e of entries) {
      const r = rel === '' ? e.name : `${rel}/${e.name}`;
      if (e.isDirectory()) {
        if (!SKIP_DIRS.has(e.name)) await walk(r);
      } else {
        out.push(r);
      }
    }
  };
  await walk('');
  return out.sort();
}

async function ensureRealParents(root: string, rel: string): Promise<void> {
  const parts = rel.split('/').slice(0, -1);
  let cur = root;
  for (const part of parts) {
    cur = join(cur, part);
    const st = await lstat(cur).catch(() => undefined);
    if (st && !st.isDirectory()) await rm(cur, { force: true, recursive: true }); // symlink or file in the way
    if (!st || !st.isDirectory()) await mkdir(cur);
  }
}

async function copyEntry(srcRoot: string, dstRoot: string, rel: string): Promise<void> {
  const src = join(srcRoot, rel);
  const dst = join(dstRoot, rel);
  await ensureRealParents(dstRoot, rel);
  await rm(dst, { force: true, recursive: true });
  await mkdir(dirname(dst), { recursive: true });
  const st = await lstat(src);
  if (st.isSymbolicLink()) {
    await symlink(await readlink(src), dst);
  } else {
    await copyFile(src, dst);
    await chmod(dst, st.mode & 0o777);
  }
}

async function sameEntry(a: string, b: string): Promise<boolean> {
  const [sa, sb] = await Promise.all([lstat(a), lstat(b).catch(() => undefined)]);
  if (!sb) return false;
  if (sa.isSymbolicLink() || sb.isSymbolicLink()) {
    return sa.isSymbolicLink() && sb.isSymbolicLink() && (await readlink(a)) === (await readlink(b));
  }
  if (!sb.isFile() || sa.size !== sb.size) return false;
  const [ba, bb] = await Promise.all([readFile(a), readFile(b)]);
  return ba.equals(bb);
}

export interface OverlayResult {
  /** Protected paths taken from bench.ref (sorted). */
  copied: string[];
  /** Candidate-only protected paths that were deleted (sorted). */
  removed: string[];
}

export async function applyOverlay(opts: {
  codeDir: string;
  benchDir: string;
  globs: readonly string[];
  prune?: boolean;
}): Promise<OverlayResult> {
  const copied = (await listFiles(opts.benchDir)).filter((p) => isProtected(p, opts.globs));
  const removed: string[] = [];
  if (opts.prune ?? true) {
    const inBench = new Set(copied);
    for (const p of (await listFiles(opts.codeDir)).filter((p) => isProtected(p, opts.globs))) {
      await rm(join(opts.codeDir, p), { force: true, recursive: true });
      if (!inBench.has(p)) removed.push(p);
    }
  }
  for (const p of copied) await copyEntry(opts.benchDir, opts.codeDir, p);
  return { copied, removed };
}

/**
 * Compare the protected files of the code tree with bench.ref's, restore any that differ and
 * return their paths. Non-empty means an earlier step (build, tests, the bench itself) modified
 * trusted files.
 */
export async function restoreProtected(opts: { codeDir: string; benchDir: string; files: readonly string[] }): Promise<string[]> {
  const restored: string[] = [];
  for (const p of opts.files) {
    // an ancestor may have been swapped for a symlink; lstat of the final path would follow it
    if (!(await sameEntryThroughRealParents(opts.benchDir, opts.codeDir, p))) {
      await copyEntry(opts.benchDir, opts.codeDir, p);
      restored.push(p);
    }
  }
  return restored;
}

async function sameEntryThroughRealParents(srcRoot: string, dstRoot: string, rel: string): Promise<boolean> {
  const parts = rel.split('/').slice(0, -1);
  let cur = dstRoot;
  for (const part of parts) {
    cur = join(cur, part);
    const st = await lstat(cur).catch(() => undefined);
    if (!st || !st.isDirectory()) return false;
  }
  return sameEntry(join(srcRoot, rel), join(dstRoot, rel));
}
