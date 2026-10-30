// Launch the built desktop app under Playwright. Playwright is installed globally (NODE_PATH), so it is loaded
// through createRequire instead of an ESM `import`.
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
export const { _electron, chromium } = require('playwright');

const here = dirname(fileURLToPath(import.meta.url));
export const E2E_ROOT = join(here, '..');
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
export async function launchApp({ width = VIEWPORT.width, height = VIEWPORT.height } = {}) {
  const userDataDir = mkdtempSync(join(tmpdir(), 'ratchet-e2e-'));
  const app = await _electron.launch({
    executablePath: electronPath(),
    args: [MAIN_ENTRY, '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage'],
    env: { ...process.env, RATCHET_USER_DATA_DIR: userDataDir, ELECTRON_DISABLE_SECURITY_WARNINGS: '1' },
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
  const height = await win.evaluate(() => {
    const c = document.querySelector('.content');
    return Math.max(c ? c.scrollHeight : 0, document.documentElement.scrollHeight);
  });
  await setContentSize(app, VIEWPORT.width, Math.max(VIEWPORT.height, Math.min(height, 5000)));
  await win.waitForTimeout(250);
  await win.screenshot({ path });
  await setContentSize(app, VIEWPORT.width, VIEWPORT.height);
  await win.waitForTimeout(100);
  return path;
}

/** Sidebar navigation by visible label. */
export async function goTo(win, label) {
  await win.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: label, exact: true }).click();
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Screens in sidebar order → screenshot file names. */
export const SCREENS = [
  { name: 'optimizations', nav: 'Optimizations', ready: 'Optimizations' },
  { name: 'live-agents', nav: 'Live agents', ready: 'Live agents' },
  { name: 'benchmarks', nav: 'Benchmark history', ready: 'Benchmark history' },
  { name: 'result', nav: 'Result', ready: 'Result' },
  { name: 'sandbox', nav: 'AWS sandbox', ready: 'AWS sandbox' },
  { name: 'settings', nav: 'Settings', ready: 'Settings' },
];

/** Open the "New optimization" screen from the Optimizations header button. */
export async function openNewOptimization(win) {
  await goTo(win, 'Optimizations');
  await win.getByRole('button', { name: /New optimization/ }).click();
  await win.getByRole('heading', { level: 1, name: 'New optimization' }).waitFor();
}
