import { resolve } from 'node:path';
import { openLedger, type Ledger } from '@ratchet/core';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { createDevEngine as createEngineForApp } from '../main/dev-app';
import { createService, type ConnectionTesterLike, type RatchetService } from '../main/service';
import { SettingsStore, type SecretCipher } from '../main/settings';
import type { RatchetApi } from '../shared/types';

const cipher: SecretCipher = {
  isAvailable: () => true,
  encrypt: (s) => Buffer.from(`enc:${s}`),
  decrypt: (b) => b.toString().slice(4),
};

export interface Harness {
  service: RatchetService;
  ledger: Ledger;
  settings: SettingsStore;
}

let live: Harness[] = [];

/**
 * Install `window.ratchet` backed by the real service over a seeded in-memory ledger (dev engine, timers parked),
 * so renderer tests run against exactly the view models the main process produces.
 */
export function installApi(opts: { encryption?: boolean; connectionTester?: ConnectionTesterLike } = {}): Harness {
  const ledger = openLedger(':memory:', { migrationsFolder: resolve(__dirname, '../../../core/src/ledger/migrations') });
  const settings = new SettingsStore(null, { ...cipher, isAvailable: () => opts.encryption ?? true }, {});
  const service = createService({ ledger, settings, ...(opts.connectionTester ? { connectionTester: opts.connectionTester } : {}), createApp: (m) => createEngineForApp(ledger, m, { tickMs: 3_600_000 }) });
  window.ratchet = { ...service } as RatchetApi;
  const h = { service, ledger, settings };
  live.push(h);
  return h;
}

afterEach(() => {
  cleanup();
  for (const h of live) h.service.dispose();
  live = [];
});
