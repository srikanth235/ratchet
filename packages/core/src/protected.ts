import { posix } from 'node:path';

/** A changed path that matched a protected glob. */
export interface ProtectedViolation {
  path: string;
  pattern: string;
}

/** Normalise to a repo-relative POSIX path: `\` -> `/`, strip leading `./` and `/`, collapse `a/../b`. */
export function normalizeRepoPath(p: string): string {
  let s = p.replace(/\\/g, '/');
  s = posix.normalize(s);
  s = s.replace(/^(\.\/)+/, '').replace(/^\/+/, '');
  return s === '.' ? '' : s;
}

const cache = new Map<string, RegExp>();

/**
 * Compile a glob to an anchored RegExp. Supported syntax:
 *   `*`  any run of characters except `/`
 *   `?`  one character except `/`
 *   `**` any run of characters including `/`; `**\/` also matches zero directories
 *   `[abc]`, `[a-z]`, `[!abc]`  character classes
 *   `{a,b}` alternation (no nesting)
 * Everything else is literal. Matching is case-sensitive, and dotfiles are matched
 * by wildcards (deliberately: protection should err on the side of blocking).
 */
export function globToRegExp(glob: string): RegExp {
  const hit = cache.get(glob);
  if (hit) return hit;
  let re = '';
  let inBrace = false;
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i]!;
    if (c === '*') {
      if (glob[i + 1] === '*') {
        i++;
        if (glob[i + 1] === '/') {
          i++;
          re += '(?:.*/)?';
        } else {
          re += '.*';
        }
      } else {
        re += '[^/]*';
      }
    } else if (c === '?') {
      re += '[^/]';
    } else if (c === '[') {
      const close = glob.indexOf(']', i + 2);
      if (close === -1) {
        re += '\\[';
      } else {
        let body = glob.slice(i + 1, close);
        if (body.startsWith('!')) body = '^' + body.slice(1);
        re += `[${body.replace(/\\/g, '\\\\')}]`;
        i = close;
      }
    } else if (c === '{' && glob.indexOf('}', i) !== -1 && !inBrace) {
      inBrace = true;
      re += '(?:';
    } else if (c === '}' && inBrace) {
      inBrace = false;
      re += ')';
    } else if (c === ',' && inBrace) {
      re += '|';
    } else {
      re += c.replace(/[.+^$()|\\{}\]]/g, '\\$&');
    }
  }
  const compiled = new RegExp(`^${re}$`);
  cache.set(glob, compiled);
  return compiled;
}

/**
 * Does `path` match `glob`? A path also matches when the glob matches one of its
 * parent directories (so `bench` or `bench/` protects `bench/k6/search.js`).
 * Patterns match against the full repo-relative path: `ratchet.yaml` only matches the
 * root file; use `**\/ratchet.yaml` for any depth.
 */
export function matchesGlob(path: string, glob: string): boolean {
  const p = normalizeRepoPath(path);
  const g = glob.replace(/\\/g, '/').replace(/^(\.\/)+/, '').replace(/^\/+/, '').replace(/\/+$/, '');
  if (p === '' || g === '') return false;
  const re = globToRegExp(g);
  const parts = p.split('/');
  for (let n = 1; n <= parts.length; n++) {
    if (re.test(parts.slice(0, n).join('/'))) return true;
  }
  return false;
}

/**
 * Return one violation per changed path that matches any protected glob (the first
 * matching pattern is reported). Pass both old and new paths for renames.
 */
export function findProtectedViolations(
  changedPaths: readonly string[],
  protectedGlobs: readonly string[],
): ProtectedViolation[] {
  const out: ProtectedViolation[] = [];
  const seen = new Set<string>();
  for (const path of changedPaths) {
    const norm = normalizeRepoPath(path);
    if (seen.has(norm)) continue;
    seen.add(norm);
    const pattern = protectedGlobs.find((g) => matchesGlob(norm, g));
    if (pattern !== undefined) out.push({ path: norm, pattern });
  }
  return out;
}
