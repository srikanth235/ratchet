import { createHash } from 'node:crypto';
import { DEFAULT_ENVIRONMENT_INFRA_DIR, type EnvironmentScope } from '../config/schema.js';
import { normalizeRepoPath } from '../protected.js';

/** The slice of a parsed `ratchet.yaml` that decides which environment module (if any) the sandbox is built from. */
export interface EnvironmentConfigInput {
  infra?: string | undefined;
  environment?: { scope?: EnvironmentScope | undefined } | undefined;
}

export interface EnvironmentModuleRef {
  /**
   * `custom`: `infra:` in ratchet.yaml, a standalone module that replaces the standard one (must satisfy every contract output).
   * `drafted`: the approved drafted environment under `infra/ratchet/`, built on the base module.
   */
  kind: 'drafted' | 'custom';
  /** Repo-relative directory. */
  path: string;
  /** False when the scope alone does not say there is a module (`single`): the directory may be absent, and then there is none. */
  required: boolean;
}

/**
 * Where an optimization's environment module lives, from its config alone; `null` for scope `none` / no scope (the standard
 * sandbox, nothing extra). The engine then reads the directory from the protected ref.
 */
export function environmentModuleOf(config: EnvironmentConfigInput): EnvironmentModuleRef | null {
  if (config.infra) return { kind: 'custom', path: normalizeRepoPath(config.infra), required: true };
  const scope = config.environment?.scope;
  if (scope === 'full') return { kind: 'drafted', path: DEFAULT_ENVIRONMENT_INFRA_DIR, required: true };
  if (scope === 'single') return { kind: 'drafted', path: DEFAULT_ENVIRONMENT_INFRA_DIR, required: false };
  return null;
}

/** Order-independent content hash of an environment's files (path + bytes), 10 hex characters. */
export function hashEnvironmentFiles(files: ReadonlyArray<{ path: string; content: string }>): string {
  const h = createHash('sha256');
  for (const f of [...files].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))) {
    h.update(`${f.path}\0${Buffer.byteLength(f.content)}\0`);
    h.update(f.content);
    h.update('\0');
  }
  return h.digest('hex').slice(0, 10);
}

/**
 * Sandbox id of an environment sandbox. Two optimizations share a sandbox only when region AND environment are the same;
 * `[a-z0-9-]`, 2-32 chars (`ratchet-env-` + 8 hex = 20).
 */
export function environmentSandboxId(region: string, hash: string): string {
  const h = createHash('sha256').update(`${region}\0${hash}`).digest('hex').slice(0, 8);
  return `ratchet-env-${h}`;
}
