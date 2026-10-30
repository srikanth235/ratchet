import { existsSync } from 'node:fs';
import { join } from 'node:path';
import type { AgentKind } from '@ratchet/core';
import type { LiveSettings } from '../shared/live-settings.js';
import type { SecretName } from '../shared/types.js';
import type { SettingsStore } from './settings.js';

/** What empty `tofuBinary` / `moduleDir` settings resolve to. */
export interface LivePaths {
  tofuBinary: string;
  moduleDir: string;
}

/**
 * Packaged: the module ships in `Resources/infra/aws` and a pinned `tofu` in `Resources/bin/tofu` (when the build
 * includes one); from a checkout: `<repo>/infra/aws` and `tofu` from PATH.
 */
export function defaultLivePaths(o: { isPackaged: boolean; resourcesPath: string; repoRoot: string; exists?: (path: string) => boolean }): LivePaths {
  const exists = o.exists ?? existsSync;
  const bundledTofu = join(o.resourcesPath, 'bin', process.platform === 'win32' ? 'tofu.exe' : 'tofu');
  return {
    moduleDir: o.isPackaged ? join(o.resourcesPath, 'infra', 'aws') : join(o.repoRoot, 'infra', 'aws'),
    tofuBinary: o.isPackaged && exists(bundledTofu) ? bundledTofu : 'tofu',
  };
}

export function effectivePaths(live: LiveSettings, defaults: LivePaths): LivePaths {
  return { tofuBinary: live.tofuBinary || defaults.tofuBinary, moduleDir: live.moduleDir || defaults.moduleDir };
}

const SECRET_FOR_ENV: Record<string, SecretName> = {
  ANTHROPIC_API_KEY: 'anthropicApiKey',
  OPENAI_API_KEY: 'openaiApiKey',
  GITHUB_TOKEN: 'githubToken',
};

/**
 * The engine's secret source: looks the value up in the main process only (keychain or environment). The engine
 * hands it to the runner's parameter store and nowhere else.
 */
export function secretSource(settings: SettingsStore): (kind: AgentKind, name: string) => string | undefined {
  return (_kind, name) => {
    const key = SECRET_FOR_ENV[name];
    return key ? (settings.getSecret(key) ?? undefined) : undefined;
  };
}
