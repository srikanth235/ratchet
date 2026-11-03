import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { DEFAULT_CONFIG, type Config } from './types.js';

/** Config files looked for in every directory, nearest wins: a dedicated rc file, or a `taskr` key in package.json. */
const RC_FILES = ['.taskrrc.json', '.taskrrc', 'taskr.config.json', 'package.json'];

export interface ConfigSource {
  /** Absolute path of the file. */
  file: string;
  dir: string;
  settings: Partial<Config>;
}

export interface Discovered {
  config: Config;
  /** Directory of the nearest config that sets `root` (or of the nearest config), else the start directory. */
  rootDir: string;
  /** Files that contributed, nearest first. */
  sources: ConfigSource[];
}

function readSettings(file: string, name: string): Partial<Config> | undefined {
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(file, 'utf8'));
  } catch {
    return undefined; // unreadable or not JSON: not ours
  }
  const settings = name === 'package.json' ? (raw as { taskr?: unknown }).taskr : raw;
  return settings && typeof settings === 'object' && !Array.isArray(settings) ? (settings as Partial<Config>) : undefined;
}

/**
 * Find the effective configuration for `cwd`. Every directory from `cwd` up to the filesystem root is searched;
 * the settings of nearer files override those of farther ones, and a file with `"root": true` ends the cascade
 * (farther files never apply). The search covers the whole ancestor chain before it decides what applies.
 */
export function discoverConfig(cwd: string): Discovered {
  const start = resolve(cwd);
  const all: ConfigSource[] = [];
  let dir = start;
  for (;;) {
    for (const name of RC_FILES) {
      const file = join(dir, name);
      if (!existsSync(file)) continue;
      const settings = readSettings(file, name);
      if (settings) all.push({ file, dir, settings });
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }

  const sources: ConfigSource[] = [];
  for (const s of all) {
    sources.push(s);
    if (s.settings.root === true) break;
  }
  let config: Config = { ...DEFAULT_CONFIG };
  for (const s of [...sources].reverse()) {
    const { dataFile, ...rest } = s.settings;
    config = { ...config, ...rest, ...(dataFile === undefined ? {} : { dataFile: resolve(s.dir, dataFile) }) };
  }
  if (!sources.some((s) => s.settings.dataFile !== undefined)) config.dataFile = resolve(sources[0]?.dir ?? start, config.dataFile);
  return { config, rootDir: sources.at(-1)?.dir ?? start, sources };
}
