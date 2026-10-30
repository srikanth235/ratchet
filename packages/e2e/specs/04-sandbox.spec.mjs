// Settings and repo selection against fakes, through the real main process: a fake GitHub (device flow, repos, ratchet.yaml),
// fake `claude` / `codex` executables on PATH, and a ~/.aws fixture. No network, no AWS, no installed CLIs.
//
// File name kept (04-sandbox): the sandbox itself is automatic now; its lifecycle is covered by 03 (demo run -> Environments -> receipt).
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { FAKE_LOGIN, FAKE_TOKEN, DEVICE_CODE, AWS_SECRET, fakeMachine, makeLocalRepo, repo, startFakeGithub } from '../harness/fakes.mjs';
import { ARTIFACTS, REPO_ROOT, goTo, launchApp, openNewOptimization, poll, rpc, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';

const SHOTS = join(ARTIFACTS, 'app-live', 'settings');
const DEMO_YAML = readFileSync(join(REPO_ROOT, 'examples/demo-search/ratchet.yaml'), 'utf8');
const TEMPLATE_YAML = `version: 0
objective: { name: Checkout latency, metric: http_req_duration.p95, direction: minimize }
evaluator: { template: http-latency }
environment: { image: .devcontainer/Dockerfile, machine: c7i.4xlarge, region: eu-west-2 }
protected: [docs/**]
`;

describe('Settings: GitHub, AWS and Agents (fakes)', () => {
  let h;
  let gh;
  let machine;
  const shot = (name) => screenshotScreen(h.app, h.win, name, SHOTS);

  before(async () => {
    gh = await startFakeGithub({ repos: [], files: {} });
    machine = fakeMachine();
    h = await launchApp({ env: { ...gh.env, ...machine.env } });
    await h.win.waitForTimeout(1000);
    await goTo(h.win, 'Settings');
    await waitForScreen(h.win, 'settings');
    await waitSettled(h.win);
  });
  after(async () => {
    await h?.close();
    await gh?.close();
    machine?.cleanup();
  });

  test('Agents: both harnesses are detected in the main process with version and sign-in status; hints and key fields only where needed', async () => {
    const row = h.win.getByTestId('row-agents');
    const cc = row.getByTestId('harness-claude-code');
    await poll('Claude Code detected', async () => /v2\.1\.4/.test(await cc.innerText()), { timeout: 30_000 });
    assert.match(await cc.innerText(), /Signed in/);
    assert.equal(await cc.getByLabel('Anthropic API key value').count(), 0, 'no key field for a signed-in harness');
    const cx = row.getByTestId('harness-codex');
    assert.match(await cx.innerText(), /v0\.9\.0/);
    assert.match(await cx.innerText(), /Signed out/);
    assert.match(await cx.getByRole('status').innerText(), /codex login/, 'the sign-in hint');
    assert.equal(await cx.getByLabel('OpenAI API key value').count(), 1, 'a key field for the harness that is not signed in');
    assert.match(await row.innerText(), /1 ready/);
    const api = await rpc(h.win, 'detectHarnesses');
    assert.deepEqual(api.map((x) => [x.kind, x.installed, x.auth]), [['claude-code', true, 'signed-in'], ['codex', true, 'signed-out']]);
    assert.ok(api[0].path.startsWith(machine.dir), 'found on the PATH the app resolved');
  });

  test('Re-detect probes again: signing in to Codex in a terminal is picked up', async () => {
    machine.setCodexSignedIn(true);
    const row = h.win.getByTestId('row-agents');
    await row.getByRole('button', { name: 'Re-detect' }).click();
    const cx = row.getByTestId('harness-codex');
    await poll('Codex signed in', async () => /Signed in/.test(await cx.innerText()), { timeout: 30_000 });
    assert.equal(await cx.getByLabel('OpenAI API key value').count(), 0);
    assert.match(await row.innerText(), /2 ready/);
    assert.equal(await row.getByText('Cloud runners need API keys').locator('xpath=ancestor::details').evaluate((d) => d.open), false);
  });

  test('AWS: profiles come from ~/.aws as names only; picking one connects; the region defaults to us-east-1', async () => {
    const row = h.win.getByTestId('row-aws');
    assert.match(await row.innerText(), /Not connected/);
    const list = row.getByRole('radiogroup', { name: 'AWS profile' });
    await list.waitFor({ timeout: 15_000 });
    assert.deepEqual((await list.locator('.agent-choice__name').allInnerTexts()).map((t) => t.trim()), ['default', 'corp-sso', 'deploy-role']);
    assert.match(await list.getByRole('radio', { name: /corp-sso/ }).innerText(), /SSO · session corp/);
    const body = await h.win.locator('body').innerText();
    for (const secret of ['AKIAIOSFODNN7EXAMPLE', AWS_SECRET, '111122223333', 'awsapps.com']) assert.ok(!body.includes(secret), `the page never shows ${secret}`);
    assert.equal(await row.getByLabel('AWS region').inputValue(), 'us-east-1');
    assert.match(await row.innerText(), /a repo can override it in ratchet\.yaml/);
    await list.getByRole('radio', { name: /corp-sso/ }).click();
    await poll('connected', async () => /Connected · corp-sso/.test(await row.innerText()), { timeout: 15_000 });
    const s = await rpc(h.win, 'getSettings');
    assert.equal(s.awsProfile, 'corp-sso');
    assert.equal(s.dev.awsAuth, 'sso');
    assert.equal(s.awsRegion, 'us-east-1');
    assert.equal(await row.getByRole('button', { name: 'Test connection' }).isEnabled(), true);
  });

  test('GitHub: "Sign in with GitHub" runs the device flow in the main process and shows the code', async (t) => {
    const row = h.win.getByTestId('row-github');
    assert.match(await row.innerText(), /Not signed in/);
    assert.equal((await rpc(h.win, 'githubStatus')).clientIdConfigured, true);
    await row.getByRole('button', { name: 'Sign in with GitHub' }).click();
    const code = row.getByTestId('device-code');
    await code.waitFor({ timeout: 15_000 });
    assert.equal((await code.innerText()).trim(), DEVICE_CODE);
    assert.match(await row.innerText(), /Waiting for you to approve/);
    await shot('01-device-code');
    gh.approve();
    const settings = await rpc(h.win, 'getSettings');
    if (!settings.encryptionAvailable) {
      // No OS keychain here (Linux CI): the app refuses to keep a token in plaintext and says so, instead of signing in.
      await poll('the keychain refusal', async () => /keychain/i.test(await row.getByRole('alert').innerText().catch(() => '')), { timeout: 30_000 });
      assert.equal((await rpc(h.win, 'githubStatus')).signedIn, false);
      return t.skip('no OS keychain in this environment: the token cannot be stored (covered by unit tests)');
    }
    await poll('signed in as the fake account', async () => new RegExp(`Signed in as ${FAKE_LOGIN}`).test(await row.innerText()), { timeout: 30_000 });
    assert.equal((await rpc(h.win, 'githubStatus')).login, FAKE_LOGIN);
    assert.ok(!(await h.win.locator('body').innerText()).includes(FAKE_TOKEN), 'the token never reaches the page');
    assert.ok(!JSON.stringify(await rpc(h.win, 'getSettings')).includes(FAKE_TOKEN));
    await row.getByRole('button', { name: 'Sign out' }).click();
    await poll('signed out', async () => /Not signed in/.test(await row.innerText()), { timeout: 15_000 });
  });

  test('pasting a token is an Advanced fallback; the page never gets a secret back', async (t) => {
    const row = h.win.getByTestId('row-github');
    const adv = row.getByText('Advanced: paste a token instead');
    await adv.click();
    const input = row.getByLabel('GitHub token value');
    if (await input.isDisabled()) return t.skip('no OS keychain in this environment: secrets cannot be stored (covered by unit tests)');
    await input.fill('ghp_E2EPASTEDTOKEN0123456789abcdefgh');
    await row.getByRole('button', { name: 'Save' }).click();
    await poll('signed in with the pasted token', async () => new RegExp(`Signed in as ${FAKE_LOGIN}`).test(await row.innerText()), { timeout: 30_000 });
    assert.match(await row.innerText(), /pasted token/);
    assert.ok(!(await h.win.locator('body').innerText()).includes('ghp_E2EPASTED'));
    assert.equal(await h.win.evaluate(() => typeof window.ratchet.getSecret), 'undefined');
    await shot('02-signed-in');
  });

  test('the development overrides are collapsed and validated', async () => {
    await h.win.getByText('Advanced (development)').click();
    await h.win.getByLabel('Bench AMI', { exact: true }).fill('nope');
    await h.win.getByText(/Use an AMI id such as ami-/).waitFor();
    assert.equal(await h.win.locator('details', { hasText: 'Advanced (development)' }).getByRole('button', { name: 'Save' }).isDisabled(), true);
    assert.deepEqual(h.errors, []);
    await shot('03-settings');
  });

  test('the settings file keeps no secret and no credential from ~/.aws', async () => {
    const file = readdirSync(h.userDataDir).includes('settings.json') ? readFileSync(join(h.userDataDir, 'settings.json'), 'utf8') : '';
    for (const secret of [FAKE_TOKEN, 'ghp_E2EPASTED', 'AKIA', AWS_SECRET]) assert.ok(!file.includes(secret), `settings.json does not contain ${secret}`);
  });
});

describe('New optimization against a fake GitHub account and local folders', () => {
  let h;
  let gh;
  let machine;
  let local;
  let scratch;
  const shot = (name) => screenshotScreen(h.app, h.win, name, join(ARTIFACTS, 'app-live', 'new-optimization'));

  before(async () => {
    gh = await startFakeGithub({
      repos: [
        repo('acme/search-api', 'main', 'the search service', true),
        repo('acme/bare', 'trunk', 'no ratchet.yaml yet'),
        repo('acme/broken', 'main', 'a ratchet.yaml that does not parse'),
      ],
      files: {
        'acme/search-api@main:ratchet.yaml': DEMO_YAML,
        'acme/broken@main:ratchet.yaml': 'version: 3\n',
      },
    });
    machine = fakeMachine();
    local = makeLocalRepo({ remote: 'git@github.com:acme/checkout.git', yaml: TEMPLATE_YAML, branch: 'trunk' });
    scratch = makeLocalRepo({});
    // GITHUB_TOKEN stands in for the sign-in (no keychain on CI): the account's repos come from the fake.
    h = await launchApp({ env: { ...gh.env, ...machine.env, GITHUB_TOKEN: 'ghp_E2EENVTOKEN0123456789abcdefghijkl', RATCHET_E2E_PICK_FOLDER: local.dir } });
    await h.win.waitForTimeout(1000);
  });
  after(async () => {
    await h?.close();
    await gh?.close();
    machine?.cleanup();
    local?.cleanup();
    scratch?.cleanup();
  });

  const pick = (name) => h.win.getByRole('list', { name: 'Repositories' }).getByRole('button', { name: new RegExp(name) });

  test('the picker lists the account repos and searches them', async () => {
    await openNewOptimization(h.win);
    await pick('acme/search-api').waitFor({ timeout: 30_000 });
    assert.deepEqual((await h.win.locator('.repo-row__name').allInnerTexts()).map((t) => t.trim()), ['acme/search-api', 'acme/bare', 'acme/broken']);
    await h.win.getByLabel('Search repositories').fill('bare');
    assert.deepEqual((await h.win.locator('.repo-row__name').allInnerTexts()).map((t) => t.trim()), ['acme/bare']);
    await h.win.getByLabel('Search repositories').fill('');
  });

  test('a repo with ratchet.yaml: read from the default branch, then Review shows what the repo declares and what is missing', async () => {
    await pick('acme/search-api').click();
    const found = h.win.getByTestId('repo-found');
    await found.waitFor({ timeout: 30_000 });
    assert.match(await found.innerText(), /Found ratchet\.yaml on main of acme\/search-api: http_req_duration\.p95/);
    await h.win.getByRole('button', { name: 'Continue to review' }).click();
    await h.win.getByRole('heading', { name: 'Review', level: 2 }).waitFor();
    const facts = await h.win.getByLabel('What will run').innerText();
    assert.match(facts, /http_req_duration\.p95\s*↓ lower is better/);
    assert.match(facts, /evaluator .*bench\/k6\/search\.js/);
    assert.match(facts, /Toolchain\s+host/i);
    assert.match(facts, /Machine\s+c7i\.2xlarge/i);
    assert.match(facts, /Noise is measured during the baseline/);
    const prot = await h.win.getByLabel('Protected paths').innerText();
    for (const g of ['ratchet.yaml', 'bench/**', 'tests/contract/**']) assert.ok(prot.includes(g), `protected: ${g}`);
    // the detected agents are pre-selected: Claude Code is signed in (fake CLI), Codex is not
    const agents = h.win.getByRole('group', { name: 'Agents' });
    await poll('Claude Code pre-selected', async () => (await agents.getByRole('button', { name: /Claude Code/ }).getAttribute('aria-pressed')) === 'true', { timeout: 15_000 });
    assert.match(await agents.getByRole('button', { name: /Claude Code/ }).innerText(), /v2\.1\.4 · signed in/);
    assert.equal(await agents.getByRole('button', { name: /Codex/ }).getAttribute('aria-pressed'), 'false');
    assert.equal(await h.win.getByRole('radiogroup', { name: 'Where agents run' }).getByRole('radio', { name: 'This Mac' }).getAttribute('aria-checked'), 'true');
    // no AWS profile is connected: a short "what's missing" list, not a settings form
    const missing = h.win.getByTestId('missing');
    await poll('the missing list', async () => /Connect AWS/.test(await missing.innerText()), { timeout: 15_000 });
    assert.equal(await missing.locator('li').count(), 1);
    assert.equal(await h.win.getByRole('button', { name: 'Continue to budget' }).isDisabled(), true);
    assert.equal(await h.win.getByLabel('AWS profile').count(), 0);
    // the yaml is a read-only disclosure
    await h.win.getByText('View ratchet.yaml').click();
    assert.match(await h.win.getByLabel('ratchet.yaml (read-only)').innerText(), /bench\/k6\/search\.js/);
    assert.equal(await h.win.locator('textarea').count(), 0, 'nothing to edit');
    await shot('01-review-missing');
    // fixing it in Settings clears the list
    await rpc(h.win, 'setAwsProfile', 'default');
    await goTo(h.win, 'Optimizations');
    await openNewOptimization(h.win);
    await pick('acme/search-api').click();
    await h.win.getByTestId('repo-found').waitFor();
    await h.win.getByRole('button', { name: 'Continue to review' }).click();
    await h.win.getByTestId('missing').getByText(/Everything this run needs is in place/).waitFor({ timeout: 15_000 });
    assert.equal(await h.win.getByRole('button', { name: 'Continue to budget' }).isEnabled(), true);
    // "This Mac" vs "Cloud": cloud runners need API keys, so the list names them
    await h.win.getByRole('radio', { name: 'Cloud (unattended)' }).click();
    await poll('API key missing for cloud', async () => /Anthropic API key/.test(await h.win.getByTestId('missing').innerText()), { timeout: 15_000 });
    await h.win.getByRole('radio', { name: 'This Mac' }).click();
    await h.win.getByRole('button', { name: 'Continue to budget' }).click();
    await h.win.getByRole('heading', { name: 'Budget & time', level: 2 }).waitFor();
    assert.match(await h.win.locator('.review').innerText(), /Repository\s+acme\/search-api/);
    assert.match(await h.win.locator('.review').innerText(), /Run on\s+This Mac/);
    assert.equal(await h.win.getByLabel('Budget', { exact: true }).inputValue(), '50');
    assert.deepEqual(h.errors, []);
  });

  test('a repo without ratchet.yaml offers the templates, each with "Add via pull request"', async () => {
    await goTo(h.win, 'Optimizations');
    await openNewOptimization(h.win);
    await pick('acme/bare').click();
    const offer = h.win.getByTestId('template-offer');
    await offer.waitFor({ timeout: 30_000 });
    assert.match(await offer.innerText(), /acme\/bare has no ratchet\.yaml on trunk/);
    const items = await offer.locator('.template-offer__title').allInnerTexts();
    assert.ok(items.includes('HTTP latency') && items.includes('CLI cold start') && items.length >= 7, `templates: ${items}`);
    assert.equal(await offer.getByRole('button', { name: /via pull request/ }).count(), items.length);
    assert.match(await offer.getByRole('button', { name: /via pull request/ }).first().innerText(), /^Add via pull request$/);
    assert.equal(await h.win.getByRole('button', { name: 'Continue to review' }).isDisabled(), true);
    await shot('02-template-offer');
  });

  test('a ratchet.yaml that does not parse is reported with the reason, and blocks the step', async () => {
    await pick('acme/broken').click();
    const found = h.win.getByTestId('repo-found');
    await poll('the parse error', async () => /does not parse/.test(await found.innerText()), { timeout: 30_000 });
    assert.match(await found.innerText(), /version/);
    assert.equal(await h.win.getByRole('button', { name: 'Continue to review' }).isDisabled(), true);
  });

  test('Local folder… reads the folder through the dialog: its GitHub remote and ratchet.yaml', async () => {
    await h.win.getByRole('button', { name: /Local folder/ }).click();
    const found = h.win.getByTestId('repo-found');
    await poll('the local repo', async () => /Found ratchet\.yaml on trunk of acme\/checkout/.test(await found.innerText()), { timeout: 30_000 });
    await h.win.getByRole('button', { name: 'Continue to review' }).click();
    const facts = await h.win.getByLabel('What will run').innerText();
    assert.match(facts, /evaluator template http-latency/);
    assert.match(facts, /hidden request mix/);
    assert.match(facts, /\.devcontainer\/Dockerfile/);
    assert.match(facts, /c7i\.4xlarge/);
    assert.match(facts, /eu-west-2/);
    const prot = await h.win.getByLabel('Protected paths').innerText();
    for (const g of ['ratchet.yaml', 'bench/**', '.devcontainer/Dockerfile', 'docs/**']) assert.ok(prot.includes(g), `protected: ${g}`);
    await shot('03-local-review');
  });

  test('a local folder without a GitHub remote says why it cannot be used', async () => {
    await h.app.evaluate((_, dir) => {
      process.env.RATCHET_E2E_PICK_FOLDER = dir;
    }, scratch.dir);
    await goTo(h.win, 'Optimizations');
    await openNewOptimization(h.win);
    await h.win.getByRole('button', { name: /Local folder/ }).click();
    const found = h.win.getByTestId('repo-found');
    await poll('the reason', async () => /no GitHub remote/.test(await found.innerText()), { timeout: 30_000 });
    assert.equal(await h.win.getByRole('button', { name: 'Continue to review' }).isDisabled(), true);
    assert.deepEqual(h.errors, []);
  });

  test('the fake GitHub saw only reads: no repo was written to', async () => {
    assert.ok(gh.state.requests.every((r) => /^(GET|POST \/login\/)/.test(r)), gh.state.requests.join('\n'));
  });
});
