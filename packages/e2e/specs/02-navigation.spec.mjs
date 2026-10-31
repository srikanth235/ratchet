// Navigation: every route reachable from the sidebar renders on an empty ledger, without errors, with an intentional empty state;
// the first screens of the simplified model (three Settings rows, the three-step New optimization, the Templates gallery).
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { ARTIFACTS, SCREENS, goTo, launchApp, openNewOptimization, openSandboxDetails, openSandboxPanel, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';

const EMPTY = [/No optimizations yet/i, /Create one to start a baseline run/i];
const EXPECT = {
  overview: { h1: /^Overview$/, text: EMPTY },
  optimizations: { h1: /^Optimizations$/, text: EMPTY },
  benchmarks: { h1: /^Benchmarks$/, text: [/No optimization selected/i, /Start one from Optimizations/i, /evaluator templates/i] },
  // The screen of the `settings` route is the Connectors screen (its own worker builds it); only the route and the sidebar label are fixed here.
  settings: { h1: /^(Settings|Connectors)$/, text: [/GitHub/, /AWS/] },
};

describe('navigation on an empty ledger', () => {
  let h;
  before(async () => {
    h = await launchApp();
    await h.win.waitForTimeout(1500);
  });
  after(() => h?.close());

  test('sidebar is the logo, four places and the sandbox line; nothing else', async () => {
    const labels = await h.win.getByRole('navigation', { name: 'Primary' }).getByRole('button').allInnerTexts();
    assert.deepEqual(labels.map((l) => l.replace(/\s+/g, ' ').trim()), ['Overview', 'Optimizations', 'Benchmarks', 'Connectors']);
    const active = await h.win.locator('.nav__item--active').innerText();
    assert.match(active, /Overview/);
    assert.equal(await h.win.locator('.sidebar .brand__name').innerText(), 'Ratchet');
    // the old furniture is gone: Live now card, integration chips, workspace switcher, user row, Environments / Agents / Result entries
    for (const gone of ['.live-card', '.integration', '.workspace', '.user']) assert.equal(await h.win.locator(gone).count(), 0, gone);
    assert.equal(await h.win.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: /Environments|Agents|Result|Settings/ }).count(), 0);
    assert.match(await h.win.locator('.sbx__line').innerText(), /No sandbox running/);
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

  test('the sandbox line opens a quiet panel: nothing is billing, no End now, no Plan / Apply / Destroy; Details opens Environments', async () => {
    await goTo(h.win, 'Overview');
    const panel = await openSandboxPanel(h.win);
    assert.match(await panel.innerText(), /No sandbox is running, so nothing is billing/);
    assert.equal(await h.win.getByRole('button', { name: 'End now' }).count(), 0);
    const buttons = (await panel.getByRole('button').allInnerTexts()).map((t) => t.trim());
    assert.deepEqual(buttons.filter((t) => /plan|apply|destroy/i.test(t)), []);
    await h.win.keyboard.press('Escape');
    assert.equal(await h.win.getByTestId('sandbox-panel').count(), 0);
    await openSandboxDetails(h.win);
    assert.equal((await h.win.locator('main h1').first().innerText()).trim(), 'Environments');
    const all = (await h.win.locator('main button').allInnerTexts()).map((t) => t.trim());
    assert.deepEqual(all.filter((t) => /plan|apply|destroy/i.test(t)), []);
    assert.equal(await h.win.locator('.nav__item--active').count(), 0, 'Environments is under no sidebar entry');
  });

  test('Settings (reached through Connectors) has three rows and none of the old per-repo knobs; the development overrides are collapsed', async () => {
    await goTo(h.win, 'Connectors');
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

  test('New optimization is reached from the Overview and Optimizations headers: step 1 offers the repo picker, Local folder and the demos', async () => {
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
