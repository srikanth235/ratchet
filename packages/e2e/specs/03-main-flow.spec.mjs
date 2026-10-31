// The main flow against the REAL core engine on a demo (New optimization -> "Try a demo" -> review -> budget & time -> Start): a copy
// of examples/demo-search served as the repository, mock AWS with the automatic environment, a local evaluator running a real
// build + tests + load generator, scripted agents applying real patches, on a fresh ledger.
//
// Every wait polls on UI state with generous timeouts; the run takes about 10 minutes. Screenshots of each screen made
// during the real run land in artifacts/app-live/.
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { ARTIFACTS, fmtNum, goTo, launchApp, openNewOptimization, openOptimization, openSandboxPanel, openTab, poll, rpc, screenshotScreen, waitForScreen, waitSettled } from '../harness/launch.mjs';

const NAME = 'Search API latency (demo)'; // the demo's name in the ledger
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
  });

  test('step 1 (Repo): "Try a demo" picks the search demo; nothing else is needed', async () => {
    await openNewOptimization(h.win);
    const demos = h.win.getByRole('group', { name: 'Demos' });
    await demos.getByRole('button', { name: /Search API latency/ }).click();
    assert.equal(await demos.getByRole('button', { name: /Search API latency/ }).getAttribute('aria-pressed'), 'true');
    assert.match(await h.win.locator('.step__head', { hasText: 'Repo' }).innerText(), /Ready/);
    assert.match(await h.win.locator('.review').innerText(), /Repository\s+Search API latency/);
    await h.win.getByRole('button', { name: 'Continue to review' }).click();
  });

  test('step 2 (Review): what is measured, toolchain, machine and calibration; agents and where they run; nothing missing', async () => {
    await h.win.getByRole('heading', { name: 'Review', level: 2 }).waitFor();
    const facts = await h.win.getByLabel('What will run').innerText();
    assert.match(facts, /http_req_duration\.p95\s*↓ lower is better/);
    assert.match(facts, /Toolchain\s+simulation/i);
    assert.match(facts, /Machine\s+mock AWS/i);
    assert.match(facts, /Noise is measured during the baseline/);
    // the repo owns the config: no yaml editor, no evaluator cards, no guardrail ticks
    assert.equal(await h.win.getByLabel('ratchet.yaml', { exact: true }).count(), 0);
    assert.equal(await h.win.locator('main input[name=evaluator]').count(), 0);
    assert.equal(await h.win.locator('.guardrail-list').count(), 0);
    const agents = h.win.getByRole('group', { name: 'Agents' });
    assert.equal(await agents.getByRole('button', { name: /Claude Code/ }).getAttribute('aria-pressed'), 'true');
    assert.equal(await agents.getByRole('button', { name: /Codex/ }).getAttribute('aria-pressed'), 'true');
    assert.match(await agents.innerText(), /scripted demo agent/);
    const where = h.win.getByRole('radiogroup', { name: 'Where agents run' });
    assert.equal(await where.getByRole('radio', { name: 'This Mac' }).getAttribute('aria-checked'), 'true', 'This Mac is the default');
    assert.equal(await where.getByRole('radio', { name: 'Cloud (unattended)' }).isDisabled(), true, 'a demo runs in simulation');
    await h.win.getByTestId('missing').getByText(/Everything this run needs is in place/).waitFor({ timeout: 15_000 });
    assert.equal(await h.win.getByTestId('missing').locator('li').count(), 0, 'no "what is missing" items');
    assert.equal(await h.win.getByRole('button', { name: 'Continue to budget' }).isEnabled(), true);
    assert.deepEqual(h.errors, []);
  });

  test('step 3 (Budget & time): defaults are $50 and 2 hours', async () => {
    await h.win.getByRole('button', { name: 'Continue to budget' }).click();
    await h.win.getByRole('heading', { name: 'Budget & time', level: 2 }).waitFor();
    assert.equal(await h.win.getByLabel('Budget', { exact: true }).inputValue(), '50');
    assert.match(await h.win.locator('.budget').innerText(), /Budget · \$50/);
    assert.equal(await h.win.getByLabel('Time limit').inputValue(), '7200');
    assert.match(await h.win.locator('.review').innerText(), /Limits\s+\$50 · 2 h/);
    assert.equal(await h.win.getByRole('button', { name: 'Start', exact: true }).isEnabled(), true);
    await shot('new-optimization');
    // the steps can be walked back and forward without losing the choice
    await h.win.getByRole('button', { name: 'Back' }).click();
    await h.win.getByRole('heading', { name: 'Review', level: 2 }).waitFor();
    await h.win.getByRole('button', { name: 'Continue to budget' }).click();
    assert.deepEqual(h.errors, []);
  });

  test('Start opens Agents with the new optimization (a simulation run, mock agents)', async () => {
    await h.win.getByRole('button', { name: 'Start', exact: true }).click();
    await waitForScreen(h.win, 'agents');
    assert.equal((await h.win.locator('main h1').first().innerText()).trim(), NAME);
    id = await poll('the optimization in the ledger', async () => (await rpc(h.win, 'listOptimizations')).optimizations[0]?.id, { timeout: T.start });
    const text = await badge().innerText();
    assert.match(text, /Baselining|Live|Draft|Starting/i, `status badge: ${text}`);
    assert.match(await h.win.locator('.page-header .rt-eyebrow').innerText(), /OPTIMIZATIONS\s*\/\s*\S+/i);
    assert.equal((await rpc(h.win, 'getOptimization', id)).summary.budgetUsd, 50);
    assert.equal((await rpc(h.win, 'getOptimization', id)).timeLimitS, 7200);
    assert.equal(await h.win.locator('.live-card').count(), 0, 'the sidebar has no "Live now" card any more');
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
    await poll('the calibration in plain words', async () => /noise|stable|measured/i.test(await h.win.getByTestId('calibration').innerText()), { timeout: 30_000 });
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

    // Metric switch: the demo's bench reports http_req_duration.{p50,p95,p99}; the switch lists exactly those.
    const series = await rpc(h.win, 'ratchetHistory', id);
    assert.deepEqual(series.options.map((o) => o.label), ['p50', 'p95', 'p99']);
    const sw = h.win.getByRole('radiogroup', { name: 'Metric' });
    assert.deepEqual((await sw.getByRole('radio').allInnerTexts()).map((t) => t.trim()), ['p50', 'p95', 'p99']);
    await sw.getByRole('radio', { name: 'p99' }).click();
    await poll('the p99 view', async () => /Best verified · p99/i.test(await rc.innerText()), { timeout: 15_000 });
    const p99 = await rpc(h.win, 'ratchetHistory', id, 'http_req_duration.p99');
    assert.equal(p99.metric, 'http_req_duration.p99');
    assert.equal(p99.steps[0].value, d.baselineStats['metric:http_req_duration.p99'], 'the p99 line starts at the baseline p99');
    await sw.getByRole('radio', { name: 'p95' }).click();
    await poll('back to the objective', async () => !/Best verified · p/i.test(await rc.innerText()), { timeout: 15_000 });
    await shot('benchmarks');
  });

  test('Overview shows the live run: spend, value, its row in the table; the range switch follows', async () => {
    needId();
    await goTo(h.win, 'Overview');
    await waitForScreen(h.win, 'overview');
    await waitSettled(h.win);
    const row = h.win.locator('[data-testid=optimizations-table] tbody tr', { hasText: NAME });
    await row.waitFor();
    assert.match(await row.innerText(), /Running|Baselining/);
    assert.match(await row.innerText(), /\$\d+(\.\d+)? \/ 50/, 'spend / budget while it runs');
    assert.match(await h.win.getByTestId('hero-spent').innerText(), /Spent\s*\$\d/);
    // The demo config has an objective.value, so the value hero is a number (or $0 before the first win), never the "Set a value" hint.
    const o = await rpc(h.win, 'overview', '90d');
    assert.ok(o.value, 'the demo has a value model');
    assert.equal(await h.win.getByTestId('set-a-value').count(), 0);
    assert.match(await h.win.getByTestId('hero-value').innerText(), /Value locked in\s*\$\d/);
    assert.equal(o.rows.length, 1);
    // Range switch: 30 days / 90 days / All time each reload the screen; the live run is in every window.
    for (const [label, range] of [['30 days', '30d'], ['All time', 'all'], ['90 days', '90d']]) {
      await h.win.getByRole('radio', { name: label }).click();
      await poll(`${label} selected`, async () => (await h.win.getByRole('radio', { name: label }).getAttribute('aria-checked')) === 'true', { timeout: 10_000 });
      assert.equal((await rpc(h.win, 'overview', range)).range, range);
      await row.waitFor();
    }
    assert.doesNotMatch(await h.win.locator('main').innerText(), /NaN|undefined/);
    assert.equal(await h.win.locator('main .rt-pill').count(), 0, 'status is a dot and a word, not a pill');
    await shot('overview');
  });

  test('Optimizations lists the live run in the same table; Running keeps it, Done leaves it out', async () => {
    needId();
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    await waitSettled(h.win);
    const row = h.win.locator('[data-testid=optimizations-table] tbody tr', { hasText: NAME });
    await row.waitFor();
    assert.match(await row.innerText(), /Running|Baselining/);
    await h.win.getByRole('radio', { name: 'Done' }).click();
    await h.win.getByText('Nothing has finished yet.').waitFor();
    await h.win.getByRole('radio', { name: 'Running' }).click();
    await row.waitFor();
    await h.win.getByRole('radio', { name: 'All' }).click();
    await shot('optimizations');
  });

  test('The sandbox line shows the automatic environment of the live run; its panel has phase, region, cost, no Apply or Destroy', async () => {
    needId();
    const line = h.win.locator('.sbx__line');
    await poll('the sandbox line to say it is on', async () => /^Sandbox on · ratchet-sim-search/.test((await line.innerText()).trim()), { timeout: 60_000 });
    assert.match(await line.innerText(), /\$\d+\.\d\d so far/);
    const panel = await openSandboxPanel(h.win);
    const env = panel.getByTestId('environment');
    await env.waitFor({ timeout: 30_000 });
    const info = (await rpc(h.win, 'environments')).environments[0];
    assert.match(info.sandboxId, /^ratchet-sim-search/);
    assert.equal(info.simulated, true);
    assert.equal(info.region, 'us-east-1');
    assert.deepEqual(info.activeRuns, [id]);
    assert.equal(await env.getAttribute('data-phase'), 'ready');
    const text = await env.innerText();
    assert.match(text, /ratchet-sim-search/);
    assert.match(text, /us-east-1/);
    assert.match(text, /simulated/);
    assert.match(text, /Runs active\s*1/);
    assert.match(text, /Cost so far\s*\$\d/);
    assert.match(text, /Removed automatically 30 min after the last run ends/);
    assert.equal(await env.getByRole('button', { name: 'End now' }).isDisabled(), true, 'cannot end it while a run is active');
    const buttons = (await panel.getByRole('button').allInnerTexts()).map((t) => t.trim());
    assert.deepEqual(buttons.filter((t) => /plan|apply|destroy sandbox/i.test(t)), []);
    assert.equal(await panel.getByTestId('receipt').count(), 0, 'no receipt while the environment is alive');
    await shot('sandbox');
    await h.win.keyboard.press('Escape');
    assert.equal(await h.win.getByTestId('sandbox-panel').count(), 0);
  });

  test('Open pull request stays disabled while the run is live', async () => {
    needId();
    await openOptimization(h.win, NAME);
    await openTab(h.win, 'Result');
    await waitForScreen(h.win, 'result');
    await waitSettled(h.win);
    assert.equal(await h.win.getByRole('button', { name: /pull request/i }).isDisabled(), true);
    assert.match(await h.win.getByTestId('no-receipt').innerText(), /Available once the run has finished/);
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
    await openOptimization(h.win, NAME); // a running optimization opens its Agents
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
    wins = await winCount();
  });

  test('Result shows the outcome; Export report works; Open PR only with a verified win', async () => {
    needId();
    await openOptimization(h.win, NAME); // a finished optimization opens its Result
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
    assert.match(md, new RegExp(`# Ratchet report: ${NAME.replace(/[()]/g, '\\$&')}`));
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

    // Winning change: the best verified commit diffed against main by the simulated GitHub (LocalGitHost).
    const change = h.win.locator('[data-testid=winning-change]');
    await change.waitFor({ timeout: 30_000 });
    const w = await rpc(h.win, 'winningChange', id);
    if (wins > 0) {
      assert.equal(w.unavailable, null, `diff: ${w.unavailable}`);
      assert.equal(w.commits, wins);
      assert.ok(w.files.length >= 1 && w.files.every((f) => f.path.startsWith('src/')), `files: ${w.files.map((f) => f.path)}`);
      await poll('file rows', async () => (await change.locator('[data-testid=diff-file]').count()) === w.files.length, { timeout: 30_000 });
      assert.match(await change.innerText(), new RegExp(`${wins} verified commits?, squashed`));
      assert.match(await change.innerText(), new RegExp(`\\+${w.additions} −${w.deletions}`));
      // Where the time went: the demo's local bench reports Server-Timing spans of its p95 requests.
      const spans = h.win.locator('[data-testid=spans]');
      await spans.waitFor({ timeout: 15_000 });
      assert.deepEqual((await spans.locator('.span-row__name').allInnerTexts()).map((t) => t.trim()).sort(), ['parse', 'search', 'serialize', 'wait']);
    } else {
      assert.equal(w.unavailable, 'No verified win yet.');
      assert.match(await change.innerText(), /No verified win yet/);
      assert.equal(await h.win.locator('[data-testid=spans]').count(), 0, 'no spans panel without a verified win');
    }
    await shot('result');
  });

  test('the sandbox goes idle after the run; "End now" (behind a confirm) removes it and shows the receipt', async () => {
    needId();
    const panel = await openSandboxPanel(h.win);
    const env = panel.getByTestId('environment');
    await env.waitFor({ timeout: 30_000 });
    await poll('the sandbox to be idle', async () => (await env.getAttribute('data-phase')) === 'idle', { timeout: 2 * MIN, interval: 1000 });
    assert.match(await h.win.locator('.sbx__line').innerText(), /^Sandbox idle · ratchet-sim-search[\s\S]*off at \d\d:\d\d/);
    const end = env.getByRole('button', { name: 'End now' });
    await poll('End now enabled', async () => end.isEnabled(), { timeout: 60_000 });
    assert.match(await env.innerText(), /Removes itself at \d/);
    await end.click();
    const confirm = h.win.getByRole('group', { name: 'Confirm end sandbox' });
    await confirm.waitFor();
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    assert.equal(await h.win.getByRole('group', { name: 'Confirm end sandbox' }).count(), 0);
    assert.equal((await rpc(h.win, 'environments')).environments.length, 1, 'cancel ends nothing');
    await end.click();
    await h.win.getByRole('button', { name: 'Confirm end sandbox' }).click();
    const receipt = panel.getByTestId('receipt');
    await receipt.waitFor({ timeout: 90_000 });
    assert.equal(await h.win.getByTestId('environment').count(), 0, 'the sandbox is gone');
    const text = await receipt.innerText();
    assert.match(text, /Nothing left running/);
    assert.match(text, /ended by you/);
    assert.match(text, /Cost\s+\$\d/i);
    assert.match(text, /instances? terminated/);
    await poll('the sandbox line to say none is running', async () => /No sandbox running/.test(await h.win.locator('.sbx__line').innerText()), { timeout: 30_000 });
    const vm = (await rpc(h.win, 'environments')).lastReceipt;
    assert.equal(vm.trigger, 'manual');
    assert.equal(vm.nothingRunning, true);
    assert.deepEqual(vm.runs.map((r) => r.optimizationId), [id]);
    assert.deepEqual(vm.orphans, []);
    await shot('sandbox-receipt');
    await h.win.keyboard.press('Escape');
    assert.deepEqual(h.errors, []);
  });

  test('Result now shows the teardown receipt instead of a checklist', async () => {
    needId();
    await openOptimization(h.win, NAME);
    await waitForScreen(h.win, 'result');
    await waitSettled(h.win);
    const receipt = h.win.getByTestId('receipt');
    await receipt.waitFor({ timeout: 30_000 });
    assert.match(await receipt.innerText(), /Nothing left running/);
    assert.equal(await h.win.getByRole('list', { name: 'Teardown checklist' }).count(), 0);
    assert.equal(await h.win.getByRole('button', { name: /Destroy sandbox/ }).count(), 0);
    assert.equal((await api()).receipt.sandboxId, (await rpc(h.win, 'environments')).lastReceipt.sandboxId);
    await shot('result-receipt');
  });

  test('Optimizations and Overview afterwards list the finished run, waiting on the user when it has wins and no PR; Connectors still renders', async () => {
    needId();
    await goTo(h.win, 'Optimizations');
    await waitForScreen(h.win, 'optimizations');
    const row = h.win.locator('[data-testid=optimizations-table] tbody tr', { hasText: NAME });
    await row.waitFor();
    const pr = (await api()).prUrl;
    const expected = pr ? /PR open/ : wins > 0 ? /Waiting on you/ : /No win/;
    await poll('the status word of the finished run', async () => expected.test(await row.innerText()), { timeout: 30_000 });
    assert.doesNotMatch(await row.innerText(), /Running|Baselining/);
    await goTo(h.win, 'Overview');
    await waitForScreen(h.win, 'overview');
    const needs = h.win.getByTestId('needs-you');
    if (!pr && wins > 0) {
      await poll('the run in "Needs you"', async () => /verified wins?/.test(await needs.innerText()), { timeout: 30_000 });
      await needs.getByRole('button').first().click();
      await waitForScreen(h.win, 'result');
      await goTo(h.win, 'Overview');
      await waitForScreen(h.win, 'overview');
    } else {
      await poll('nothing waiting', async () => /Nothing is waiting on you/.test(await needs.innerText()), { timeout: 30_000 });
    }
    await shot('overview-finished');
    await goTo(h.win, 'Connectors');
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
