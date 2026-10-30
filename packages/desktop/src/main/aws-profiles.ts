import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { AwsProfileVM } from '../shared/types.js';

/**
 * The AWS profiles and SSO sessions on this machine, parsed from ~/.aws/config and ~/.aws/credentials.
 * Names and kinds only: no key, token or account id is read into the result (the parser looks at key NAMES
 * and, for `sso_session`, the session name), so nothing here can leak a credential.
 */

export interface AwsProfileList {
  profiles: AwsProfileVM[];
  ssoSessions: string[];
  /** At least one of the files exists. */
  filesFound: boolean;
}

export interface AwsFiles {
  config: string | null;
  credentials: string | null;
}

interface Section {
  header: string;
  keys: Map<string, string>;
}

/** `[header]` sections with `key = value` lines; `#` and `;` comments; indented continuation lines are ignored. */
function sections(text: string): Section[] {
  const out: Section[] = [];
  let cur: Section | null = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === '' || line.startsWith('#') || line.startsWith(';')) continue;
    const h = /^\[\s*(.*?)\s*\]$/.exec(line);
    if (h) {
      cur = { header: h[1]!, keys: new Map() };
      out.push(cur);
      continue;
    }
    if (!cur || /^\s/.test(raw)) continue; // sub-settings of a nested block, or a line before any section
    const eq = line.indexOf('=');
    if (eq > 0) cur.keys.set(line.slice(0, eq).trim().toLowerCase(), line.slice(eq + 1).trim());
  }
  return out;
}

const NAME_RE = /^[A-Za-z0-9_.@:-]{1,120}$/;

export function parseAwsFiles(files: AwsFiles): AwsProfileList {
  const byName = new Map<string, AwsProfileVM>();
  const ssoSessions: string[] = [];

  for (const s of sections(files.config ?? '')) {
    const sess = /^sso-session\s+(.+)$/.exec(s.header);
    if (sess) {
      if (NAME_RE.test(sess[1]!)) ssoSessions.push(sess[1]!);
      continue;
    }
    const prof = s.header === 'default' ? 'default' : /^profile\s+(.+)$/.exec(s.header)?.[1];
    if (!prof || !NAME_RE.test(prof)) continue;
    const ssoSession = s.keys.get('sso_session') ?? null;
    const kind: AwsProfileVM['kind'] =
      ssoSession || s.keys.has('sso_start_url') ? 'sso' : s.keys.has('role_arn') ? 'role' : s.keys.has('credential_process') ? 'process' : s.keys.has('aws_access_key_id') ? 'keys' : 'other';
    byName.set(prof, { name: prof, kind, source: 'config', ssoSession });
  }

  for (const s of sections(files.credentials ?? '')) {
    if (!NAME_RE.test(s.header)) continue;
    const existing = byName.get(s.header);
    if (existing) {
      // Static keys in the credentials file win over an otherwise empty config section.
      if (existing.kind === 'other' && s.keys.has('aws_access_key_id')) byName.set(s.header, { ...existing, kind: 'keys' });
      continue;
    }
    byName.set(s.header, { name: s.header, kind: s.keys.has('aws_access_key_id') ? 'keys' : 'other', source: 'credentials', ssoSession: null });
  }

  const profiles = [...byName.values()].sort((a, b) => (a.name === 'default' ? -1 : b.name === 'default' ? 1 : a.name.localeCompare(b.name)));
  return { profiles, ssoSessions: [...new Set(ssoSessions)].sort(), filesFound: files.config !== null || files.credentials !== null };
}

export interface ListAwsProfilesOptions {
  env?: Record<string, string | undefined>;
  home?: string;
  readFile?: (path: string) => string;
}

/** Read the two files (honouring AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE) and parse them. */
export function listAwsProfiles(opts: ListAwsProfilesOptions = {}): AwsProfileList {
  const env = opts.env ?? process.env;
  const home = opts.home ?? homedir();
  const read = opts.readFile ?? ((p: string) => readFileSync(p, 'utf8'));
  const tryRead = (p: string): string | null => {
    try {
      return read(p);
    } catch {
      return null;
    }
  };
  return parseAwsFiles({
    config: tryRead(env['AWS_CONFIG_FILE'] || join(home, '.aws', 'config')),
    credentials: tryRead(env['AWS_SHARED_CREDENTIALS_FILE'] || join(home, '.aws', 'credentials')),
  });
}
