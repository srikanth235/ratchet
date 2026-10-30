import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openLedger } from '@ratchet/core';
import { describe, expect, it } from 'vitest';
import { createEngineForApp } from './engine.js';
import { createService } from './service.js';
import { SettingsStore, type SecretCipher } from './settings.js';

const rot: SecretCipher = {
  isAvailable: () => true,
  encrypt: (s) => Buffer.from([...Buffer.from(s)].map((b) => b ^ 0x5a)),
  decrypt: (b) => Buffer.from([...b].map((x) => x ^ 0x5a)).toString(),
};

describe('SettingsStore secrets', () => {
  it('stores secrets encrypted, round-trips them in main, and persists across instances', () => {
    const file = join(mkdtempSync(join(tmpdir(), 'rt-')), 'settings.json');
    const s = new SettingsStore(file, rot, {});
    s.setSecret('anthropicApiKey', 'sk-ant-SUPERSECRET');
    expect(readFileSync(file, 'utf8')).not.toContain('SUPERSECRET');
    expect(s.getSecret('anthropicApiKey')).toBe('sk-ant-SUPERSECRET');
    const again = new SettingsStore(file, rot, {});
    expect(again.hasSecret('anthropicApiKey')).toBe(true);
    expect(again.getSecret('anthropicApiKey')).toBe('sk-ant-SUPERSECRET');
    again.clearSecret('anthropicApiKey');
    expect(again.hasSecret('anthropicApiKey')).toBe(false);
  });

  it('refuses to store secrets when the keychain is unavailable', () => {
    const s = new SettingsStore(null, { ...rot, isAvailable: () => false }, {});
    expect(() => s.setSecret('githubToken', 'ghp_123456789')).toThrow(/unavailable/);
  });

  it('treats environment-provided credentials as configured', () => {
    const s = new SettingsStore(null, rot, { GH_TOKEN: 'ghp_x' });
    expect(s.hasSecret('githubToken')).toBe(true);
    expect(s.hasSecret('openaiApiKey')).toBe(false);
  });

  it('survives a corrupt settings file', () => {
    const dir = mkdtempSync(join(tmpdir(), 'rt-'));
    const file = join(dir, 'settings.json');
    require('node:fs').writeFileSync(file, '{not json');
    expect(new SettingsStore(file, rot, {}).mode).toBe('simulation');
  });
});

describe('secrets never cross to the renderer', () => {
  it('getSettings exposes only configured yes/no, and no response contains the secret value', async () => {
    const ledger = openLedger(':memory:');
    const settings = new SettingsStore(null, rot, {});
    const service = createService({ ledger, settings, createApp: (m) => createEngineForApp(ledger, m, { tickMs: 600_000 }) });
    const secret = 'sk-ant-TOPSECRET-VALUE';
    const afterSet = await service.setSecret('anthropicApiKey', secret);
    expect(afterSet.secrets).toEqual({ anthropicApiKey: true, openaiApiKey: false, githubToken: false });
    const everything = JSON.stringify([
      afterSet, await service.getSettings(), await service.listOptimizations(), await service.sandboxInfo(),
      await service.preflight({ agents: ['claude-code'], configYaml: '' }), await service.setAwsProfile('p'),
    ]);
    expect(everything).not.toContain('TOPSECRET');
    expect((await service.clearSecret('anthropicApiKey')).secrets.anthropicApiKey).toBe(false);
    service.dispose();
  });
});
