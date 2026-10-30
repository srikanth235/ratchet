// Live-mode settings and their validation. Pure (no zod, no Node): the main process validates every save with it,
// and the renderer uses the same functions to show format problems while typing. Secrets (API keys, the GitHub
// token) are NOT here: they have their own channel and never come back to the renderer.

/** `profile` = a named profile with static keys or an assume-role; `sso` = a profile created by `aws configure sso`. */
export type AwsAuthKind = 'profile' | 'sso';

/** Everything the renderer may read and write for live mode (no secrets). Empty string = not set / use the default. */
export interface LiveSettings {
  awsProfile: string;
  awsRegion: string;
  awsAuth: AwsAuthKind;
  /** AMI that agent runners boot from. */
  runnerImageRef: string;
  /** AMI that bench / confirm instances boot from. */
  benchImageRef: string;
  /** Path to the `tofu` binary; empty = the bundled one, else `tofu` on PATH. */
  tofuBinary: string;
  /** Directory of the OpenTofu module; empty = the bundled infra/aws. */
  moduleDir: string;
}

export type LiveSettingsInput = Partial<LiveSettings>;
export type LiveSettingKey = keyof LiveSettings;

export const DEFAULT_AWS_REGION = 'us-east-1';

export const LIVE_SETTING_DEFAULTS: LiveSettings = {
  awsProfile: '',
  awsRegion: DEFAULT_AWS_REGION,
  awsAuth: 'profile',
  runnerImageRef: '',
  benchImageRef: '',
  tofuBinary: '',
  moduleDir: '',
};

const PROFILE_RE = /^[A-Za-z0-9_.@:-]*$/;
const REGION_RE = /^[a-z]{2}(?:-[a-z]+)+-\d$/;
const AMI_RE = /^ami-(?:[0-9a-f]{8}|[0-9a-f]{17})$/;
// A path or a bare command name: no shell metacharacters, no control characters, no leading dash (would read as a flag).
const PATHLIKE_RE = /^[^\0\r\n;&|`$<>*?"'\\]+$/;
const isAbsolute = (p: string): boolean => p.startsWith('/') || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith('~/');

export const SETTING_LABELS: Record<LiveSettingKey, string> = {
  awsProfile: 'AWS profile',
  awsRegion: 'AWS region',
  awsAuth: 'AWS sign-in',
  runnerImageRef: 'Runner AMI',
  benchImageRef: 'Bench AMI',
  tofuBinary: 'OpenTofu binary',
  moduleDir: 'OpenTofu module directory',
};

/** `null` = valid. Empty values are valid (they mean "not set"); readiness reports what is still missing. */
export function validateLiveSetting(key: LiveSettingKey, raw: unknown): string | null {
  if (typeof raw !== 'string') return `${SETTING_LABELS[key]} must be text`;
  const v = raw.trim();
  switch (key) {
    case 'awsProfile':
      if (v.length > 120) return 'Profile names are at most 120 characters';
      return PROFILE_RE.test(v) ? null : 'Profile names may only contain letters, digits and _ . @ : -';
    case 'awsRegion':
      return v === '' || REGION_RE.test(v) ? null : 'Use a region id such as us-east-1 or eu-west-2';
    case 'awsAuth':
      return v === 'profile' || v === 'sso' ? null : 'Choose "profile" or "sso"';
    case 'runnerImageRef':
    case 'benchImageRef':
      return v === '' || AMI_RE.test(v) ? null : 'Use an AMI id such as ami-0123456789abcdef0';
    case 'tofuBinary':
      if (v === '') return null;
      if (v.length > 500 || !PATHLIKE_RE.test(v) || v.startsWith('-')) return 'Use a path or command name without spaces-as-arguments, quotes or shell characters';
      return /\s/.test(v) && !isAbsolute(v) ? 'Use an absolute path (or just "tofu" for the one on PATH)' : null;
    case 'moduleDir':
      if (v === '') return null;
      if (v.length > 500 || !PATHLIKE_RE.test(v) || v.startsWith('-')) return 'Use a directory path without quotes or shell characters';
      return isAbsolute(v) ? null : 'Use an absolute path';
  }
}

/** Per-field errors for every key present in `input` (empty object = all valid). */
export function validateLiveSettings(input: LiveSettingsInput): Partial<Record<LiveSettingKey, string>> {
  const errors: Partial<Record<LiveSettingKey, string>> = {};
  for (const key of Object.keys(input) as LiveSettingKey[]) {
    if (!(key in LIVE_SETTING_DEFAULTS)) {
      errors[key] = 'Unknown setting';
      continue;
    }
    const e = validateLiveSetting(key, input[key]);
    if (e) errors[key] = e;
  }
  return errors;
}

// ------------------------------------------------------------------ secret formats (checked before anything is stored)

export type SecretKey = 'anthropicApiKey' | 'openaiApiKey' | 'githubToken';

const GITHUB_TOKEN_RE = /^(?:gh[pousr]_[A-Za-z0-9]{20,255}|github_pat_[A-Za-z0-9_]{20,255}|[0-9a-f]{40})$/;

/** Loose prefix checks that catch the common paste mistakes (wrong key in the wrong box, stray whitespace). */
export function validateSecretFormat(name: SecretKey, value: string): string | null {
  if (value !== value.trim() || /\s/.test(value)) return 'Remove spaces and line breaks around the value';
  switch (name) {
    case 'anthropicApiKey':
      return /^sk-ant-[A-Za-z0-9_-]{8,}$/.test(value) ? null : 'Anthropic keys start with sk-ant-';
    case 'openaiApiKey':
      return /^sk-(?!ant-)[A-Za-z0-9_-]{16,}$/.test(value) ? null : 'OpenAI keys start with sk- (not sk-ant-) and are at least 20 characters';
    case 'githubToken':
      return GITHUB_TOKEN_RE.test(value) ? null : 'GitHub tokens start with ghp_, gho_, ghu_, ghs_ or github_pat_';
  }
}
