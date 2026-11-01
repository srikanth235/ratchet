import { discoverConfig, type ConfigSource } from './config.js';
import { TaskrError } from './errors.js';
import { scanPlugins } from './plugins.js';
import { loadStore, type Store } from './store.js';
import { detectTerminal, type Terminal } from './terminal.js';
import { isKnownZone } from './tz.js';
import type { Config, Plugin } from './types.js';

export interface Io {
  out(text: string): void;
  err(text: string): void;
}

/** Everything a command needs, resolved once per process. */
export interface Context extends Io {
  cwd: string;
  env: NodeJS.ProcessEnv;
  /** "Now": TASKR_NOW (an ISO timestamp) when set, so output can be reproduced. */
  now: Date;
  config: Config;
  rootDir: string;
  sources: ConfigSource[];
  plugins: Plugin[];
  terminal: Terminal;
  store: Store;
}

export function createContext(io: Io, cwd: string, env: NodeJS.ProcessEnv): Context {
  const { config, rootDir, sources } = discoverConfig(cwd);
  if (!isKnownZone(config.tz)) throw new TaskrError('E0101', { tz: config.tz });
  const plugins = scanPlugins(cwd);
  const terminal = detectTerminal();
  const store = loadStore(config.dataFile);
  const now = env.TASKR_NOW ? new Date(env.TASKR_NOW) : new Date();
  return { ...io, cwd, env, now, config, rootDir, sources, plugins, terminal, store };
}
