// Launch the built desktop app under Playwright. Playwright is installed globally (NODE_PATH), so it is loaded
// through createRequire instead of an ESM `import`.
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
export const { _electron, chromium } = require('playwright');

const here = dirname(fileURLToPath(import.meta.url));
export const E2E_ROOT = join(here, '..');
export const REPO_ROOT = join(here, '..', '..', '..');
export const DESKTOP_ROOT = join(E2E_ROOT, '..', 'desktop');
export const ARTIFACTS = join(E2E_ROOT, 'artifacts');
export const MAIN_ENTRY = process.env.RATCHET_MAIN ?? join(DESKTOP_ROOT, 'out', 'main', 'index.js');
export const VIEWPORT = { width: 1440, height: 1000 };

/** The electron binary shipped in packages/desktop's node_modules (Playwright only looks in the cwd). */
const electronPath = () => createRequire(join(DESKTOP_ROOT, 'package.json'))('electron');

/**
 * Launch the app with a fresh, isolated userData dir (so the ledger DB and settings are new for every run) and
 * resolve the first window at 1440x1000. Console errors and uncaught page errors are collected in `errors`.
 */
export async function launchApp({ width = VIEWPORT.width, height = VIEWPORT.height, engine = 'real', env: extraEnv = {} } = {}) {
  const userDataDir = mkdtempSync(join(tmpdir(), 'ratchet-e2e-'));
  const saveDir = join(userDataDir, 'exports');
  const env = {
    ...process.env,
    RATCHET_USER_DATA_DIR: userDataDir,
    RATCHET_E2E_SAVE_DIR: saveDir,
    RATCHET_REPO_ROOT: REPO_ROOT,
    ELECTRON_DISABLE_SECURITY_WARNINGS: '1',
  };
  // Nothing of the machine running the tests leaks in: no credentials, no ~/.aws, no GitHub.
  for (const k of ['GITHUB_TOKEN', 'GH_TOKEN', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_SESSION_TOKEN', 'AWS_PROFILE', 'RATCHET_GITHUB_API_URL', 'RATCHET_GITHUB_WEB_URL', 'RATCHET_GITHUB_CLIENT_ID']) delete env[k];
  env.AWS_CONFIG_FILE = join(userDataDir, 'no-aws', 'config');
  env.AWS_SHARED_CREDENTIALS_FILE = join(userDataDir, 'no-aws', 'credentials');
  Object.assign(env, extraEnv);
  // `real` = the product engine: demos in simulation, repositories on the hybrid runtime (default). `dev` = the seeded stub, for design screenshots only.
  // Design tools compare screens against the design: the dev agents tick once an hour so the seeded ledger holds still.
  if (engine === 'dev') {
    env.RATCHET_ENGINE = 'dev';
    env.RATCHET_DEV_TICK_MS ??= '3600000';
  } else delete env.RATCHET_ENGINE;
  const app = await _electron.launch({
    executablePath: electronPath(),
    args: [MAIN_ENTRY, '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage'],
    env,
  });
  const win = await app.firstWindow();
  const errors = [];
  const consoleLog = [];
  win.on('console', (m) => {
    consoleLog.push(`${m.type()}: ${m.text()}`);
    if (m.type() === 'error') errors.push(`console.error: ${m.text()}`);
  });
  win.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  await setContentSize(app, width, height);
  await win.waitForSelector('.sidebar', { timeout: 30_000 });
  return {
    app,
    win,
    errors,
    consoleLog,
    userDataDir,
    saveDir,
    async close() {
      await app.close().catch(() => undefined);
      rmSync(userDataDir, { recursive: true, force: true });
    },
  };
}

export async function setContentSize(app, width, height) {
  await app.evaluate(({ BrowserWindow }, [w, h]) => {
    const win = BrowserWindow.getAllWindows()[0];
    win.setMinimumSize(1, 1);
    win.setContentSize(w, h);
  }, [width, height]);
}

/**
 * Full-page screenshot of the current screen. The app scrolls inside `.content` (not the document), so grow the
 * window to the content height, capture, and restore the 1440x1000 window.
 */
export async function screenshotScreen(app, win, name, dir = join(ARTIFACTS, 'app')) {
  mkdirSync(dir, { recursive: true });
  const path = join(dir, `${name}.png`);
  await win.evaluate(() => document.fonts.ready);
  // Grow the window until `.content` no longer scrolls (growing can reflow charts and change the height).
  let target = VIEWPORT.height;
  for (let i = 0; i < 4; i++) {
    const need = await win.evaluate(() => {
      const c = document.querySelector('.content');
      return c ? c.scrollHeight : document.documentElement.scrollHeight;
    });
    const next = Math.min(Math.max(VIEWPORT.height, need), 5000);
    if (next <= target && i > 0) break;
    target = Math.max(target, next);
    await setContentSize(app, VIEWPORT.width, target);
    await win.waitForTimeout(250);
  }
  await win.screenshot({ path });
  await setContentSize(app, VIEWPORT.width, VIEWPORT.height);
  await win.waitForTimeout(100);
  return path;
}

/** Sidebar navigation by visible label. */
export async function goTo(win, label) {
  await win.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: new RegExp(`^${label}( \\d+| \\d+ live)?$`) }).click();
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Screens in sidebar order: screenshot name, sidebar label, route id (`main[data-screen]`). */
export const SCREENS = [
  { name: 'optimizations', nav: 'Optimizations', route: 'optimizations' },
  { name: 'benchmarks', nav: 'Benchmarks', route: 'benchmarks' },
  { name: 'live-agents', nav: 'Agents', route: 'agents' },
  { name: 'sandbox', nav: 'Environments', route: 'sandbox' },
  { name: 'result', nav: 'Result', route: 'result' },
  { name: 'settings', nav: 'Settings', route: 'settings' },
];

/** Wait until the main column shows the given route and its first heading has rendered. */
export async function waitForScreen(win, route) {
  await win.locator(`main[data-screen="${route}"] h1`).first().waitFor({ timeout: 15_000 });
}

/** Open the "New optimization" screen from the Optimizations header button. */
export async function openNewOptimization(win) {
  await goTo(win, 'Optimizations');
  await win.getByRole('button', { name: /New optimization/ }).click();
  await waitForScreen(win, 'new');
}

// ---------------------------------------------------------------- polling + helpers shared by the specs

/** Poll `fn` until it returns a truthy value (returned) or `timeout` ms pass (throws with `what` and the last error). */
export async function poll(what, fn, { timeout = 60_000, interval = 500 } = {}) {
  const end = Date.now() + timeout;
  let last;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
      last = undefined;
    } catch (e) {
      last = e;
    }
    if (Date.now() > end) throw new Error(`timed out after ${Math.round(timeout / 1000)}s waiting for ${what}${last ? `: ${last.message}` : ''}`);
    await sleep(interval);
  }
}

