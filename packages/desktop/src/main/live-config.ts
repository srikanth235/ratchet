import { existsSync } from 'node:fs';
import { join } from 'node:path';
import type { AgentKind } from '@ratchet/core';
import type { LiveSettings } from '../shared/live-settings.js';
import type { ResolvedConnections } from './connectors.js';

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

const AGENT_FOR_ENV: Record<string, 'claude-code' | 'codex'> = { ANTHROPIC_API_KEY: 'claude-code', OPENAI_API_KEY: 'codex' };

/**
 * The engine's secret source: looks the value up in the main process only. The engine hands it to the runner's parameter
 * store and nowhere else. The GitHub token and the API keys come from the connectors chosen for the repo; a model key falls back
 * to the environment only when no connector of that agent holds one (never next to a chosen login).
 */
export function secretSource(resolved: ResolvedConnections, env: (name: 'ANTHROPIC_API_KEY' | 'OPENAI_API_KEY') => string | null = () => null): (kind: AgentKind, name: string) => string | undefined {
  return (_kind, name) => {
    if (name === 'GITHUB_TOKEN') return resolved.github?.token;
    const agent = AGENT_FOR_ENV[name];
    if (!agent) return undefined;
    const key = resolved.cloudKeys[agent];
    if (key) return key;
    const chosenLogin = resolved.agents.some((a) => a.agent === agent && a.source === 'login');
    return chosenLogin ? undefined : (env(name as 'ANTHROPIC_API_KEY' | 'OPENAI_API_KEY') ?? undefined);
  };
}
