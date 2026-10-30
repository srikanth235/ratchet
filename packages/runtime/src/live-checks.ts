/**
 * "Test connection" for the Settings screen: small, read-only calls that prove each live setting works before any
 * money is spent. Everything is injectable so tests never touch the network; none of the results contains a secret.
 */
import { access } from 'node:fs/promises';
import { join } from 'node:path';
import { NodeProcessRunner, checkAwsCredentials, type CallerIdentity, type CheckAwsCredentialsOptions, type ProcessRunner } from '@ratchet/aws';
import { checkGitHubAccess, type CheckGitHubAccessOptions, type GitHubAccess } from '@ratchet/github';

export type ConnectionTarget = 'aws' | 'github' | 'anthropic' | 'openai' | 'tofu';

export interface ConnectionResult {
  target: ConnectionTarget;
  ok: boolean;
  /** One line for the UI. Never contains a key or token. */
  detail: string;
}

export interface ConnectionTester {
  aws(input: { region: string; profile?: string | undefined; expectedAccountId?: string | undefined }): Promise<ConnectionResult>;
  github(input: { repo: string; token: string }): Promise<ConnectionResult>;
  anthropic(input: { apiKey: string }): Promise<ConnectionResult>;
  openai(input: { apiKey: string }): Promise<ConnectionResult>;
  tofu(input: { binary: string; moduleDir: string }): Promise<ConnectionResult>;
}

export interface ConnectionTesterDeps {
  fetch?: typeof fetch;
  processRunner?: ProcessRunner;
  checkAws?: (o: CheckAwsCredentialsOptions) => Promise<CallerIdentity>;
  checkGitHub?: (o: CheckGitHubAccessOptions) => Promise<GitHubAccess>;
  /** Resolves when the path exists. */
  exists?: (path: string) => Promise<void>;
  /** Abort a model API probe after this long. Default 15 s. */
  timeoutMs?: number;
}

const redact = (text: string, secrets: string[]): string => secrets.filter((s) => s.length >= 4).reduce((t, s) => t.split(s).join('***'), text);

export function createConnectionTester(deps: ConnectionTesterDeps = {}): ConnectionTester {
  const doFetch = deps.fetch ?? fetch;
  const runner = deps.processRunner ?? new NodeProcessRunner();
  const checkAws = deps.checkAws ?? checkAwsCredentials;
  const checkGitHub = deps.checkGitHub ?? checkGitHubAccess;
  const exists = deps.exists ?? ((p: string) => access(p));
  const timeoutMs = deps.timeoutMs ?? 15_000;

  const wrap = async (target: ConnectionTarget, secrets: string[], fn: () => Promise<string>): Promise<ConnectionResult> => {
    try {
      return { target, ok: true, detail: redact(await fn(), secrets) };
    } catch (err) {
      return { target, ok: false, detail: redact((err as Error).message.split('\n')[0] ?? 'failed', secrets) };
    }
  };

  /** GET a model API's "list models" endpoint: 200 = the key works. */
  const probeModels = async (name: string, url: string, headers: Record<string, string>): Promise<string> => {
    const res = await doFetch(url, { method: 'GET', headers, signal: AbortSignal.timeout(timeoutMs) });
    if (res.status === 200) return `${name} accepted the key`;
    if (res.status === 401 || res.status === 403) throw new Error(`${name} rejected the key (${res.status})`);
    if (res.status === 429) return `${name} accepted the key (rate limited right now)`;
    throw new Error(`${name} answered ${res.status}`);
  };

  return {
    aws: ({ region, profile, expectedAccountId }) =>
      wrap('aws', [], async () => {
        const id = await checkAws({ region, ...(profile ? { profile } : {}), ...(expectedAccountId ? { expectedAccountId } : {}) });
        return `signed in to account ${id.accountId} in ${region}${profile ? ` as profile ${profile}` : ''}`;
      }),
    github: ({ repo, token }) =>
      wrap('github', [token], async () => {
        const a = await checkGitHub({ repo, token });
        if (!a.canPush) throw new Error(`${a.fullName} is readable but the token cannot push: grant it Contents: read and write and Pull requests: read and write`);
        return `${a.fullName} (${a.private ? 'private' : 'public'}, default branch ${a.defaultBranch}); the token can push`;
      }),
    anthropic: ({ apiKey }) =>
      wrap('anthropic', [apiKey], () => probeModels('Anthropic', 'https://api.anthropic.com/v1/models?limit=1', { 'x-api-key': apiKey, 'anthropic-version': '2023-06-01' })),
    openai: ({ apiKey }) =>
      wrap('openai', [apiKey], () => probeModels('OpenAI', 'https://api.openai.com/v1/models', { authorization: `Bearer ${apiKey}` })),
    tofu: ({ binary, moduleDir }) =>
      wrap('tofu', [], async () => {
        await exists(join(moduleDir, 'main.tf')).catch(() => {
          throw new Error(`${moduleDir} does not contain an OpenTofu module (main.tf not found)`);
        });
        const res = await runner.run({ command: binary, args: ['version'], cwd: moduleDir, env: { CHECKPOINT_DISABLE: '1' } }).catch((err: Error) => {
          throw new Error(`cannot run ${binary}: ${err.message}`);
        });
        if (res.exitCode !== 0) throw new Error(`\`${binary} version\` exited with status ${res.exitCode}`);
        const v = /OpenTofu v(\d+\.\d+\.\d+\S*)/.exec(res.stdout);
        if (!v) throw new Error(`${binary} is not OpenTofu (got "${res.stdout.trim().split('\n')[0] ?? ''}")`);
        return `OpenTofu ${v[1]}, module ${moduleDir}`;
      }),
  };
}