/** Call the preload API from the page, e.g. `rpc(win, 'getOptimization', id)`. Used to read the truth the UI must agree with. */
export const rpc = (win, method, ...args) =>
  win.evaluate(([m, a]) => window.ratchet[m](...a), [method, args]);

/** The documented renderer API: every method of `RatchetApi` in shared/types.ts, read from the source so it cannot drift. */
export function documentedApi() {
  const src = readFileSync(join(DESKTOP_ROOT, 'src', 'shared', 'types.ts'), 'utf8');
  const body = src.slice(src.indexOf('export interface RatchetApi'));
  return [...body.matchAll(/^ {2}(\w+)\(/gm)].map((m) => m[1]).sort();
}

/** Never fail on a transient "Loading…": wait until the main column has settled. */
export async function waitSettled(win, timeout = 20_000) {
  await poll('screen to finish loading', () => win.evaluate(() => !document.querySelector('main')?.textContent?.includes('Loading…')), { timeout });
}

/** Same number formatting as the app (renderer/format.ts `fmtNum`). */
export function fmtNum(v) {
  if (Math.abs(v) >= 100) return v.toFixed(0);
  if (Math.abs(v) >= 10) return Number.isInteger(v) ? String(v) : v.toFixed(1);
  return Number.isInteger(v) ? String(v) : v.toFixed(2);
}
