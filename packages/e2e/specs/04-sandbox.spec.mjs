// Sandbox lifecycle in simulation, through the real engine and the mock sandbox provider:
// plan diff -> Apply (with confirmation) -> real recorded outputs -> Destroy (with confirmation) -> back to "after apply".
// Also the live-settings form: validation as you type, and that Settings never shows a stored secret.
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { ARTIFACTS, goTo, launchApp, poll, rpc, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';

const SHOTS = join(ARTIFACTS, 'app-live', 'sandbox');
const SECRET = 'sk-ant-api03-E2E-NOT-A-REAL-KEY-123456';

describe('sandbox lifecycle (real engine, simulation mode)', () => {
  let h;
  const shot = (name) => screenshotScreen(h.app, h.win, name, SHOTS);
  const badge = () => h.win.locator('.page-header__title-row .rt-pill').first();
  const outputRows = () => h.win.locator('.outputs .kv-row');

  before(async () => {
    h = await launchApp();
    await h.win.waitForTimeout(1500);
    await goTo(h.win, 'Environments');
    await waitForScreen(h.win, 'sandbox');
    await waitSettled(h.win);
  });
  after(() => h?.close());

  test('before apply: the plan diff is shown, nothing is applied, outputs say "after apply"', async () => {
    await badge().filter({ hasText: /Plan ready/ }).waitFor({ timeout: 30_000 });
    const items = await h.win.getByRole('list', { name: 'Plan changes' }).locator('li').count();
    assert.ok(items >= 15, `a real plan diff is listed (${items} resources)`);
    assert.equal(await h.win.locator('.plan--create').count(), items, 'every change is a create on a fresh sandbox');
    assert.match(await h.win.locator('.plan-card__head').innerText(), /Plan: \d+ to add, 0 to change, 0 to destroy/);
    assert.equal(await outputRows().filter({ hasText: 'after apply' }).count(), 5);
    assert.equal(await h.win.getByRole('button', { name: 'Destroy sandbox' }).count(), 0, 'nothing to destroy yet');
    assert.equal((await rpc(h.win, 'sandboxInfo')).status, 'planned');
    await shot('01-planned');
  });

  test('Apply asks for confirmation; cancelling applies nothing', async () => {
    await h.win.getByRole('button', { name: 'Apply sandbox' }).click();
    const confirm = h.win.getByRole('group', { name: 'Confirm apply' });
    await confirm.waitFor();
    assert.match(await confirm.innerText(), /Plan: \d+ to add/);
    assert.match(await confirm.innerText(), /nothing is created in AWS/i);
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    assert.equal(await h.win.getByRole('group', { name: 'Confirm apply' }).count(), 0);
    assert.equal((await rpc(h.win, 'sandboxInfo')).status, 'planned');
  });

  test('confirmed Apply records the outputs: the screen shows them instead of "after apply"', async () => {
    await h.win.getByRole('button', { name: 'Apply sandbox' }).click();
    await h.win.getByRole('group', { name: 'Confirm apply' }).getByRole('button', { name: 'Confirm apply' }).click();
    await badge().filter({ hasText: 'Applied' }).waitFor({ timeout: 30_000 });
    const info = await rpc(h.win, 'sandboxInfo');
    assert.equal(info.status, 'applied');
    assert.ok(info.outputs, 'outputs come from the ledger');
    assert.equal(info.account.accountId, '000000000000');
    assert.ok(info.appliedAt > 0);
    await poll('outputs rendered', async () => (await outputRows().filter({ hasText: 'after apply' }).count()) === 0, { timeout: 10_000 });
    const text = await h.win.locator('.outputs').innerText();
    assert.ok(text.includes(info.outputs.artifactsBucket), 'artifacts_bucket value is shown');
    assert.ok(text.includes(info.outputs.sealedBucket), 'sealed_bucket value is shown');
    assert.ok(text.includes(info.outputs.network.subnetId), 'network_ref value is shown');
    // the plan now has nothing left to do, and Apply is gone while Destroy is offered
    assert.equal(await h.win.locator('.plan--noop').count(), await h.win.getByRole('list', { name: 'Plan changes' }).locator('li').count());
    assert.equal(await h.win.getByRole('button', { name: 'Apply sandbox' }).count(), 0);
    assert.equal(await h.win.getByRole('button', { name: 'Destroy sandbox' }).isEnabled(), true);
    await shot('02-applied');
    assert.deepEqual(h.errors, []);
  });

  test('the applied state survives navigating away and back', async () => {
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    await goTo(h.win, 'Environments');
    await waitForScreen(h.win, 'sandbox');
    await badge().filter({ hasText: 'Applied' }).waitFor({ timeout: 15_000 });
  });

  test('Destroy asks for confirmation, then clears the outputs', async () => {
    await h.win.getByRole('button', { name: 'Destroy sandbox' }).click();
    const confirm = h.win.getByRole('group', { name: 'Confirm destroy' });
    await confirm.waitFor();
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    assert.equal((await rpc(h.win, 'sandboxInfo')).status, 'applied', 'cancel destroys nothing');

    await h.win.getByRole('button', { name: 'Destroy sandbox' }).click();
    await h.win.getByRole('group', { name: 'Confirm destroy' }).getByRole('button', { name: 'Confirm destroy' }).click();
    await badge().filter({ hasText: 'Destroyed' }).waitFor({ timeout: 30_000 });
    const info = await rpc(h.win, 'sandboxInfo');
    assert.equal(info.status, 'destroyed');
    assert.equal(info.outputs, null);
    await poll('outputs cleared', async () => (await outputRows().filter({ hasText: 'after apply' }).count()) === 5, { timeout: 10_000 });
    await shot('03-destroyed');
    // and it can be applied again
    assert.equal(await h.win.getByRole('button', { name: 'Apply sandbox' }).isEnabled(), true);
    assert.deepEqual(h.errors, []);
  });
});

describe('live settings form', () => {
  let h;
  before(async () => {
    h = await launchApp();
    await h.win.waitForTimeout(1000);
    await goTo(h.win, 'Settings');
    await waitForScreen(h.win, 'settings');
    await waitSettled(h.win);
  });
  after(() => h?.close());

  const liveSave = () => h.win.locator('form.live-form').getByRole('button', { name: 'Save', exact: true });

  test('lists what a live run needs, validates as you type, and saves valid values', async () => {
    const readiness = h.win.getByRole('list', { name: 'Live readiness' });
    assert.match(await readiness.innerText(), /AWS sign-in/);
    assert.match(await readiness.innerText(), /Sandbox applied/);
    assert.equal(await h.win.getByText('Not ready').count(), 1);

    await h.win.getByLabel('AWS region', { exact: true }).fill('moon-base-1');
    await h.win.getByText(/Use a region id such as us-east-1/).waitFor();
    assert.equal(await liveSave().isDisabled(), true);
    await h.win.getByLabel('AWS region', { exact: true }).fill('eu-west-2');
    await h.win.getByLabel('AWS profile', { exact: true }).fill('ratchet-sandbox');
    await h.win.getByLabel('GitHub repository', { exact: true }).fill('acme/search-api');
    await h.win.getByLabel('Runner AMI', { exact: true }).fill('ami-0123456789abcdef0');
    await h.win.getByLabel('Bench AMI', { exact: true }).fill('ami-0fedcba9876543210');
    await poll('Save enabled', () => liveSave().isEnabled());
    await liveSave().click();
    await poll('saved', async () => (await rpc(h.win, 'getSettings')).live.awsRegion === 'eu-west-2');
    const s = await rpc(h.win, 'getSettings');
    assert.equal(s.awsProfile, 'ratchet-sandbox');
    assert.equal(s.live.githubRepo, 'acme/search-api');
    assert.equal(s.live.runnerImageRef, 'ami-0123456789abcdef0');
    const failing = s.live.readiness.filter((r) => r.state === 'fail').map((r) => r.id);
    // (keys may count as configured through the environment of the machine running this test)
    assert.ok(failing.includes('sandbox'), 'the sandbox is not applied');
    assert.ok(!failing.includes('aws') && !failing.includes('images') && !failing.includes('tofu'), `AWS, images and OpenTofu are ready, got ${failing}`);
    assert.equal(s.mode, 'simulation', 'saving live settings does not switch the mode');
    await screenshotScreen(h.app, h.win, 'settings', SHOTS);
  });

  test('a stored secret is never sent back to the page', async (t) => {
    const input = h.win.getByLabel('Anthropic API key value');
    if (await input.isDisabled()) return t.skip('no OS keychain in this environment: secrets cannot be stored (covered by unit tests)');
    await input.fill(SECRET);
    await h.win.locator('[data-testid="secret-anthropicApiKey"]').getByRole('button', { name: 'Save' }).click();
    await h.win.locator('[data-testid="secret-anthropicApiKey"]').getByText('Configured').waitFor();
    assert.ok(!(await h.win.locator('body').innerText()).includes(SECRET));
    assert.ok(!JSON.stringify(await rpc(h.win, 'getSettings')).includes(SECRET));
    assert.equal(await h.win.evaluate(() => typeof window.ratchet.getSecret), 'undefined');
  });
});
