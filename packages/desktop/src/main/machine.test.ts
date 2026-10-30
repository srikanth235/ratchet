import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { HarnessInfo } from '@ratchet/agents';
import { afterAll, describe, expect, it } from 'vitest';
import type { EngineEventVM } from '../shared/types.js';
import { listAwsProfiles, parseAwsFiles } from './aws-profiles.js';
import { HarnessCache } from './harnesses.js';
import { githubSlug, inspectLocalFolder } from './local-repo.js';
import { createPowerSaveHook } from './power.js';

const CONFIG = `
# a comment
[default]
region = us-west-2

[profile dev-sso]
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Admin
region = eu-west-1

[profile legacy-sso]
sso_start_url = https://corp.awsapps.com/start
sso_region = us-east-1

[profile deploy-role]
role_arn = arn:aws:iam::111122223333:role/Deploy
source_profile = default

[profile with-process]
credential_process = /usr/local/bin/get-creds

[profile "weird name"]
region = us-east-1

[profile static-in-config]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[sso-session corp]
sso_start_url = https://corp.awsapps.com/start
sso_region = us-east-1
sso_registration_scopes = sso:account:access
`;

const CREDENTIALS = `
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[ci-user]
aws_access_key_id = AKIAI44QH8DHBEXAMPLE
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY
`;

describe('AWS profiles from ~/.aws', () => {
  it('lists profile and SSO session names with their kind, never a key', () => {
    const r = parseAwsFiles({ config: CONFIG, credentials: CREDENTIALS });
    expect(r.filesFound).toBe(true);
    expect(r.ssoSessions).toEqual(['corp']);
    expect(r.profiles.map((p) => [p.name, p.kind])).toEqual([
      ['default', 'keys'],
      ['ci-user', 'keys'],
      ['deploy-role', 'role'],
      ['dev-sso', 'sso'],
      ['legacy-sso', 'sso'],
      ['static-in-config', 'keys'],
      ['with-process', 'process'],
    ]);
    expect(r.profiles.find((p) => p.name === 'dev-sso')).toMatchObject({ ssoSession: 'corp', source: 'config' });
    expect(r.profiles.find((p) => p.name === 'ci-user')).toMatchObject({ source: 'credentials' });
    const dump = JSON.stringify(r);
    for (const secret of ['AKIA', 'wJalr', 'je7Mt', '111122223333', 'awsapps.com']) expect(dump).not.toContain(secret);
  });

  it('handles missing files and empty ones', () => {
    expect(parseAwsFiles({ config: null, credentials: null })).toEqual({ profiles: [], ssoSessions: [], filesFound: false });
    expect(parseAwsFiles({ config: '', credentials: null }).filesFound).toBe(true);
  });

  it('reads AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE, else ~/.aws', () => {
    const reads: string[] = [];
    const readFile = (p: string) => {
      reads.push(p);
      if (p.endsWith('/custom/config')) return '[profile x]\nregion=us-east-1\n';
      throw new Error('ENOENT');
    };
    const r = listAwsProfiles({ env: { AWS_CONFIG_FILE: '/custom/config' }, home: '/home/u', readFile });
    expect(r.profiles.map((p) => p.name)).toEqual(['x']);
    expect(reads).toEqual(['/custom/config', '/home/u/.aws/credentials']);
  });
});

// ------------------------------------------------------------------ harness cache

const info = (kind: HarnessInfo['kind'], over: Partial<HarnessInfo> = {}): HarnessInfo => ({ kind, installed: true, version: '1.0.0', path: `/bin/${kind}`, auth: 'signed-in', hint: 'ok', ...over });

describe('HarnessCache', () => {
  it('probes once, caches, and probes again on refresh', async () => {
    let n = 0;
    const cache = new HarnessCache({
      loginPath: async () => '/usr/bin:/opt/bin',
      detect: async (p) => {
        n++;
        return [info('claude-code', { version: `${n}.0.0` }), info('codex', { installed: false, auth: 'signed-out', version: null, path: null, hint: 'npm i -g @openai/codex' })].map((h) => ({ ...h, hint: `${h.hint} via ${p}` }));
      },
    });
    const a = await cache.get();
    const b = await cache.get();
    expect(n).toBe(1);
    expect(a).toBe(b);
    expect(a.loginPath).toBe('/usr/bin:/opt/bin');
    expect(a.harnesses[0]!.version).toBe('1.0.0');
    expect((await cache.refresh()).harnesses[0]!.version).toBe('2.0.0');
    expect((await cache.get()).harnesses[0]!.version).toBe('2.0.0');
  });

  it('a failing probe is an empty result, not a crash', async () => {
    const cache = new HarnessCache({ loginPath: async () => { throw new Error('shell hung'); }, detect: async () => [] });
    expect(await cache.get()).toEqual({ harnesses: [], loginPath: undefined });
  });
});

