import { existsSync } from 'node:fs';
import { join } from 'node:path';
import type { AgentKind } from '@ratchet/core';
import type { LiveSettings } from '../shared/live-settings.js';
import type { PreflightItem, SecretName } from '../shared/types.js';
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

export interface ReadinessInput {
  live: LiveSettings;
  hasSecret(name: SecretName): boolean;
  sandboxApplied: boolean;
  defaults: LivePaths;
}

/**
 * What a live run needs, one line each. `fail` = cannot run yet. The same list drives the Settings screen and the
 * live part of the New optimization preflight, so the two never disagree.
 */
export function liveReadiness({ live, hasSecret, sandboxApplied, defaults }: ReadinessInput): PreflightItem[] {
  const paths = effectivePaths(live, defaults);
  const item = (id: string, label: string, ok: boolean, okDetail: string, failDetail: string): PreflightItem => ({ id, label, state: ok ? 'ok' : 'fail', detail: ok ? okDetail : failDetail });
  const hasModelKey = hasSecret('anthropicApiKey') || hasSecret('openaiApiKey');
  return [
    item('aws', 'AWS sign-in', live.awsProfile !== '', `${live.awsProfile}${live.awsAuth === 'sso' ? ' (SSO)' : ''} · ${live.awsRegion || 'us-east-1'}`, 'choose an AWS profile'),
    item('tofu', 'OpenTofu', paths.tofuBinary !== '' && paths.moduleDir !== '', `${paths.tofuBinary} · ${paths.moduleDir}`, 'set the tofu binary and module directory'),
    item('github', 'GitHub repository and token', live.githubRepo !== '' && hasSecret('githubToken'), live.githubRepo, live.githubRepo === '' ? 'set the repository (owner/name)' : 'store a GitHub token'),
    item('models', 'Model API key', hasModelKey, [hasSecret('anthropicApiKey') && 'Anthropic', hasSecret('openaiApiKey') && 'OpenAI'].filter(Boolean).join(' and '), 'store an Anthropic or OpenAI key'),
    item('images', 'Runner and bench AMIs', live.runnerImageRef !== '' && live.benchImageRef !== '', `${live.runnerImageRef} / ${live.benchImageRef}`, 'set both AMI ids'),
    item('sandbox', 'Sandbox applied', sandboxApplied, 'applied', 'apply the sandbox under Environments'),
  ];
}

/** Problems that stop the sandbox itself from being planned or applied (the rest only block optimizations). */
export function sandboxSetupProblems(live: LiveSettings, defaults: LivePaths): string[] {
  const paths = effectivePaths(live, defaults);
  const problems: string[] = [];
  if (!live.awsProfile) problems.push('Choose an AWS profile in Settings.');
  if (!live.awsRegion) problems.push('Set an AWS region in Settings.');
  if (!paths.tofuBinary || !paths.moduleDir) problems.push('Set the OpenTofu binary and module directory in Settings.');
  return problems;
}
