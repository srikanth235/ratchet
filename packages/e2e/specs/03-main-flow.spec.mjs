// The main flow against the REAL core engine in simulation mode (git copy of examples/demo-search, mock AWS, local
// evaluator running a real build + tests + load generator, scripted agents applying real patches), on a fresh ledger.
//
// Every wait polls on UI state with generous timeouts; the run takes about 10 minutes. Screenshots of each screen made
// during the real run land in artifacts/app-live/.
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { ARTIFACTS, REPO_ROOT, fmtNum, goTo, launchApp, openNewOptimization, poll, rpc, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';

const YAML = readFileSync(join(REPO_ROOT, 'examples/demo-search/ratchet.yaml'), 'utf8');
const NAME = 'search-api p95 latency'; // objective.name of the demo config
const SHOTS = join(ARTIFACTS, 'app-live');
const MIN = 60_000;

const T = {
  start: 90_000, // create + navigate to Agents
  running: 6 * MIN, // baseline (~1 min) + agents launched
  events: 4 * MIN,
  verdict: 8 * MIN,
  winWindow: 4 * MIN, // extra time to wait for a verified win (never required: depends on evaluator noise)
  stop: 4 * MIN,
};

describe('main flow (real engine, simulation mode)', () => {
  let h;
  let id = null;
  let wins = 0;
  const shot = (name) => screenshotScreen(h.app, h.win, name, SHOTS);
  const needId = () => assert.ok(id, 'an optimization was created by an earlier step');
  const card = (kind) => h.win.locator('.agent-card', { has: h.win.locator(`.agent-card__name`, { hasText: kind }) });
  const badge = () => h.win.locator('.page-header__title-row .rt-pill').first();
  const api = () => rpc(h.win, 'getOptimization', id);
  const candidates = () => rpc(h.win, 'candidatesWithResults', id);
  const winCount = async () => (await candidates()).filter((c) => c.movedRatchet).length;

  before(async () => {
    h = await launchApp();
    await h.win.waitForTimeout(1500);
  });
  after(() => h?.close());

  test('the ledger starts empty', async () => {
    const snap = await rpc(h.win, 'listOptimizations');
    assert.equal(snap.optimizations.length, 0);
    assert.equal(snap.engine.kind, 'real', 'the real engine, not the dev stub');
    assert.equal(snap.engine.mode, 'simulation');
  });

  test('pasting the demo ratchet.yaml validates and summarizes metric, evaluator and value', async () => {
    await openNewOptimization(h.win);
    await h.win.getByRole('button', { name: 'Edit', exact: true }).click();
    await h.win.getByLabel('ratchet.yaml', { exact: true }).fill(YAML);
    // The form's own default config is valid too: wait until the pasted one (4 guardrails) is what is being summarized.
    await poll('the pasted config to be summarized', async () => /4 guardrails/.test(await h.win.locator('.file-bar').innerText()), { timeout: 15_000 });
    await h.win.locator('.validation').getByText('Valid config').waitFor({ timeout: 15_000 });
    const bar = await h.win.locator('.file-bar').innerText();
    assert.match(bar, /http_req_duration\.p95/);
    assert.match(bar, /benchmark evaluator/);
    assert.match(bar, /\$1,200 per ms \/ year · estimated/);
    assert.match(bar, /4 guardrails/);
    assert.match(bar, /4 protected paths/);
    const review = await h.win.locator('.review').innerText();
    assert.match(review, /Evaluator\s+benchmark · ratchet\/bench/);
    assert.match(review, /Value\s+\$1,200 per ms \/ year · estimated/);
    assert.match(review, /http_req_duration\.p95 ↓ · min 3%/);
    assert.equal(await h.win.locator('.preflight li[data-state="fail"]').count(), 0, 'preflight has no blocking item');
    assert.equal(await h.win.getByRole('button', { name: /Run baseline & start/ }).isEnabled(), true);
    await shot('new-optimization');
  });

  test('a broken ratchet.yaml gives a path-precise error and blocks submit; fixing it recovers', async () => {
    const editor = h.win.getByLabel('ratchet.yaml', { exact: true });
    const run = h.win.getByRole('button', { name: /Run baseline & start/ });

    await editor.fill(YAML.replace('direction: minimize', 'direction: sideways'));
    const err = h.win.locator('.validation .errors li').first();
    await err.waitFor({ timeout: 15_000 });
    assert.equal(await err.locator('code').innerText(), 'objective.direction');
    assert.match(await err.innerText(), /must be "minimize" or "maximize"/);
    assert.match(await h.win.locator('.validation').innerText(), /1 error\b/);
    assert.equal(await run.isDisabled(), true, 'cannot submit an invalid config');
    assert.match(await h.win.locator('.file-bar').innerText(), /needs attention/);

    await editor.fill(YAML.replace('kind: benchmark', 'kind: vibes').replace('usd_per_unit: 1200', 'usd_per_unit: lots'));
    await poll('two path-precise errors', async () => (await h.win.locator('.validation .errors li code').allInnerTexts()).length >= 2, { timeout: 15_000 });
    const paths = await h.win.locator('.validation .errors li code').allInnerTexts();
    assert.ok(paths.includes('evaluator.kind'), `paths: ${paths}`);
    assert.ok(paths.includes('objective.value.usd_per_unit'), `paths: ${paths}`);
    assert.equal(await run.isDisabled(), true);

    await editor.fill(YAML);
    await h.win.locator('.validation').getByText('Valid config').waitFor({ timeout: 15_000 });
    assert.equal(await run.isEnabled(), true);
    assert.deepEqual(h.errors, []);
  });

  test('Run baseline & start opens Live agents with the new optimization', async () => {
    await h.win.getByRole('button', { name: /Run baseline & start/ }).click();
    await waitForScreen(h.win, 'agents');
    assert.equal((await h.win.locator('main h1').first().innerText()).trim(), NAME);
    id = await poll('the optimization in the ledger', async () => (await rpc(h.win, 'listOptimizations')).optimizations[0]?.id, { timeout: T.start });
    const text = await badge().innerText();
    assert.match(text, /Baselining|Live|Draft|Starting/i, `status badge: ${text}`);
    assert.match(await h.win.locator('.page-header .rt-eyebrow').innerText(), /OPTIMIZATIONS\s*\/\s*local · remote\.git/i);
    // The sidebar announces it as soon as it is baselining / running.
    await h.win.locator('.live-card').waitFor({ timeout: 60_000 });
    assert.match(await h.win.locator('.live-card').innerText(), new RegExp(NAME.slice(0, 12)));
    assert.equal((await h.win.getByRole('tab', { selected: true }).innerText()).trim(), 'Agents');
    await shot('live-agents-baselining');
  });

  test('status reaches running; the baseline is locked and shown', async () => {
    needId();
    await poll('status badge "Live"', async () => /^Live/.test(await badge().innerText()), { timeout: T.running, interval: 1000 });
    const d = await api();
    assert.equal(d.summary.status, 'running');
    assert.ok(d.summary.baseline > 0, 'baseline recorded');
    assert.ok(d.baselineSamples.length >= 3, 'baseline has repeated samples');
    await poll('baseline in the stat card', async () => (await h.win.locator('.stat-card').first().innerText()).includes(`baseline ${fmtNum(d.summary.baseline)}`), { timeout: 30_000 });
  });

  test('both agents are launched, their hypotheses and events stream in', async () => {
    needId();
    for (const kind of ['Claude Code', 'Codex']) {
      await card(kind).waitFor({ timeout: 60_000 });
      await poll(`${kind} running`, async () => /Running/.test(await card(kind).locator('.agent-card__head .rt-pill').innerText()), { timeout: T.events });
      await poll(`${kind} first log line`, async () => (await card(kind).locator('.log__text').count()) > 0, { timeout: T.events });
    }
    await poll('a current hypothesis', async () => (await h.win.locator('[data-testid=hypothesis]').allInnerTexts()).some((t) => t.startsWith('“')), { timeout: T.events });
    const cc = card('Claude Code').locator('.log__text');
    const n1 = await cc.count();
    await poll('more events to stream in', async () => (await cc.count()) > n1, { timeout: 3 * MIN, interval: 1000 });
    assert.match(await card('Claude Code').locator('.agent-card__branch').innerText(), /ratchet\/opt-.*\/cla/);
    assert.deepEqual(h.errors, []);
  });

  test('messaging an agent works', async () => {
    needId();
    const cc = card('Claude Code');
    await cc.getByRole('button', { name: 'Message', exact: true }).click();
    const text = 'Please look at the query tokenizer next.';
    await cc.getByLabel('Message Claude Code').fill(text);
    await cc.getByRole('button', { name: 'Send' }).click();
    await cc.locator('.agent-card__sent').getByText(`Sent: “${text}”`).waitFor({ timeout: 20_000 });
    assert.equal(await cc.getByLabel('Message Claude Code').count(), 0, 'the composer closes after sending');
    assert.equal(await cc.locator('[role=alert]').count(), 0);
  });

  test('pausing and resuming one agent, and all agents, works', async () => {
    needId();
    const cx = card('Codex');
    const pill = (c) => c.locator('.agent-card__head .rt-pill');
    await cx.getByRole('button', { name: 'Pause', exact: true }).click();
    await poll('Codex paused', async () => /Paused/.test(await pill(cx).innerText()), { timeout: 30_000 });
    assert.equal((await api()).agents.find((a) => a.kind === 'codex').status, 'paused');
    assert.match(await pill(card('Claude Code')).innerText(), /Running/, 'the other agent keeps running');
    await cx.getByRole('button', { name: 'Resume', exact: true }).click();
    await poll('Codex running again', async () => /Running/.test(await pill(cx).innerText()), { timeout: 30_000 });

    await h.win.getByRole('button', { name: 'Pause all' }).click();
    await poll('all paused', async () => (await h.win.locator('.agent-card__head .rt-pill').allInnerTexts()).every((t) => /Paused/.test(t)), { timeout: 30_000 });
    await h.win.getByRole('button', { name: 'Resume all' }).click();
    await poll('all running again', async () => (await h.win.locator('.agent-card__head .rt-pill').allInnerTexts()).every((t) => /Running/.test(t)), { timeout: 30_000 });
    assert.equal(await h.win.locator('main [role=alert]').count(), 0);
  });

  test('at least one candidate gets a verdict from the trusted evaluator', async () => {
    needId();
    const decided = await poll('a candidate with a verdict', async () => {
      const ds = (await candidates()).filter((c) => c.verdict !== 'pending');
      return ds.length > 0 && ds;
    }, { timeout: T.verdict, interval: 2000 });
    assert.ok(decided.length >= 1);
    const evals = h.win.locator('.stat-card', { has: h.win.getByText('Evals', { exact: true }) });
    await poll('Evals stat reflects the ledger', async () => {
      const n = (await candidates()).length;
      return (await evals.locator('.stat-card__num').innerText()).trim() === String(n);
    }, { timeout: 30_000 });
    assert.ok(await h.win.locator('.tick-row span').count() >= 1);
    await poll('a log line from the pipeline', async () => (await h.win.locator('.log__text').count()) >= 2, { timeout: 30_000 });
    await shot('live-agents');
  });

  test('Benchmark history shows the baseline, the ratchet chart and every candidate with its verdict', async () => {
    needId();
    await goTo(h.win, 'Benchmarks');
    await waitForScreen(h.win, 'benchmarks');
    await waitSettled(h.win);
    const d = await api();
    const rc = h.win.locator('[data-testid=ratchet-card]');
    await rc.waitFor();
    assert.ok(await rc.locator('svg').count() >= 1, 'the ratchet chart is drawn');
    await poll('baseline label on the chart', async () => /baseline \d/.test(await rc.locator('svg').first().textContent()), { timeout: 30_000 });
    assert.ok(await rc.locator('[data-testid=stair-line]').count() >= 1, 'the best-so-far staircase');
    assert.match(await rc.innerText(), /Best verified/i);
    const rows = h.win.locator('.table-card tbody tr[data-verdict]');
    const decided = rows.filter({ hasNot: h.win.locator('.rt-pill', { hasText: /pending/i }) });
    await poll('a decided candidate row', async () => (await decided.count()) >= 1, { timeout: 60_000 });
    assert.equal(await rows.count(), (await candidates()).length);
    const verdicts = await rows.evaluateAll((trs) => trs.map((t) => t.getAttribute('data-verdict')));
    assert.ok(verdicts.some((v) => v !== 'pending'));
    // filter chips narrow the table
    const v0 = verdicts.find((v) => v !== 'pending');
    await h.win.locator('.chips[aria-label="Filter by verdict"] .chip', { hasText: new RegExp(`^${v0.replace('_', ' ')}$`) }).click();
    const filtered = await rows.evaluateAll((trs) => trs.map((t) => t.getAttribute('data-verdict')));
    assert.ok(filtered.length >= 1 && filtered.every((v) => v === v0), `filter ${v0}: ${filtered}`);
    await h.win.locator('.chips[aria-label="Filter by verdict"] .chip', { hasText: /^all$/ }).click();
    await shot('benchmarks');
  });

  test('Optimizations shows the live run, its chart and the table row', async () => {
    needId();
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    await waitSettled(h.win);
    const f = h.win.locator('[data-testid=featured]');
    await f.waitFor();
    assert.match(await f.innerText(), new RegExp(NAME, 'i'));
    assert.match(await f.locator('.featured__eyebrow').innerText(), /Live/i);
    const d = await api();
    await poll('hero number = best verified', async () => (await f.locator('[data-testid=featured-best]').innerText()).trim() === fmtNum(d.summary.best), { timeout: 30_000 });
    const row = h.win.locator('[data-testid=optimizations-table] tbody tr', { hasText: NAME });
    await row.waitFor();
    assert.match(await row.innerText(), /Running|Baselining/);
    assert.equal(await h.win.locator('.kpi').count(), 4);
    await shot('optimizations');
  });

  test('Environments shows the plan, account and limits while the run is live', async () => {
    needId();
    await goTo(h.win, 'Environments');
    await waitForScreen(h.win, 'sandbox');
    await waitSettled(h.win);
    const info = await rpc(h.win, 'sandboxInfo');
    const text = await h.win.locator('main').innerText();
    assert.match(text, /Simulated sandbox/);
    assert.ok(info.plan.changes.length > 0);
    for (const c of info.plan.changes) assert.ok(text.includes(c.address), `plan lists ${c.address}`);
    for (const t of info.limits.allowedInstanceTypes) assert.ok(text.includes(t), `limits list ${t}`);
    // The app has no "apply" control and the simulation never applies the sandbox: outputs stay "after apply".
    assert.equal(info.outputs, null);
    assert.match(text, /Plan ready · \d+ to add/);
    assert.match(await h.win.locator('.outputs').innerText(), /after apply/);
    await shot('sandbox');
  });

  test('Open pull request stays disabled while the run is live', async () => {
    needId();
    await goTo(h.win, 'Result');
    await waitForScreen(h.win, 'result');
    await waitSettled(h.win);
    assert.equal(await h.win.getByRole('button', { name: /pull request/i }).isDisabled(), true);
    assert.equal(await h.win.getByRole('button', { name: 'Destroy sandbox…' }).isDisabled(), true);
  });

  test('wait (bounded) for a verified win from the ratchet', async (t) => {
    needId();
    try {
      await poll('a verified win', async () => (await winCount()) > 0, { timeout: T.winWindow, interval: 3000 });
    } catch {
      /* depends on the evaluator's noise floor on this machine: the Result assertions cover both outcomes */
    }
    wins = await winCount();
    const d = await api();
    const cs = await candidates();
    t.diagnostic(`verified wins: ${wins}; candidates: ${cs.map((c) => c.verdict).join(',')}; baseline ${d.summary.baseline} best ${d.summary.best} noise ${d.noiseFloor}`);
    for (const c of cs.filter((x) => x.verdict !== 'pending')) t.diagnostic(`  ${c.verdict} ${c.benchValue ?? '-'}: ${c.reasons.join(' | ').slice(0, 160)}`);
  });

  test('Stop & revoke keys needs confirmation, then stops the run', async () => {
    needId();
    await goTo(h.win, 'Agents');
    await waitForScreen(h.win, 'agents');
    await h.win.getByRole('button', { name: /Stop & revoke keys/ }).click();
    await h.win.getByRole('button', { name: 'Confirm kill switch' }).waitFor();
    await h.win.getByRole('button', { name: 'Cancel' }).click();
    assert.equal(await h.win.getByRole('button', { name: 'Confirm kill switch' }).count(), 0, 'cancel backs out');
    assert.equal((await api()).summary.status, 'running');

    await h.win.getByRole('button', { name: /Stop & revoke keys/ }).click();
    await h.win.getByRole('button', { name: 'Confirm kill switch' }).click();
    await poll('the optimization to stop', async () => ['stopped', 'completed'].includes((await api()).summary.status), { timeout: T.stop, interval: 1000 });
    await poll('the badge to leave "Live"', async () => !/^Live/.test(await badge().innerText()), { timeout: 30_000 });
    await poll('no running agents', async () => (await api()).agents.every((a) => a.status !== 'running'), { timeout: 60_000 });
    assert.equal(await h.win.getByRole('button', { name: /Stop & revoke keys/ }).count(), 0);
    await poll('the sidebar "Live now" card to go away', async () => (await h.win.locator('.live-card').count()) === 0, { timeout: 30_000 });
    wins = await winCount();
  });

  test('Result shows the outcome; Export report works; Open PR only with a verified win', async () => {
    needId();
    await goTo(h.win, 'Result');
    await waitForScreen(h.win, 'result');
    await waitSettled(h.win);
    const d = await api();
    const headline = h.win.locator('[data-testid=headline]');
    await headline.waitFor();
    const text = await headline.innerText();
    assert.ok(text.includes(fmtNum(d.summary.baseline)), 'baseline in the headline');
    assert.ok(text.includes(fmtNum(d.summary.best)), 'best in the headline');
    assert.match(text, new RegExp(`${d.summary.verifiedWins} verified win`));
    assert.match(await h.win.locator('.page-header').innerText(), /Stopped|Completed/);
    // $ value + return on spend (objective.value in the demo config) appear exactly when the run improved the metric.
    assert.equal(await headline.locator('[data-testid=value]').count(), d.summary.best < d.summary.baseline ? 1 : 0, 'value line follows the improvement');

    // Export report: the main process writes it (the e2e build points the save dialog at a temp dir).
    await h.win.getByRole('button', { name: 'Export report' }).click();
    const note = h.win.locator('.ok-note');
    await note.waitFor({ timeout: 30_000 });
    const m = /Report saved to (.+)$/.exec(await note.innerText());
    assert.ok(m, `export note: ${await note.innerText()}`);
    assert.ok(existsSync(m[1]), `report file exists at ${m[1]}`);
    const md = readFileSync(m[1], 'utf8');
    assert.match(md, new RegExp(`# Ratchet report: ${NAME}`));
    assert.match(md, /## Result/);
    assert.match(md, /## Verified wins/);

    const pr = h.win.getByRole('button', { name: /pull request/i });
    if (wins > 0) {
      assert.equal(await pr.isEnabled(), true, 'Open pull request is enabled with a verified win');
      await pr.click();
      await h.win.locator('.ok-note', { hasText: 'Pull request opened:' }).waitFor({ timeout: 60_000 });
      assert.ok((await api()).prUrl, 'the PR url is stored');
      await poll('button relabelled', async () => /Reopen pull request/.test(await pr.innerText()), { timeout: 15_000 });
    } else {
      assert.equal(await pr.isDisabled(), true, 'Open pull request is disabled without a verified win');
      assert.match(await pr.getAttribute('title'), /at least one verified win/);
    }
    await shot('result');
  });

  test('teardown destroys the sandbox and ticks the checklist', async () => {
    needId();
    const destroy = h.win.getByRole('button', { name: 'Destroy sandbox…' });
    assert.equal(await destroy.isEnabled(), true, 'enabled once the run has stopped');
    await destroy.click();
    await h.win.locator('.ok-note', { hasText: 'Sandbox torn down' }).waitFor({ timeout: 90_000 });
    await poll('every teardown item done', async () => {
      const items = h.win.locator('.checklist li');
      return (await items.count()) > 0 && (await items.count()) === (await h.win.locator('.checklist li[data-done=true]').count());
    }, { timeout: 30_000 });
    assert.equal(await destroy.isDisabled(), true);
  });

  test('Optimizations afterwards lists the finished run; Settings still renders', async () => {
    needId();
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    const row = h.win.locator('[data-testid=optimizations-table] tbody tr', { hasText: NAME });
    await row.waitFor();
    assert.match(await row.innerText(), /Stopped|Completed|PR open/);
    assert.equal(await h.win.locator('.live-card').count(), 0);
    await goTo(h.win, 'Settings');
    await waitForScreen(h.win, 'settings');
    await shot('settings');
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    await shot('optimizations-finished');
  });

  test('no console errors or page errors during the whole run', () => {
    assert.deepEqual(h.errors, []);
  });
});