// ------------------------------------------------------------------ sleep prevention

describe('powerSaveBlocker hook', () => {
  it('starts prevent-app-suspension on run-active and stops it when the last run ends', () => {
    const calls: string[] = [];
    let next = 7;
    const hook = createPowerSaveHook({ start: (t) => (calls.push(`start:${t}`), next++), stop: (id) => calls.push(`stop:${id}`) });
    const ev = (active: boolean): EngineEventVM => ({ type: 'run-active', active, optimizationIds: active ? ['opt'] : [] });
    hook.onEvent({ type: 'spend', optimizationId: 'opt', totalUsd: 1 });
    expect(calls).toEqual([]);
    hook.onEvent(ev(true));
    hook.onEvent(ev(true)); // idempotent: no second blocker
    expect(calls).toEqual(['start:prevent-app-suspension']);
    expect(hook.held()).toBe(true);
    hook.onEvent(ev(false));
    hook.onEvent(ev(false));
    expect(calls).toEqual(['start:prevent-app-suspension', 'stop:7']);
    expect(hook.held()).toBe(false);
    hook.onEvent(ev(true));
    hook.release(); // on quit
    expect(calls.at(-1)).toBe('stop:8');
  });
});

// ------------------------------------------------------------------ local folder

describe('githubSlug', () => {
  it.each([
    ['https://github.com/acme/api.git', 'acme/api'],
    ['https://github.com/acme/api', 'acme/api'],
    ['git@github.com:acme/api.git', 'acme/api'],
    ['ssh://git@github.com/acme/api.git', 'acme/api'],
    ['https://x-access-token:abc@github.com/acme/api.git', 'acme/api'],
    ['https://gitlab.com/acme/api.git', null],
    ['/srv/repos/api.git', null],
  ])('%s -> %s', (remote, slug) => expect(githubSlug(remote)).toBe(slug));
});

describe('inspectLocalFolder', () => {
  const dirs: string[] = [];
  afterAll(() => dirs.forEach((d) => rmSync(d, { recursive: true, force: true })));
  const tmp = () => (dirs.push(mkdtempSync(join(tmpdir(), 'ratchet-local-'))), dirs.at(-1)!);
  const git = (cwd: string, ...args: string[]) => execFileSync('git', args, { cwd, stdio: 'pipe', env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null' } });

  it('reads the GitHub remote, the branch and ratchet.yaml of a real repository', async () => {
    const dir = tmp();
    git(dir, 'init', '-q', '-b', 'trunk');
    git(dir, 'remote', 'add', 'origin', 'git@github.com:acme/search.git');
    writeFileSync(join(dir, 'ratchet.yaml'), 'version: 0\n');
    const r = await inspectLocalFolder(dir);
    expect(r).toMatchObject({ repo: 'acme/search', baseRef: 'trunk', configYaml: 'version: 0\n', problem: null });
  });

  it('explains a folder that cannot be used', async () => {
    const plain = tmp();
    expect((await inspectLocalFolder(plain)).problem).toMatch(/not a git repository/);
    expect((await inspectLocalFolder(join(plain, 'missing'))).problem).toMatch(/does not exist/);
    const noRemote = tmp();
    git(noRemote, 'init', '-q');
    expect((await inspectLocalFolder(noRemote))).toMatchObject({ repo: null, problem: expect.stringMatching(/no GitHub remote/) });
    const elsewhere = tmp();
    git(elsewhere, 'init', '-q');
    git(elsewhere, 'remote', 'add', 'origin', 'https://gitlab.com/acme/x.git');
    expect((await inspectLocalFolder(elsewhere)).problem).toMatch(/not on GitHub/);
    mkdirSync(join(plain, 'sub'));
  });
});
