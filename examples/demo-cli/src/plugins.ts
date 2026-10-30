import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import type { Plugin } from './types.js';

const PLUGIN_NAME = /^(taskr-plugin-[\w.-]+|@taskr\/plugin-[\w.-]+)$/;

function manifest(dir: string): { name?: string; version?: string } | undefined {
  const file = join(dir, 'package.json');
  if (!existsSync(file)) return undefined;
  try {
    return JSON.parse(readFileSync(file, 'utf8')) as { name?: string; version?: string };
  } catch {
    return undefined;
  }
}

/**
 * Plugins are installed packages named `taskr-plugin-*` or `@taskr/plugin-*`. To find them every package of every
 * `node_modules` directory above `cwd` is opened and its manifest checked.
 */
export function scanPlugins(cwd: string): Plugin[] {
  const found = new Map<string, Plugin>();
  let dir = resolve(cwd);
  for (;;) {
    const modules = join(dir, 'node_modules');
    if (existsSync(modules)) {
      for (const entry of readdirSync(modules)) {
        const candidates = entry.startsWith('@') ? readdirSync(join(modules, entry)).map((n) => join(modules, entry, n)) : [join(modules, entry)];
        for (const pkgDir of candidates) {
          const pkg = manifest(pkgDir);
          if (pkg?.name && PLUGIN_NAME.test(pkg.name) && !found.has(pkg.name)) found.set(pkg.name, { name: pkg.name, version: pkg.version ?? '0.0.0' });
        }
      }
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return [...found.values()].sort((a, b) => a.name.localeCompare(b.name));
}
