import type { RepoVM } from '../shared/types.js';

/**
 * GitHub for the signed-in account: the OAuth device flow, the account's repositories and a file on a branch.
 * Plain `fetch` against the REST API, so tests inject a fake and never touch the network. A token only ever travels in
 * the Authorization header and never appears in a thrown message.
 */

/** Placeholder OAuth app client id. Register an OAuth app with the device flow enabled and set RATCHET_GITHUB_CLIENT_ID. */
export const PLACEHOLDER_GITHUB_CLIENT_ID = 'Ov23liRatchetPlaceholder';
/** `repo` lets Ratchet push candidate branches and open pull requests (including on private repositories). */
export const GITHUB_SCOPE = 'repo';

export interface DeviceCode {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  /** Seconds until the code expires. */
  expiresIn: number;
  /** Minimum seconds between polls. */
  interval: number;
}

export type DevicePoll =
  | { status: 'ok'; token: string }
  | { status: 'pending' }
  | { status: 'slow_down'; interval: number }
  | { status: 'expired' }
  | { status: 'denied' }
  | { status: 'error'; message: string };

export interface GithubApi {
  /** True when a real client id is configured (not the placeholder). */
  readonly clientIdConfigured: boolean;
  deviceStart(): Promise<DeviceCode>;
  devicePoll(deviceCode: string): Promise<DevicePoll>;
  user(token: string): Promise<{ login: string }>;
  /** The account's repositories, most recently pushed first, filtered by `query` (substring of owner/name). */
  repos(token: string, query?: string): Promise<RepoVM[]>;
  repo(token: string, fullName: string): Promise<RepoVM>;
  /** Text of `path` at `ref`, or null when the file does not exist. */
  file(token: string, fullName: string, ref: string, path: string): Promise<string | null>;
}

export interface GithubApiOptions {
  clientId?: string | undefined;
  fetch?: typeof fetch;
  /** REST API base (GitHub Enterprise). Default https://api.github.com */
  apiUrl?: string;
  /** Web base for the device flow. Default https://github.com */
  webUrl?: string;
}

interface ApiRepo {
  full_name: string;
  default_branch: string;
  private: boolean;
  description: string | null;
  pushed_at: string | null;
}

const toRepo = (r: ApiRepo): RepoVM => ({ fullName: r.full_name, defaultBranch: r.default_branch, private: r.private, description: r.description, pushedAt: r.pushed_at });

export function createGithubApi(opts: GithubApiOptions = {}): GithubApi {
  const doFetch = opts.fetch ?? fetch;
  const api = (opts.apiUrl ?? 'https://api.github.com').replace(/\/$/, '');
  const web = (opts.webUrl ?? 'https://github.com').replace(/\/$/, '');
  const clientId = opts.clientId?.trim() || PLACEHOLDER_GITHUB_CLIENT_ID;
  const clientIdConfigured = clientId !== PLACEHOLDER_GITHUB_CLIENT_ID;

  const headers = (token: string): Record<string, string> => ({
    Authorization: `Bearer ${token}`,
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
    'User-Agent': 'Ratchet',
  });

  async function get(token: string, path: string): Promise<Response> {
    let res: Response;
    try {
      res = await doFetch(`${api}${path}`, { headers: headers(token), signal: AbortSignal.timeout(20_000) });
    } catch (err) {
      throw new Error(`Could not reach GitHub: ${scrub((err as Error).message, token)}`);
    }
    return res;
  }

  function fail(res: Response, what: string): never {
    if (res.status === 401) throw new Error('GitHub rejected the sign-in (401): sign in again in Settings.');
    if (res.status === 403) throw new Error(`GitHub refused ${what} (403): the token may lack access, or the rate limit was hit.`);
    if (res.status === 404) throw new Error(`${what}: not found, or the account cannot see it (404).`);
    throw new Error(`GitHub ${what} failed (${res.status}).`);
  }

  async function form(url: string, body: Record<string, string>): Promise<Record<string, unknown>> {
    let res: Response;
    try {
      res = await doFetch(url, {
        method: 'POST',
        headers: { Accept: 'application/json', 'Content-Type': 'application/x-www-form-urlencoded', 'User-Agent': 'Ratchet' },
        body: new URLSearchParams(body).toString(),
        signal: AbortSignal.timeout(20_000),
      });
    } catch (err) {
      throw new Error(`Could not reach GitHub: ${(err as Error).message}`);
    }
    if (!res.ok && res.status !== 400) throw new Error(`GitHub answered ${res.status}${res.status === 404 ? ' (is the OAuth client id right? set RATCHET_GITHUB_CLIENT_ID)' : ''}.`);
    return (await res.json()) as Record<string, unknown>;
  }

  return {
    clientIdConfigured,
    async deviceStart() {
      const j = await form(`${web}/login/device/code`, { client_id: clientId, scope: GITHUB_SCOPE });
      if (typeof j['error'] === 'string') throw new Error(`GitHub: ${String(j['error_description'] ?? j['error'])}`);
      return {
        deviceCode: String(j['device_code']),
        userCode: String(j['user_code']),
        verificationUri: String(j['verification_uri'] ?? `${web}/login/device`),
        expiresIn: Number(j['expires_in'] ?? 900),
        interval: Number(j['interval'] ?? 5),
      };
    },
    async devicePoll(deviceCode) {
      const j = await form(`${web}/login/oauth/access_token`, {
        client_id: clientId,
        device_code: deviceCode,
        grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
      });
      if (typeof j['access_token'] === 'string') return { status: 'ok', token: j['access_token'] };
      switch (j['error']) {
        case 'authorization_pending':
          return { status: 'pending' };
        case 'slow_down':
          return { status: 'slow_down', interval: Number(j['interval'] ?? 10) };
        case 'expired_token':
          return { status: 'expired' };
        case 'access_denied':
          return { status: 'denied' };
        default:
          return { status: 'error', message: String(j['error_description'] ?? j['error'] ?? 'unexpected answer') };
      }
    },
    async user(token) {
      const res = await get(token, '/user');
      if (!res.ok) fail(res, 'reading the account');
      return { login: String(((await res.json()) as { login: string }).login) };
    },
    async repos(token, query) {
      const out: RepoVM[] = [];
      for (let page = 1; page <= 3; page++) {
        const res = await get(token, `/user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member&page=${page}`);
        if (!res.ok) fail(res, 'listing repositories');
        const rows = (await res.json()) as ApiRepo[];
        out.push(...rows.map(toRepo));
        if (rows.length < 100) break;
      }
      const q = query?.trim().toLowerCase();
      return q ? out.filter((r) => r.fullName.toLowerCase().includes(q) || (r.description ?? '').toLowerCase().includes(q)) : out;
    },
    async repo(token, fullName) {
      const res = await get(token, `/repos/${fullName}`);
      if (!res.ok) fail(res, `reading ${fullName}`);
      return toRepo((await res.json()) as ApiRepo);
    },
    async file(token, fullName, ref, path) {
      const res = await get(token, `/repos/${fullName}/contents/${path.split('/').map(encodeURIComponent).join('/')}?ref=${encodeURIComponent(ref)}`);
      if (res.status === 404) return null;
      if (!res.ok) fail(res, `reading ${path}`);
      const j = (await res.json()) as { content?: string; encoding?: string };
      if (typeof j.content !== 'string') return null;
      return j.encoding === 'base64' ? Buffer.from(j.content, 'base64').toString('utf8') : j.content;
    },
  };
}

function scrub(text: string, secret: string): string {
  return secret.length >= 4 ? text.split(secret).join('***') : text;
}
