import { describe, expect, it, vi } from 'vitest';
import type { ProcessRunner } from '@ratchet/aws';
import { createConnectionTester } from './live-checks.js';

const KEY = 'sk-ant-api03-VERYSECRETKEYVALUE';
const reply = (status: number) => vi.fn(async () => new Response('{}', { status }));

describe('createConnectionTester', () => {
  it('aws: reports the account, passes region and profile through, and reports credential errors', async () => {
    const checkAws = vi.fn(async () => ({ accountId: '123456789012', arn: 'arn' }));
    const t = createConnectionTester({ checkAws });
    expect(await t.aws({ region: 'eu-west-1', profile: 'sandbox' })).toEqual({ target: 'aws', ok: true, detail: 'signed in to account 123456789012 in eu-west-1 as profile sandbox' });
    expect(checkAws).toHaveBeenCalledWith({ region: 'eu-west-1', profile: 'sandbox' });

    const expired = createConnectionTester({ checkAws: async () => { throw new Error('The SSO session associated with this profile has expired\nstack...'); } });
    expect(await expired.aws({ region: 'us-east-1', profile: 'sso' })).toEqual({ target: 'aws', ok: false, detail: 'The SSO session associated with this profile has expired' });
  });

  it('github: requires push rights and never leaks the token', async () => {
    const ok = createConnectionTester({ checkGitHub: async () => ({ fullName: 'acme/api', defaultBranch: 'main', private: true, canPush: true }) });
    expect(await ok.github({ repo: 'acme/api', token: 'ghp_x1234567890' })).toMatchObject({ ok: true, detail: expect.stringContaining('acme/api (private, default branch main)') });
    const ro = createConnectionTester({ checkGitHub: async () => ({ fullName: 'acme/api', defaultBranch: 'main', private: false, canPush: false }) });
    expect(await ro.github({ repo: 'acme/api', token: 'ghp_x1234567890' })).toMatchObject({ ok: false, detail: expect.stringContaining('cannot push') });
    const leaky = createConnectionTester({ checkGitHub: async () => { throw new Error('bad credentials ghp_x1234567890'); } });
    const r = await leaky.github({ repo: 'acme/api', token: 'ghp_x1234567890' });
    expect(r.ok).toBe(false);
    expect(r.detail).not.toContain('ghp_x1234567890');
  });

  it('anthropic / openai: probe the models endpoint with the key in a header, and map statuses', async () => {
    const f = reply(200);
    const t = createConnectionTester({ fetch: f as unknown as typeof fetch });
    expect(await t.anthropic({ apiKey: KEY })).toEqual({ target: 'anthropic', ok: true, detail: 'Anthropic accepted the key' });
    const [url, init] = f.mock.calls[0] as unknown as [string, { headers: Record<string, string> }];
    expect(url).toMatch(/^https:\/\/api\.anthropic\.com\/v1\/models/);
    expect(init.headers['x-api-key']).toBe(KEY);
    expect(url).not.toContain(KEY);

    await t.openai({ apiKey: 'sk-proj-abcdefghij' });
    const [ourl, oinit] = f.mock.calls[1] as unknown as [string, { headers: Record<string, string> }];
    expect(ourl).toBe('https://api.openai.com/v1/models');
    expect(oinit.headers['authorization']).toBe('Bearer sk-proj-abcdefghij');

    expect(await createConnectionTester({ fetch: reply(401) as unknown as typeof fetch }).anthropic({ apiKey: KEY })).toMatchObject({ ok: false, detail: 'Anthropic rejected the key (401)' });
    expect(await createConnectionTester({ fetch: reply(500) as unknown as typeof fetch }).openai({ apiKey: 'sk-proj-abcdefghij' })).toMatchObject({ ok: false, detail: 'OpenAI answered 500' });
    expect(await createConnectionTester({ fetch: reply(429) as unknown as typeof fetch }).openai({ apiKey: 'sk-proj-abcdefghij' })).toMatchObject({ ok: true });
  });

  it('network errors are reported without the key', async () => {
    const f = vi.fn(async () => { throw new Error(`getaddrinfo ENOTFOUND api.anthropic.com (key ${KEY})`); });
    const r = await createConnectionTester({ fetch: f as unknown as typeof fetch }).anthropic({ apiKey: KEY });
    expect(r.ok).toBe(false);
    expect(r.detail).toContain('ENOTFOUND');
    expect(r.detail).not.toContain(KEY);
  });

  it('tofu: finds the module and an OpenTofu binary', async () => {
    const runner = (stdout: string, exitCode = 0): ProcessRunner => ({ run: vi.fn(async () => ({ exitCode, stdout, stderr: '' })) });
    const t = createConnectionTester({ processRunner: runner('OpenTofu v1.8.2\non linux_amd64\n'), exists: async () => undefined });
    expect(await t.tofu({ binary: 'tofu', moduleDir: '/m/infra/aws' })).toEqual({ target: 'tofu', ok: true, detail: 'OpenTofu 1.8.2, module /m/infra/aws' });

    const noModule = createConnectionTester({ processRunner: runner('OpenTofu v1.8.2'), exists: async () => { throw new Error('ENOENT'); } });
    expect(await noModule.tofu({ binary: 'tofu', moduleDir: '/nope' })).toMatchObject({ ok: false, detail: expect.stringContaining('main.tf not found') });

    const terraform = createConnectionTester({ processRunner: runner('Terraform v1.5.7'), exists: async () => undefined });
    expect(await terraform.tofu({ binary: 'terraform', moduleDir: '/m' })).toMatchObject({ ok: false, detail: expect.stringContaining('not OpenTofu') });

    const missing = createConnectionTester({ processRunner: { run: async () => { throw new Error('spawn tofu ENOENT'); } }, exists: async () => undefined });
    expect(await missing.tofu({ binary: 'tofu', moduleDir: '/m' })).toMatchObject({ ok: false, detail: expect.stringContaining('cannot run tofu') });
  });
});
