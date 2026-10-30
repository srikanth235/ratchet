// Navigation: every route reachable from the sidebar renders on an empty ledger, without errors, with an intentional empty state;
// the first screens of the simplified model (three Settings rows, the three-step New optimization, the Templates gallery).
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { ARTIFACTS, SCREENS, goTo, launchApp, openNewOptimization, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';

const EXPECT = {
  optimizations: { h1: /^Optimizations$/, text: [/No optimizations yet/i, /Create one to start a baseline run/i, /Verified wins/i, /All optimizations/i] },
  benchmarks: { h1: /^Benchmarks$/, text: [/No optimization selected/i, /Start one from Optimizations/i, /evaluator templates/i] },
  agents: { h1: /^Agents$/, text: [/No optimization selected/i, /Start one from Optimizations/i] },
  sandbox: { h1: /^Environments$/, text: [/No environment is running/i, /Last teardown receipt/i, /No environment has been removed yet/i, /How it fits together/i, /None running/i] },
  result: { h1: /^Result$/, text: [/No optimization selected/i, /Start one from Optimizations/i] },
  settings: { h1: /^Settings$/, text: [/GitHub/, /Sign in with GitHub/, /AWS/, /Region/, /Agents/, /Re-detect/, /Advanced \(development\)/] },
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
    assert.deepEqual(await h.win.locator('.integration').allInnerTexts(), ['GitHub', 'AWS', 'Claude Code', 'Codex']);
    // nothing is signed in or connected on a clean machine
    assert.equal(await h.win.locator('.integration--on', { hasText: /GitHub|AWS/ }).count(), 0);
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

  test('Environments is read-only: no Plan / Apply / Destroy anywhere', async () => {
    await goTo(h.win, 'Environments');
    await waitForScreen(h.win, 'sandbox');
    const buttons = (await h.win.locator('main button').allInnerTexts()).map((t) => t.trim());
    assert.deepEqual(buttons.filter((t) => /plan|apply|destroy/i.test(t)), []);
  });

  test('Settings has three rows and none of the old per-repo knobs; the development overrides are collapsed', async () => {
    await goTo(h.win, 'Settings');
    await waitForScreen(h.win, 'settings');
    await waitSettled(h.win);
    for (const row of ['github', 'aws', 'agents']) assert.equal(await h.win.getByTestId(`row-${row}`).count(), 1, `row ${row}`);
    assert.equal(await h.win.getByRole('radio', { name: /Simulation|Live/ }).count(), 0, 'no simulation / live toggle');
    const open = await h.win.locator('details').evaluateAll((ds) => ds.map((d) => d.open));
    assert.ok(open.length >= 3 && open.every((o) => !o), 'every disclosure starts collapsed');
    // GitHub: signed out, offering the device flow; AWS: nothing connected, with the region default and the override note
    const gh = h.win.getByTestId('row-github');
    assert.match(await gh.innerText(), /Not signed in/);
    assert.equal(await gh.getByRole('button', { name: 'Sign in with GitHub' }).isEnabled(), true);
    const aws = h.win.getByTestId('row-aws');
    assert.match(await aws.innerText(), /Not connected/);
    assert.equal(await aws.getByLabel('AWS region').inputValue(), 'us-east-1');
    assert.match(await aws.innerText(), /a repo can override it in ratchet\.yaml/);
    // Agents: both harnesses are reported (installed or not), never silently missing
    const agents = h.win.getByTestId('row-agents');
    assert.equal(await agents.getByTestId('harness-claude-code').count(), 1);
    assert.equal(await agents.getByTestId('harness-codex').count(), 1);
    assert.equal(await agents.getByRole('button', { name: 'Re-detect' }).isEnabled(), true);
    assert.deepEqual(h.errors, []);
  });

  test('New optimization is reached from the Optimizations header: step 1 offers the repo picker, Local folder and the demos', async () => {
    await openNewOptimization(h.win);
    await waitSettled(h.win);
    assert.equal((await h.win.locator('main h1').first().innerText()).trim(), 'New optimization');
    assert.deepEqual((await h.win.getByRole('navigation', { name: 'Steps' }).getByRole('button').allInnerTexts()).map((t) => t.replace(/\s+/g, ' ').trim()), ['1 Repo', '2 Review', '3 Budget & time']);
    assert.ok(await h.win.locator('.step h2', { hasText: /^Repo$/ }).isVisible());
    assert.equal(await h.win.getByRole('button', { name: /Local folder/ }).isEnabled(), true);
    assert.match(await h.win.getByRole('status').first().innerText(), /not signed in to GitHub/i);
    assert.equal(await h.win.getByLabel('Search repositories').isDisabled(), true);
    const demos = h.win.getByRole('group', { name: 'Demos' });
    assert.deepEqual((await demos.locator('.agent-choice__name').allInnerTexts()).map((t) => t.trim()), ['Search API latency', 'CLI cold start', 'SKU demand forecast']);
    // the yaml editor and its cards are gone: the repo owns the config
    for (const gone of ['ratchet.yaml', 'New guardrail']) assert.equal(await h.win.getByLabel(gone, { exact: true }).count(), 0, gone);
    assert.equal(await h.win.locator('main input[name=evaluator]').count(), 0);
    assert.equal(await h.win.getByRole('button', { name: 'Continue to review' }).isDisabled(), true);
    await screenshotScreen(h.app, h.win, 'new-optimization', join(ARTIFACTS, 'app-live', 'empty'));
    await h.win.getByRole('button', { name: 'Cancel' }).click();
    await waitForScreen(h.win, 'optimizations');
    assert.deepEqual(h.errors, []);
  });

  test('Benchmarks has a Templates gallery next to the history, with the demos', async () => {
    await goTo(h.win, 'Benchmarks');
    await waitForScreen(h.win, 'benchmarks');
    await h.win.getByRole('radio', { name: 'Templates' }).click();
    await h.win.getByRole('region', { name: 'Evaluator templates' }).waitFor();
    const cards = h.win.getByTestId('template-card');
    assert.deepEqual(await cards.evaluateAll((cs) => cs.map((c) => c.getAttribute('data-template'))), ['http-latency', 'cli-cold-start', 'test-suite-time', 'build-size', 'memory', 'forecast-backtest', 'llm-eval']);
    const http = cards.filter({ has: h.win.getByText('HTTP latency') });
    const text = await http.innerText();
    for (const re of [/p50 \/ p95 \/ p99 latency/, /hidden request mix/, /c7i\.2xlarge/]) assert.match(text, re);
    assert.equal(await http.getByRole('button', { name: /Use template/ }).isEnabled(), true);
    assert.deepEqual(await h.win.getByTestId('demo-card').evaluateAll((cs) => cs.map((c) => c.getAttribute('data-template'))), ['demo-search', 'demo-cli', 'demo-forecast']);
    await screenshotScreen(h.app, h.win, 'benchmarks-templates', join(ARTIFACTS, 'app-live', 'empty'));
    await h.win.getByRole('radio', { name: 'Benchmark history' }).click();
    await h.win.getByText('No optimization selected').waitFor();
    assert.deepEqual(h.errors, []);
  });
});
