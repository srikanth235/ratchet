import { normalizeRepoPath } from './protected.js';
import { findEvaluatorTemplate } from './templates/registry.js';

/**
 * Automatic protected paths (docs/SIMPLIFIED_MODEL.md: "No knobs the system can derive").
 *
 * A candidate that changes a protected path is blocked, and the bench takes protected files from the evaluator ref
 * instead of the candidate. Which paths those are used to be typed under `protected:`; now most are derived:
 *
 *  - `ratchet.yaml` itself (the config path in use);
 *  - the evaluator's files: the template's (`evaluator.template`), plus every `bench/` or `eval/` directory that
 *    `evaluator.run` references;
 *  - the environment: `environment.image` (and `.devcontainer/` when the image lives there) and the `infra` module;
 *  - lockfiles and install config (`INSTALL_FILES`): the bench has no network, so dependencies are those of the base ref;
 *  - the user's own `protected:` list, which only ADDS to the above.
 */

/** Lockfiles and install configuration, at any depth. Dependency changes are not allowed in v0. */
export const INSTALL_FILES = [
  'pnpm-lock.yaml',
  'package-lock.json',
  'npm-shrinkwrap.json',
  'yarn.lock',
  'bun.lock',
  'bun.lockb',
  '.npmrc',
  '.yarnrc',
  '.yarnrc.yml',
  'pnpm-workspace.yaml',
  '.pnpmfile.cjs',
] as const;

/** Directory names whose contents are treated as the evaluator when `evaluator.run` points into them. */
export const EVALUATOR_DIRS = ['bench', 'eval'] as const;

export type ProtectedSource = 'ratchet' | 'evaluator' | 'environment' | 'install' | 'user';

export interface ProtectedEntry {
  glob: string;
  source: ProtectedSource;
  /** Plain words, for the UI and for the reason shown when a candidate is blocked. */
  reason: string;
}

/** The slice of a parsed `ratchet.yaml` that decides what is protected. A `RatchetConfig` fits. */
export interface ProtectedConfigInput {
  protected?: readonly string[] | undefined;
  evaluator: { run?: string | undefined; template?: string | undefined };
  environment?: { image?: string | undefined } | undefined;
  infra?: string | undefined;
}

export interface EffectiveProtected {
  /** Everything, de-duplicated, in the order: config, evaluator, environment, install, user. */
  entries: ProtectedEntry[];
  /** Every glob: what the engine blocks candidates on. */
  globs: string[];
  /**
   * Everything except lockfiles and install config: what the bench takes from the evaluator ref and requires to be
   * non-empty. (Lockfiles are pinned separately through the dependency strategy.)
   */
  evaluatorGlobs: string[];
  /** Only the lockfiles and install config. */
  installGlobs: string[];
  /**
   * The evaluator's own files and the user's list: the globs of which at least one must match a file in the evaluator ref,
   * or the bench refuses to run (an evaluator ref that holds none of them is not trusted). `ratchet.yaml`, the
   * environment and the lockfiles are protected too but do not count here.
   */
  required: string[];
}

/**
 * `bench/` and `eval/` directories referenced by an evaluation command. For `node dist/bench/x.js` that is
 * `dist/bench/**` and, as the likely source of the built file, `bench/**`. Paths that escape the repo are ignored.
 */
export function evaluatorDirsFromRun(run: string): string[] {
  const out: string[] = [];
  const dirs = EVALUATOR_DIRS.join('|');
  const re = new RegExp(`(?<![\\w.@/])((?:\\./)?(?:[\\w.@-]+/)*?)(${dirs})(?=/)`, 'g');
  for (const m of run.matchAll(re)) {
    const prefix = normalizeRepoPath(m[1] ?? '');
    if (prefix.split('/').includes('..') || prefix.startsWith('..')) continue;
    const dir = prefix ? `${prefix.replace(/\/$/, '')}/${m[2]}` : m[2]!;
    out.push(`${dir}/**`);
    if (prefix) out.push(`${m[2]}/**`);
  }
  return [...new Set(out)];
}

/**
 * The effective protected set for a config. Pure.
 * @param opts.configPath path of the config inside the repo (default `ratchet.yaml`)
 */
export function effectiveProtected(config: ProtectedConfigInput, opts: { configPath?: string } = {}): EffectiveProtected {
  const entries: ProtectedEntry[] = [];
  const seen = new Set<string>();
  const add = (glob: string, source: ProtectedSource, reason: string) => {
    const g = glob.replace(/\\/g, '/').replace(/^(\.\/)+/, '').replace(/^\/+/, '');
    if (!g || seen.has(g)) return;
    seen.add(g);
    entries.push({ glob: g, source, reason });
  };

  add(normalizeRepoPath(opts.configPath ?? 'ratchet.yaml') || 'ratchet.yaml', 'ratchet', 'the optimization config');

  const tpl = config.evaluator.template ? findEvaluatorTemplate(config.evaluator.template) : undefined;
  for (const g of tpl?.protect ?? []) add(g, 'evaluator', `the ${tpl!.id} evaluator`);
  if (config.evaluator.run) {
    for (const g of evaluatorDirsFromRun(config.evaluator.run)) add(g, 'evaluator', 'the evaluator referenced by evaluator.run');
  }

  const image = config.environment?.image ? normalizeRepoPath(config.environment.image) : '';
  if (image) {
    add(image, 'environment', 'the environment image');
    if (image.startsWith('.devcontainer/')) add('.devcontainer/**', 'environment', 'the environment image');
  }
  if (config.infra) add(normalizeRepoPath(config.infra), 'environment', 'the custom infrastructure module');

  for (const f of INSTALL_FILES) add(`**/${f}`, 'install', 'dependencies are fixed: the bench has no network');

  for (const g of config.protected ?? []) add(g, 'user', 'listed under protected: in ratchet.yaml');

  const globs = entries.map((e) => e.glob);
  return {
    entries,
    globs,
    evaluatorGlobs: entries.filter((e) => e.source !== 'install').map((e) => e.glob),
    installGlobs: entries.filter((e) => e.source === 'install').map((e) => e.glob),
    required: entries.filter((e) => e.source === 'evaluator' || e.source === 'user').map((e) => e.glob),
  };
}
