import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { LIVE_SETTING_DEFAULTS, SETTING_LABELS, normalizeRepo, validateLiveSettings, type LiveSettings, type LiveSettingsInput } from '../shared/live-settings.js';
import type { Mode, SecretName } from '../shared/types.js';

/** Matches Electron's `safeStorage` (OS keychain: Keychain on macOS, DPAPI, libsecret). */
export interface SecretCipher {
  isAvailable(): boolean;
  encrypt(plain: string): Buffer;
  decrypt(data: Buffer): string;
}

interface Persisted {
  mode: Mode;
  awsProfile: string;
  /** Live-mode settings other than the profile and the secrets (those live in `awsProfile` and `secrets`). */
  live: Omit<LiveSettings, 'awsProfile'>;
  pullRequests: Record<string, string>;
  tornDown: Record<string, true>;
  /** base64 of the cipher text; plaintext never touches disk. */
  secrets: Partial<Record<SecretName, string>>;
}

const { awsProfile: _profile, ...LIVE_DEFAULTS } = LIVE_SETTING_DEFAULTS;
const DEFAULTS: Persisted = { mode: 'simulation', awsProfile: '', live: LIVE_DEFAULTS, pullRequests: {}, tornDown: {}, secrets: {} };

const ENV_FOR: Record<SecretName, string[]> = {
  anthropicApiKey: ['ANTHROPIC_API_KEY'],
  openaiApiKey: ['OPENAI_API_KEY'],
  githubToken: ['GITHUB_TOKEN', 'GH_TOKEN'],
};

/**
 * Small JSON-file store for app settings and the few facts the ledger does not hold (PR urls, teardown state),
 * plus secrets encrypted with the OS keychain. Secret values only leave via `getSecret` (main process only).
 */
export class SettingsStore {
  private data: Persisted;

  constructor(
    private readonly file: string | null,
    private readonly cipher: SecretCipher,
    private readonly env: NodeJS.ProcessEnv = process.env,
  ) {
    this.data = { ...DEFAULTS, live: { ...LIVE_DEFAULTS }, pullRequests: {}, tornDown: {}, secrets: {} };
    if (file && existsSync(file)) {
      try {
        const raw = JSON.parse(readFileSync(file, 'utf8')) as Partial<Persisted>;
        this.data = { ...this.data, ...raw, live: { ...LIVE_DEFAULTS, ...raw.live }, secrets: { ...raw.secrets }, pullRequests: { ...raw.pullRequests }, tornDown: { ...raw.tornDown } };
      } catch {
        /* corrupt settings file: start from defaults rather than crash the app */
      }
    }
  }

  get mode(): Mode {
    return this.data.mode;
  }
  get awsProfile(): string {
    return this.data.awsProfile;
  }
  /** Every live-mode setting that is not a secret. Safe to show in the renderer. */
  get live(): LiveSettings {
    return { ...this.data.live, awsProfile: this.data.awsProfile };
  }
  get encryptionAvailable(): boolean {
    return this.cipher.isAvailable();
  }

  setMode(mode: Mode): void {
    this.data.mode = mode;
    this.save();
  }
  setAwsProfile(profile: string): void {
    this.data.awsProfile = profile;
    this.save();
  }

  /** Validate and store a batch of live settings; nothing is saved when any field is invalid. */
  setLive(patch: LiveSettingsInput): void {
    const errors = validateLiveSettings(patch);
    const first = Object.entries(errors)[0];
    if (first) throw new Error(`${SETTING_LABELS[first[0] as keyof LiveSettings] ?? first[0]}: ${first[1]}`);
    const { awsProfile, ...rest } = patch;
    const next = { ...this.data.live };
    for (const [k, v] of Object.entries(rest) as [keyof typeof next, string][]) {
      (next as Record<string, string>)[k] = k === 'githubRepo' ? normalizeRepo(v) : v.trim();
    }
    this.data.live = next;
    if (awsProfile !== undefined) this.data.awsProfile = awsProfile.trim();
    this.save();
  }

  prUrl(id: string): string | null {
    return this.data.pullRequests[id] ?? null;
  }
  setPrUrl(id: string, url: string): void {
    this.data.pullRequests[id] = url;
    this.save();
  }
  tornDown(id: string): boolean {
    return this.data.tornDown[id] === true;
  }
  markTornDown(id: string): void {
    this.data.tornDown[id] = true;
    this.save();
  }

  setSecret(name: SecretName, value: string): void {
    if (!this.cipher.isAvailable()) throw new Error('OS keychain encryption is unavailable; refusing to store the secret in plaintext');
    this.data.secrets[name] = this.cipher.encrypt(value).toString('base64');
    this.save();
  }
  clearSecret(name: SecretName): void {
    delete this.data.secrets[name];
    this.save();
  }
  /** Configured = stored in the keychain, or provided through the environment. */
  hasSecret(name: SecretName): boolean {
    return this.data.secrets[name] !== undefined || ENV_FOR[name].some((k) => !!this.env[k]);
  }
  /** Main-process only: hand to the engine, never to the renderer. */
  getSecret(name: SecretName): string | null {
    const enc = this.data.secrets[name];
    if (enc !== undefined && this.cipher.isAvailable()) return this.cipher.decrypt(Buffer.from(enc, 'base64'));
    for (const k of ENV_FOR[name]) if (this.env[k]) return this.env[k]!;
    return null;
  }

  private save(): void {
    if (!this.file) return;
    mkdirSync(dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    writeFileSync(tmp, JSON.stringify(this.data, null, 2), { mode: 0o600 });
    renameSync(tmp, this.file);
  }
}
