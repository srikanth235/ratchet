import { lstat, readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  MAX_ENVIRONMENT_FILE_BYTES,
  MAX_ENVIRONMENT_TOTAL_BYTES,
  environmentModuleOf,
  hashEnvironmentFiles,
  normalizeRepoPath,
  validateEnvironment,
  type EnvironmentModuleRef,
  type RatchetConfig,
  type SandboxEnvironment,
} from '@ratchet/core';
import { EngineError } from './errors.js';

/** A directory of a checked-out tree as a flat, sorted list of `{ path (relative to dir), content }`. Symlinks and binaries are refused. */
export async function readEnvironmentTree(dir: string, rel = '', acc: Array<{ path: string; content: string }> = [], total = { bytes: 0 }): Promise<Array<{ path: string; content: string }>> {
  const names = await readdir(join(dir, rel));
  for (const name of names.sort()) {
    const r = rel ? `${rel}/${name}` : name;
    const st = await lstat(join(dir, r));
    if (st.isSymbolicLink()) throw new Error(`${r} is a symbolic link; an environment must be plain files`);
    if (st.isDirectory()) {
      await readEnvironmentTree(dir, r, acc, total);
      continue;
    }
    if (!st.isFile()) continue;
    if (st.size > MAX_ENVIRONMENT_FILE_BYTES) throw new Error(`${r} is ${Math.round(st.size / 1024)} KB (limit ${MAX_ENVIRONMENT_FILE_BYTES / 1024} KB)`);
    total.bytes += st.size;
    if (total.bytes > MAX_ENVIRONMENT_TOTAL_BYTES) throw new Error('the environment is larger than 2 MB');
    const buf = await readFile(join(dir, r));
    if (buf.includes(0)) throw new Error(`${r} is not a text file`);
    acc.push({ path: r, content: buf.toString('utf8') });
  }
  return acc;
}

/**
 * The environment of an optimization, read from a checkout of the PROTECTED ref (`root` is that checkout; a candidate's tree is
 * never used). `undefined` for scope `none`, for `single` without a drafted module, and when the config names nothing.
 *
 * A drafted module is validated here with the same allow-list the user saw when approving (module-only mode); a custom
 * (`infra:`) module is checked for the shape only, the sandbox provider scans it for commands and validates its outputs.
 */
export async function readEnvironment(root: string, config: RatchetConfig, commit: string, ref: string): Promise<SandboxEnvironment | undefined> {
  const mod: EnvironmentModuleRef | null = environmentModuleOf(config);
  if (!mod) return undefined;
  const dir = join(root, ...normalizeRepoPath(mod.path).split('/'));
  let files: Array<{ path: string; content: string }>;
  try {
    const st = await lstat(dir);
    if (st.isSymbolicLink() || !st.isDirectory()) throw new Error(`${mod.path} is not a plain directory`);
    files = await readEnvironmentTree(dir);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === 'ENOENT') {
      if (!mod.required) return undefined;
      throw new EngineError(
        'invalid_config',
        `${mod.kind === 'custom' ? 'infra' : 'environment.scope ' + config.environment?.scope} needs ${mod.path}/ on ${ref}, which does not exist there. Approve the environment (commit or merge its pull request) first.`,
      );
    }
    throw new EngineError('invalid_config', `cannot read ${mod.path} at ${ref}: ${(err as Error).message}`, err);
  }
  if (files.length === 0 || !files.some((f) => f.path.endsWith('.tf'))) {
    if (!mod.required) return undefined;
    throw new EngineError('invalid_config', `${mod.path}/ at ${ref} has no OpenTofu (.tf) files`);
  }
  const scope = config.environment?.scope ?? 'full';
  if (mod.kind === 'drafted') {
    const v = validateEnvironment({
      scope,
      moduleOnly: true,
      files: files.map((f) => ({ path: `${mod.path}/${f.path}`, content: f.content })),
    });
    const failed = v.checks.filter((c) => !c.ok && c.id !== 'cost' && c.id !== 'scope');
    if (failed.length > 0) {
      throw new EngineError(
        'invalid_config',
        `the environment at ${mod.path}/ (${ref}) does not satisfy the sandbox contract:\n${failed.map((c) => `  - ${c.label}: ${c.detail}`).join('\n')}`,
        failed,
      );
    }
  }
  return { kind: mod.kind, scope, path: mod.path, commit, hash: hashEnvironmentFiles([...files, { path: '#scope', content: scope }]), files };
}
