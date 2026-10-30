// Boot: a clean first launch is quiet, locked down (CSP, context isolation) and exposes only the documented API.
import assert from 'node:assert/strict';
import { after, before, describe, test } from 'node:test';
import { documentedApi, launchApp, poll } from '../harness/launch.mjs';

describe('boot', () => {
  let h;
  before(async () => {
    h = await launchApp();
    await h.win.waitForTimeout(3000); // let the first queries and the sidebar polls run
  });
  after(() => h?.close());

  test('no console errors or uncaught page errors', () => {
    assert.deepEqual(h.errors, []);
  });

  test('window title and shell render', async () => {
    assert.equal(await h.win.title(), 'Ratchet');
    assert.equal(await h.win.locator('.sidebar .brand__name').innerText(), 'Ratchet');
    assert.ok(await h.win.locator('main h1').first().isVisible());
  });

  test('CSP meta tag is intact and enforced', async () => {
    const csp = await h.win.locator('meta[http-equiv="Content-Security-Policy"]').getAttribute('content');
    assert.ok(csp, 'CSP meta tag present');
    for (const d of ["default-src 'self'", "script-src 'self'", "style-src 'self' 'unsafe-inline'", "img-src 'self' data:", "font-src 'self'"]) {
      assert.ok(csp.includes(d), `CSP contains ${d}: ${csp}`);
    }
    assert.ok(!/unsafe-eval|\*\s|http:|https:/.test(csp.replace(/'self'/g, '')), `CSP has no wildcard / remote origin / eval: ${csp}`);
    // Enforced, not just declared: an injected inline script does not run (`script-src 'self'`, no 'unsafe-inline' or
    // 'unsafe-eval'; evaluate() itself runs over CDP, which bypasses the CSP, so eval cannot be probed from here).
    const injected = await h.win.evaluate(() => {
      window.__injected = false;
      const el = document.createElement('script');
      el.textContent = 'window.__injected = true';
      document.head.appendChild(el);
      return window.__injected;
    });
    assert.equal(injected, false, 'inline scripts are blocked by the CSP');
    await poll('the CSP violation to be reported', () => h.errors.some((e) => /Content Security Policy/.test(e)), { timeout: 5000 });
    // The CSP violation itself is logged as a console error by Chromium: that is expected here, not a boot error.
    h.errors.length = 0;
  });

  test('window.ratchet exposes exactly the documented API', async () => {
    const keys = await h.win.evaluate(() => Object.keys(window.ratchet).sort());
    const documented = documentedApi();
    assert.ok(documented.length >= 20, `parsed the RatchetApi methods (${documented.length})`);
    assert.deepEqual(keys, documented);
    const types = await h.win.evaluate(() => Object.values(window.ratchet).every((v) => typeof v === 'function'));
    assert.ok(types, 'every API member is a function');
  });

  test('no Node.js in the renderer', async () => {
    const globals = await h.win.evaluate(() => ({
      require: typeof require, process: typeof process, module: typeof module, Buffer: typeof Buffer,
      __dirname: typeof __dirname, electron: typeof window.electron, ipcRenderer: typeof window.ipcRenderer,
    }));
    for (const [k, v] of Object.entries(globals)) assert.equal(v, 'undefined', `${k} must be undefined in the renderer, got ${v}`);
    const reach = await h.win.evaluate(() => typeof window.ratchet.ipcRenderer + typeof window.ratchet.require);
    assert.equal(reach, 'undefinedundefined');
  });

  test('window runs with context isolation and sandbox, without node integration', async () => {
    const prefs = await h.app.evaluate(({ BrowserWindow }) => {
      const p = BrowserWindow.getAllWindows()[0].webContents.getLastWebPreferences();
      return { contextIsolation: p.contextIsolation, nodeIntegration: p.nodeIntegration, sandbox: p.sandbox };
    });
    assert.deepEqual(prefs, { contextIsolation: true, nodeIntegration: false, sandbox: true });
  });

  test('the window cannot be navigated away or open new windows', async () => {
    const before = h.app.windows().length;
    await h.win.evaluate(() => window.open('https://example.com'));
    await h.win.waitForTimeout(500);
    assert.equal(h.app.windows().length, before);
    assert.match(h.win.url(), /^file:\/\/.*index\.html/);
  });

  test('still no errors after all of the above', () => {
    assert.deepEqual(h.errors, []);
  });
});
