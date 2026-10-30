import { resolve } from 'node:path';
import type { HarnessInfo } from '@ratchet/agents';
import { openLedger, type EngineApi, type Ledger } from '@ratchet/core';
import { cleanup, configure } from '@testing-library/react';
import { afterEach } from 'vitest';
import type { AwsProfileList } from '../main/aws-profiles';
import type { AppEngine } from '../main/dev-app';
import { createDevEngine } from '../main/dev-app';
import type { GithubApi } from '../main/github';
import { HarnessCache } from '../main/harnesses';
import { createService, type ConnectionTesterLike, type RatchetService, type ServiceDeps } from '../main/service';
import { SettingsStore, type SecretCipher } from '../main/settings';
import { FakeGithub, claude, codex } from '../main/test-support';
import type { RatchetApi } from '../shared/types';

// findBy* / waitFor: the real service under a parallel test run can take more than the default 1 s.
configure({ asyncUtilTimeout: 8000 });

const cipher: SecretCipher = {
  isAvailable: () => true,
  encrypt: (s) => Buffer.from(`enc:${s}`),
  decrypt: (b) => b.toString().slice(4),
};

export interface Harness {
  service: RatchetService;
  ledger: Ledger;
  settings: SettingsStore;
  github: FakeGithub;
  opened: string[];
}

export interface InstallOptions {
  encryption?: boolean;
  connectionTester?: ConnectionTesterLike;
  settingsEnv?: NodeJS.ProcessEnv;
  /** Engine methods the dev stub does not have (templates, sandbox activity, receipts, ...). */
  engine?: Partial<EngineApi>;
  /** AppEngine extras (`createDemo`, `missingFor`). */
  app?: Partial<AppEngine>;
  github?: GithubApi;
  /** The detected agent CLIs. Default: Claude Code signed in, Codex not installed. */
  harnesses?: HarnessInfo[];
  awsProfiles?: AwsProfileList;
  deps?: Partial<ServiceDeps>;
}

let live: Harness[] = [];

/**
 * Install `window.ratchet` backed by the real service over a seeded in-memory ledger (dev engine, timers parked),
 * so renderer tests run against exactly the view models the main process produces. GitHub, the AWS profiles and the
 * agent CLIs are fakes: nothing touches the network or the machine.
 */
export function installApi(opts: InstallOptions = {}): Harness {
  const ledger = openLedger(':memory:', { migrationsFolder: resolve(__dirname, '../../../core/src/ledger/migrations') });
  const settings = new SettingsStore(null, { ...cipher, isAvailable: () => opts.encryption ?? true }, opts.settingsEnv ?? {});
  const github = (opts.github as FakeGithub | undefined) ?? new FakeGithub();
  const opened: string[] = [];
  const service = createService({
    ledger,
    settings,
    github,
    openExternal: (u) => opened.push(u),
    sleep: async () => undefined,
    listAwsProfiles: () => opts.awsProfiles ?? { profiles: [], ssoSessions: [], filesFound: false },
    harnesses: new HarnessCache({ loginPath: async () => '/bin', detect: async () => opts.harnesses ?? [claude(), codex()] }),
    ...(opts.connectionTester ? { connectionTester: opts.connectionTester } : {}),
    createApp: () => {
      const app = createDevEngine(ledger, 'simulation', { tickMs: 3_600_000 });
      const engine = Object.assign(Object.create(app.engine) as EngineApi, opts.engine ?? {});
      return { ...app, engine, dispose: () => app.dispose(), ...opts.app };
    },
    ...opts.deps,
  });
  window.ratchet = { ...service } as RatchetApi;
  const h = { service, ledger, settings, github: github as FakeGithub, opened };
  live.push(h);
  return h;
}

afterEach(() => {
  cleanup();
  for (const h of live) h.service.dispose();
  live = [];
});
