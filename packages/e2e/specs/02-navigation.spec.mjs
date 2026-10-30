// Navigation: every route reachable from the sidebar renders on an empty ledger, without errors, with an intentional empty state.
import assert from 'node:assert/strict';
import { after, before, describe, test } from 'node:test';
import { ARTIFACTS, SCREENS, goTo, launchApp, openNewOptimization, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';
import { join } from 'node:path';

const EXPECT = {
  optimizations: { h1: /^Optimizations$/, text: [/No optimizations yet/i, /Create one to start a baseline run/i, /Verified wins/i, /All optimizations/i] },
  benchmarks: { h1: /^Benchmarks$/, text: [/No optimization selected/i, /Start one from Optimizations/i] },
  agents: { h1: /^Agents$/, text: [/No optimization selected/i, /Start one from Optimizations/i] },
  sandbox: { h1: /^AWS sandbox$/, text: [/How it fits together/i, /OpenTofu plan/i, /Agent limits/i, /Simulated sandbox/i, /Plan ready/i] },
  result: { h1: /^Result$/, text: [/No optimization selected/i, /Start one from Optimizations/i] },
  settings: { h1: /^Settings$/, text: [/Mode/i, /API keys/i, /Simulation/i, /AWS/] },
};

describe('navigation on an empty ledger', () => {
  let h;
  before(async () => {
    h = await launchApp();
    await h.win.waitForTimeout(1500);
  });
  after(() => h?.close());

  test('sidebar lists every screen and the integrations', async () => {
    const labels = await h.win.getByRole('navigation', { name: 'Primary' }).getByRole('button').allInnerTexts();
    assert.deepEqual(labels.map((l) => l.replace(/\s+/g, ' ').trim()), ['Optimizations', 'Benchmarks', 'Agents', 'Environments', 'Result', 'Settings']);
    const active = await h.win.locator('.nav__item--active').innerText();
    assert.match(active, /Optimizations/);
    assert.equal(await h.win.locator('.live-card').count(), 0, 'no "Live now" card without a running optimization');
    assert.equal(await h.win.locator('.integration').count(), 4);
  });

  for (const s of SCREENS) {
    test(`${s.nav} renders with an intentional empty state`, async () => {
      await goTo(h.win, s.nav);
      await waitForScreen(h.win, s.route);
      await waitSettled(h.win);
      await h.win.waitForTimeout(600);
      const x = EXPECT[s.route];
      const h1 = (await h.win.locator('main h1').first().innerText()).trim();
      assert.match(h1, x.h1);
      const text = await h.win.locator('main').textContent();
      for (const re of x.text) assert.match(text, re, `${s.nav}: expected ${re}`);
      // The only legitimate alert: Linux/CI has no OS keychain, so Settings says secrets cannot be stored.
      const alerts = (await h.win.locator('main [role="alert"]').allTextContents()).filter((t) => !/OS keychain is unavailable/.test(t));
      assert.deepEqual(alerts, [], `${s.nav}: no error notes`);
      assert.ok(!/undefined|NaN|\[object/.test(text), `${s.nav}: no undefined/NaN leaking into the UI`);
      assert.equal(await h.win.locator(`.nav__item--active`).innerText().then((t) => t.replace(/\s+/g, ' ').trim()), s.nav);
      await screenshotScreen(h.app, h.win, s.name, join(ARTIFACTS, 'app-live', 'empty'));
      assert.deepEqual(h.errors, []);
    });
  }

  test('New optimization is reached from the Optimizations header and shows the form', async () => {
    await openNewOptimization(h.win);
    await waitSettled(h.win);
    assert.equal((await h.win.locator('main h1').first().innerText()).trim(), 'New optimization');
    for (const step of ['Target', 'Evaluator & metric', 'Guardrails', 'Agents & limits']) {
      assert.ok(await h.win.locator('.step h2', { hasText: step }).isVisible(), `step ${step}`);
    }
    await h.win.getByText('Valid config').waitFor({ timeout: 15_000 });
    assert.equal(await h.win.getByRole('button', { name: /Run baseline & start/ }).isEnabled(), true);
    await screenshotScreen(h.app, h.win, 'new-optimization', join(ARTIFACTS, 'app-live', 'empty'));
    await h.win.getByRole('button', { name: 'Cancel' }).click();
    await waitForScreen(h.win, 'optimizations');
    assert.deepEqual(h.errors, []);
  });

  test('per-optimization tabs are not shown without an optimization, and Settings round-trips', async () => {
    await goTo(h.win, 'Settings');
    await waitForScreen(h.win, 'settings');
    assert.equal(await h.win.locator('.tabs').count(), 0);
    assert.equal(await h.win.getByRole('radio', { name: /Simulation/ }).getAttribute('aria-checked'), 'true');
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    assert.deepEqual(h.errors, []);
  });
});
