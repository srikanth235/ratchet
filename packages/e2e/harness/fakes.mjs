// Fakes for what Settings and New optimization talk to, so the end-to-end specs need no network, no AWS and no installed CLIs:
//  - startFakeGithub(): an HTTP server speaking the slice of GitHub the app uses (device flow, /user, /user/repos, contents).
//    The app is pointed at it with RATCHET_GITHUB_API_URL / RATCHET_GITHUB_WEB_URL.
//  - fakeMachine(): a temp dir with fake `claude` / `codex` executables (put it first on PATH) and a ~/.aws fixture
//    (AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE).
//  - makeLocalRepo(): a real git repository with a GitHub origin and a ratchet.yaml, for "Local folder…".
import { execFileSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export const DEVICE_CODE = 'WDJB-MJHT';
export const FAKE_LOGIN = 'e2e-octocat';
export const FAKE_TOKEN = 'gho_E2EFAKETOKEN0123456789abcdefABCDEF';

/** `files`: `"owner/name@ref:path"` -> text. Repos listed by /user/repos come from `repos`. */
export async function startFakeGithub({ repos, files }) {
  const state = { approved: false, polls: 0, requests: [] };
  const server = createServer((req, res) => {
    const url = new URL(req.url, 'http://x');
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      state.requests.push(`${req.method} ${url.pathname}`);
      const json = (status, obj) => {
        res.writeHead(status, { 'content-type': 'application/json' });
        res.end(JSON.stringify(obj));
      };
      const bearer = /^Bearer (.+)$/.exec(req.headers.authorization ?? '')?.[1];
      if (req.method === 'POST' && url.pathname === '/login/device/code') {
        return json(200, { device_code: 'dc-e2e', user_code: DEVICE_CODE, verification_uri: `http://${req.headers.host}/login/device`, expires_in: 900, interval: 1 });
      }
      if (req.method === 'POST' && url.pathname === '/login/oauth/access_token') {
        state.polls++;
        return json(200, state.approved ? { access_token: FAKE_TOKEN, token_type: 'bearer', scope: 'repo' } : { error: 'authorization_pending' });
      }
      if (!bearer) return json(401, { message: 'Bad credentials' });
      if (url.pathname === '/user') return json(200, { login: FAKE_LOGIN });
      if (url.pathname === '/user/repos') return json(200, url.searchParams.get('page') === '1' ? repos : []);
      const repo = /^\/repos\/([^/]+\/[^/]+)$/.exec(url.pathname);
      if (repo) {
        const r = repos.find((x) => x.full_name === repo[1]);
        return r ? json(200, r) : json(404, { message: 'Not Found' });
      }
      const file = /^\/repos\/([^/]+\/[^/]+)\/contents\/(.+)$/.exec(url.pathname);
      if (file) {
        const text = files[`${file[1]}@${url.searchParams.get('ref')}:${decodeURIComponent(file[2])}`];
        return text === undefined ? json(404, { message: 'Not Found' }) : json(200, { encoding: 'base64', content: Buffer.from(text).toString('base64') });
      }
      return json(404, { message: 'Not Found' });
    });
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  return {
    env: { RATCHET_GITHUB_API_URL: base, RATCHET_GITHUB_WEB_URL: base, RATCHET_GITHUB_CLIENT_ID: 'e2e-client-id' },
    state,
    approve: () => (state.approved = true),
    close: () => new Promise((r) => server.close(r)),
  };
}

export const repo = (full_name, default_branch, description, priv = false) => ({ full_name, default_branch, private: priv, description, pushed_at: '2026-10-01T00:00:00Z' });

const AWS_CONFIG = `[default]
region = us-west-2

[profile corp-sso]
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Admin
region = eu-west-1

[profile deploy-role]
role_arn = arn:aws:iam::111122223333:role/Deploy
source_profile = default

[sso-session corp]
sso_start_url = https://corp.awsapps.com/start
sso_region = us-east-1
`;
export const AWS_SECRET = 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY';
const AWS_CREDENTIALS = `[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = ${AWS_SECRET}
`;

/** Fake agent CLIs on a private PATH dir and a ~/.aws fixture. Flip `codexSignedIn` and re-detect to see Re-detect work. */
export function fakeMachine() {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-e2e-machine-'));
  const bin = join(dir, 'bin');
  mkdirSync(bin);
  const script = (name, body) => {
    writeFileSync(join(bin, name), `#!/bin/sh\n${body}\n`);
    chmodSync(join(bin, name), 0o755);
  };
  script('claude', 'case "$1" in\n  --version) echo "2.1.4 (Claude Code)";;\n  auth) echo \'{"loggedIn": true, "authMethod": "claude.ai"}\';;\nesac');
  const codex = (signedIn) =>
    script('codex', `case "$1" in\n  --version) echo "codex-cli 0.9.0";;\n  login) ${signedIn ? 'echo "Logged in using ChatGPT"' : 'echo "Not logged in" >&2; exit 1'};;\nesac`);
  codex(false);
  mkdirSync(join(dir, 'aws'));
  writeFileSync(join(dir, 'aws', 'config'), AWS_CONFIG);
  writeFileSync(join(dir, 'aws', 'credentials'), AWS_CREDENTIALS);
  return {
    dir,
    env: {
      PATH: `${bin}:${process.env.PATH}`,
      AWS_CONFIG_FILE: join(dir, 'aws', 'config'),
      AWS_SHARED_CREDENTIALS_FILE: join(dir, 'aws', 'credentials'),
    },
    setCodexSignedIn: codex,
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}

/** A real repository on `branch` whose origin is a GitHub URL and that holds `ratchet.yaml`. */
export function makeLocalRepo({ remote, yaml, branch = 'main' }) {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-e2e-local-'));
  const git = (...args) => execFileSync('git', args, { cwd: dir, stdio: 'pipe', env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null' } });
  git('init', '-q', '-b', branch);
  if (remote) git('remote', 'add', 'origin', remote);
  if (yaml) writeFileSync(join(dir, 'ratchet.yaml'), yaml);
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}
