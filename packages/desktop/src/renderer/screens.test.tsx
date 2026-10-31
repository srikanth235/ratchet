// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { SandboxActivity } from '@ratchet/core';
import type { OptimizationsSnapshot, RatchetApi } from '../shared/types';
import { GH_TOKEN, YAML_OK, receipt } from '../main/test-support';
import { App } from './App';
import { LiveLog } from './components/LiveLog';
import { installApi } from './test-utils';

const nav = (label: string) => fireEvent.click(within(screen.getByRole('navigation', { name: 'Primary' })).getByRole('button', { name: new RegExp(`^${label}( \\d+| \\d+ live)?$`) }));

const rowOf = (name: string) => within(screen.getByRole('button', { name }).closest('tr') as HTMLElement);
const tableNames = () =>
  within(screen.getByTestId('optimizations-table'))
    .getAllByRole('row')
    .slice(1)
    .map((r) => (within(r).getAllByRole('cell')[0] as HTMLElement).firstChild!.textContent);

describe('Overview (real ledger data)', () => {
  it('leads with what was spent, what the wins are worth and what that returns; one staircase, what needs you, the table', async () => {
    installApi();
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Overview', level: 1 })).toBeTruthy();
    // 90 days is the default window: all three optimizations that ran
    const spent = within(await screen.findByTestId('hero-spent'));
    expect(spent.getByText('$143')).toBeTruthy();
    expect(spent.getByText('of $280 budgeted across 3 optimizations')).toBeTruthy();
    const value = within(screen.getByTestId('hero-value'));
    expect(value.getByText('$70.7k')).toBeTruthy();
    expect(value.getByText('from 8 verified wins (estimated)')).toBeTruthy();
    const ret = within(screen.getByTestId('hero-return'));
    expect(ret.getByText(/^\d+×$/).textContent).toBe('495×');
    expect(ret.getByText(/paid back in under a day/)).toBeTruthy();
    expect(screen.getByRole('img', { name: 'Value locked in per year, rose to $70.7k through 8 verified wins' })).toBeTruthy();

    const needs = within(screen.getByTestId('needs-you'));
    expect(needs.getByText('Open a pull request for 2 verified wins')).toBeTruthy();
    expect(needs.getByText('Ingest throughput · +43%')).toBeTruthy();
    expect(needs.getByText('Open a pull request for 1 verified win')).toBeTruthy();
    expect(needs.getByText('Nothing else is waiting on you.')).toBeTruthy();

    expect(tableNames()).toEqual(['search-api p95 latency', 'Ingest throughput', 'Edge worker cold start']);
    const search = rowOf('search-api p95 latency');
    expect(search.getByText('Running')).toBeTruthy();
    expect(search.getByText(/^\dh \d+m left$/)).toBeTruthy();
    expect(search.getByText('−28%')).toBeTruthy();
    expect(search.getByText('182 → 131 ms')).toBeTruthy();
    expect(search.getByText('$61.2k')).toBeTruthy();
    expect(search.getByText('/ 120')).toBeTruthy(); // the budget only while it runs
    const ingest = rowOf('Ingest throughput');
    expect(ingest.getByText('Waiting on you')).toBeTruthy();
    expect(ingest.getByText('+43%')).toBeTruthy();
    expect(ingest.getByText('41.2 → 58.9 k/s')).toBeTruthy();
    expect(ingest.getByText('$7.1k')).toBeTruthy();
    expect(ingest.queryByText(/\/ 100/)).toBeNull();
    // status is a dot and a word: no pills anywhere, and the draft is left to Optimizations
    expect(document.querySelectorAll('main .rt-pill')).toHaveLength(0);
    expect(screen.queryByText('Web render p99')).toBeNull();
    expect(document.body.textContent).not.toMatch(/NaN|undefined/);
  });

  it('the range switch reloads: 30 days leaves out the older optimization and its win', async () => {
    installApi();
    render(<App />);
    expect(await screen.findByRole('button', { name: 'Edge worker cold start' })).toBeTruthy();
    expect(screen.getByRole('radio', { name: '90 days' }).getAttribute('aria-checked')).toBe('true');
    fireEvent.click(screen.getByRole('radio', { name: '30 days' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Edge worker cold start' })).toBeNull());
    const value = within(screen.getByTestId('hero-value'));
    expect(value.getByText('$68.3k')).toBeTruthy();
    expect(value.getByText('from 7 verified wins (estimated)')).toBeTruthy();
    expect(screen.getByRole('radio', { name: '30 days' }).getAttribute('aria-checked')).toBe('true');
    fireEvent.click(screen.getByRole('radio', { name: 'All time' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edge worker cold start' })).toBeTruthy());
    expect(document.body.textContent).not.toMatch(/NaN|undefined/);
  });

  it('hovering a dot of the staircase names the optimization that locked the win in', async () => {
    installApi();
    render(<App />);
    const dots = await waitFor(() => {
      const d = document.querySelectorAll('[data-win]');
      expect(d.length).toBe(8);
      return d;
    });
    expect(screen.queryByRole('tooltip')).toBeNull();
    fireEvent.mouseEnter(dots[0]!);
    expect(screen.getByRole('tooltip').textContent).toBe('$2.4k / yr · Edge worker cold start');
    fireEvent.mouseLeave(dots[0]!);
    fireEvent.mouseEnter(dots[7]!);
    expect(screen.getByRole('tooltip').textContent).toMatch(/^\$70\.7k \/ yr · search-api p95 latency$/);
  });

  it("a needs-you item opens that optimization's Result; rows open Agents (running) or Result; New optimization opens the form", async () => {
    installApi();
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Review: Open a pull request for 2 verified wins' }));
    expect(await screen.findByRole('heading', { name: 'Ingest throughput', level: 1 })).toBeTruthy();
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('result');
    nav('Overview');
    fireEvent.click(await screen.findByRole('button', { name: 'search-api p95 latency' }));
    await waitFor(() => expect(screen.getByRole('main').getAttribute('data-screen')).toBe('agents'));
    nav('Overview');
    fireEvent.click(await screen.findByRole('button', { name: 'New optimization' }));
    expect(await screen.findByRole('heading', { name: 'New optimization' })).toBeTruthy();
  });

  it('without a value model it shows dashes and a quiet hint instead of numbers it cannot know', async () => {
    const h = installApi();
    window.ratchet.overview = async (r) => ({ ...(await h.service.overview(r)), value: null, returnOnSpend: null, wins: [] });
    render(<App />);
    const value = within(await screen.findByTestId('hero-value'));
    expect(value.getByText('—')).toBeTruthy();
    expect(value.getByTestId('set-a-value').textContent).toBe('Set a value to see what the wins are worth');
    expect(within(screen.getByTestId('hero-return')).getByText('—')).toBeTruthy();
    expect(screen.getByText(/Once an optimization has a value/)).toBeTruthy();
    expect(screen.queryByTestId('value-stair')).toBeNull();
    expect(within(screen.getByTestId('hero-spent')).getByText('$143')).toBeTruthy(); // spend still adds up
    expect(document.body.textContent).not.toMatch(/NaN|undefined/);
  });

  it('with a value model but no win in the range it says so', async () => {
    const h = installApi();
    window.ratchet.overview = async (r) => ({ ...(await h.service.overview(r)), value: { usdPerYear: 0, wins: 0, basis: 'estimated' }, returnOnSpend: null, wins: [] });
    render(<App />);
    expect(await screen.findByText('no verified wins in this range')).toBeTruthy();
    expect(screen.getByText('No verified wins in this range yet.')).toBeTruthy();
    expect(within(screen.getByTestId('hero-return')).getByText('needs a verified win and some spend')).toBeTruthy();
  });

  it('with nothing to wait for it says so; with no optimizations it shows one calm empty state with New optimization', async () => {
    const h = installApi();
    window.ratchet.overview = async (r) => ({ ...(await h.service.overview(r)), needsYou: [] });
    const { unmount } = render(<App />);
    expect(await screen.findByText('Nothing is waiting on you.')).toBeTruthy();
    unmount();
    window.ratchet.overview = async (r) => ({ ...(await h.service.overview(r)), empty: true, rows: [], needsYou: [], wins: [], value: null, returnOnSpend: null });
    render(<App />);
    expect(await screen.findByText('No optimizations yet')).toBeTruthy();
    expect(screen.queryByTestId('hero-spent')).toBeNull();
    fireEvent.click(within(screen.getByTestId('no-optimizations')).getByRole('button', { name: 'New optimization' }));
    expect(await screen.findByRole('heading', { name: 'New optimization' })).toBeTruthy();
  });

  it('a range with nothing in it keeps the numbers and says nothing ran', async () => {
    const h = installApi();
    window.ratchet.overview = async (r) => ({ ...(await h.service.overview(r)), rows: [], wins: [], spent: { usd: 0, budgetUsd: 0, optimizations: 0 }, value: { usdPerYear: 0, wins: 0, basis: 'estimated' }, returnOnSpend: null });
    render(<App />);
    expect(await screen.findByText('Nothing ran in this range.')).toBeTruthy();
    expect(within(screen.getByTestId('hero-spent')).getByText('nothing spent in this range')).toBeTruthy();
  });
});

describe('Optimizations (the calm list)', () => {
  it('lists every optimization, drafts included, in the same table; All / Running / Done filter it', async () => {
    installApi();
    render(<App initialRoute="optimizations" />);
    expect(await screen.findByRole('heading', { name: 'Optimizations', level: 1 })).toBeTruthy();
    await waitFor(() => expect(tableNames()).toHaveLength(4));
    expect(tableNames()).toEqual(['search-api p95 latency', 'Ingest throughput', 'Web render p99', 'Edge worker cold start']);
    expect(screen.getByRole('radio', { name: 'All' }).getAttribute('aria-checked')).toBe('true');
    expect(rowOf('Ingest throughput').getByText('Waiting on you')).toBeTruthy();
    expect(screen.getByText('Web render p99').closest('tr')!.textContent).toMatch(/Draft/);
    fireEvent.click(screen.getByRole('radio', { name: 'Running' }));
    expect(tableNames()).toEqual(['search-api p95 latency']);
    fireEvent.click(screen.getByRole('radio', { name: 'Done' }));
    expect(tableNames()).toEqual(['Ingest throughput', 'Edge worker cold start']); // the draft is neither
    // the old dashboard is gone: no KPI cards, no featured card, no activity feed
    expect(document.querySelector('.kpi, [data-testid=featured], [data-testid=activity]')).toBeNull();
    expect(document.querySelectorAll('main .rt-pill')).toHaveLength(0);
  });

  it('a finished optimization opens its Result, a running one Agents, and New optimization opens the form', async () => {
    installApi();
    render(<App initialRoute="optimizations" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Ingest throughput' }));
    expect(await screen.findByRole('heading', { name: 'Ingest throughput', level: 1 })).toBeTruthy();
    expect((await screen.findAllByText('41.2 k/s')).length).toBeGreaterThan(0);
    nav('Optimizations');
    fireEvent.click(await screen.findByRole('button', { name: 'search-api p95 latency' }));
    await waitFor(() => expect(screen.getByRole('main').getAttribute('data-screen')).toBe('agents'));
    nav('Optimizations');
    fireEvent.click(await screen.findByRole('button', { name: /New optimization/ }));
    expect(await screen.findByRole('heading', { name: 'New optimization' })).toBeTruthy();
  });

  it('shows one calm empty state for a fresh ledger', async () => {
    const h = installApi();
    window.ratchet.overview = async (r) => ({ ...(await h.service.overview(r)), empty: true, rows: [] });
    render(<App initialRoute="optimizations" />);
    expect(await screen.findByText('No optimizations yet')).toBeTruthy();
    expect(screen.getByText('Create one to start a baseline run.')).toBeTruthy();
    expect(screen.queryByRole('radio', { name: 'Running' })).toBeNull();
    fireEvent.click(within(screen.getByTestId('no-optimizations')).getByRole('button', { name: 'New optimization' }));
    expect(await screen.findByRole('heading', { name: 'New optimization' })).toBeTruthy();
  });
});

describe('Sidebar and the sandbox line', () => {
  const activity = (over: Partial<SandboxActivity> = {}): SandboxActivity => ({
    sandboxId: 'ratchet-sandbox', status: 'applied', region: 'eu-west-2', auto: true, activeRuns: [], instancesRunning: 2, spendUsd: 1.2, idleSince: 5_000, destroyAt: new Date(2026, 9, 30, 15, 40, 0).getTime(), lastReceipt: null, ...over,
  });

  it('has the logo and four places: Overview, Optimizations, Benchmarks, Connectors (the settings route); nothing else', async () => {
    installApi();
    render(<App />);
    const labels = within(screen.getByRole('navigation', { name: 'Primary' })).getAllByRole('button').map((b) => b.textContent);
    expect(labels).toEqual(['Overview', 'Optimizations', 'Benchmarks', 'Connectors']);
    expect(document.querySelector('.brand__name')!.textContent).toBe('Ratchet');
    expect(document.querySelectorAll('.integration, .workspace, .user, .live-card')).toHaveLength(0);
    expect(screen.getByRole('button', { name: 'Overview' }).getAttribute('aria-current')).toBe('page');
    nav('Connectors');
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('settings');
    expect(screen.getByRole('button', { name: 'Connectors' }).getAttribute('aria-current')).toBe('page');
    nav('Benchmarks');
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('benchmarks');
  });

  it('screens of one optimization (Agents, Result) and New optimization stay under Optimizations', async () => {
    installApi();
    render(<App initialRoute="result" />);
    await screen.findByRole('heading', { level: 1 });
    expect(screen.getByRole('button', { name: 'Optimizations' }).getAttribute('aria-current')).toBe('page');
    expect(screen.getByRole('button', { name: 'Overview' }).getAttribute('aria-current')).toBeNull();
  });

  it('says "No sandbox running" when there is none; the panel says nothing is billing', async () => {
    installApi();
    render(<App />);
    const status = within(screen.getByTestId('sandbox-status'));
    expect(await status.findByText('No sandbox running')).toBeTruthy();
    fireEvent.click(status.getByRole('button', { name: /No sandbox running/ }));
    expect(screen.getByTestId('no-environment').textContent).toMatch(/nothing is billing/);
    expect(screen.queryByRole('button', { name: 'End now' })).toBeNull();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByTestId('sandbox-panel')).toBeNull();
  });

  it('shows the running sandbox in one line: name, cost so far and when it goes off; End now waits for the runs to end', async () => {
    const h = installApi({ engine: { sandboxActivity: async () => activity({ activeRuns: ['opt_demo_search'], destroyAt: null }) } });
    h.ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    render(<App />);
    const line = await within(screen.getByTestId('sandbox-status')).findByRole('button', { name: /Sandbox on/ });
    await waitFor(() => expect(line.textContent).toBe('Sandbox on · ratchet-sandbox$1.20 so far · off after the last run'));
    fireEvent.click(line);
    const env = within(screen.getByTestId('environment'));
    expect(env.getByText('eu-west-2')).toBeTruthy();
    expect(env.getByText('Runs active').nextSibling!.textContent).toBe('1');
    expect((env.getByRole('button', { name: 'End now' }) as HTMLButtonElement).disabled).toBe(true);
    expect(env.getByText('Available once the active runs have ended.')).toBeTruthy();
  });

  it('an idle sandbox says when it goes off; End now asks for a confirmation, then the receipt it produced appears', async () => {
    let alive = true;
    const r = receipt({ trigger: 'manual' });
    const h = installApi({
      engine: {
        sandboxActivity: async () => (alive ? activity() : activity({ status: 'destroyed', lastReceipt: r })),
        endSandbox: async () => ((alive = false), h.ledger.setSandboxStatus('ratchet-sandbox', 'destroyed'), r),
      },
    });
    h.ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    const end = vi.spyOn(window.ratchet, 'endSandbox');
    render(<App />);
    const line = await within(screen.getByTestId('sandbox-status')).findByRole('button', { name: /Sandbox idle/ });
    await waitFor(() => expect(line.textContent).toMatch(/^Sandbox idle · ratchet-sandbox\$1\.20 so far · off at 15:40$/));
    fireEvent.click(line);
    fireEvent.click(within(screen.getByTestId('environment')).getByRole('button', { name: 'End now' }));
    expect(end).not.toHaveBeenCalled(); // behind a confirm
    fireEvent.click(within(screen.getByRole('group', { name: 'Confirm end sandbox' })).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('group', { name: 'Confirm end sandbox' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'End now' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm end sandbox' }));
    await waitFor(() => expect(end).toHaveBeenCalledWith('ratchet-sandbox'));
    const rec = within(await screen.findByTestId('receipt'));
    expect(rec.getByText('Nothing left running')).toBeTruthy();
    expect(rec.getByText(/ended by you/)).toBeTruthy();
    expect(screen.queryByTestId('environment')).toBeNull();
    await waitFor(() => expect(within(screen.getByTestId('sandbox-status')).getByRole('button', { name: /No sandbox running/ })).toBeTruthy());
  });

  it('shows a refusal from the engine, and Details opens the full Environments screen', async () => {
    const h = installApi({
      engine: {
        sandboxActivity: async () => activity(),
        endSandbox: async () => { throw new Error('sandbox_in_use: a run is active'); },
      },
    });
    h.ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    render(<App />);
    fireEvent.click(await within(screen.getByTestId('sandbox-status')).findByRole('button', { name: /Sandbox idle/ }));
    fireEvent.click(screen.getByRole('button', { name: 'End now' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm end sandbox' }));
    expect((await screen.findByRole('alert')).textContent).toMatch(/sandbox_in_use/);
    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('sandbox');
  });
});

describe('New optimization', () => {
  const signedIn = async (h: ReturnType<typeof installApi>) => {
    await h.service.setSecret('githubToken', GH_TOKEN);
    await h.service.setAwsProfile('default');
  };
  const profiles = { profiles: [{ name: 'default', kind: 'keys' as const, source: 'credentials' as const, ssoSession: null }], ssoSessions: [], filesFound: true };
  const pickRepo = async (name: string) => {
    fireEvent.click(await screen.findByRole('button', { name: new RegExp(name) }));
  };

  it('starts at the repo step: a searchable picker, Local folder, and the demos; no yaml editor', async () => {
    installApi();
    render(<App initialRoute="new" />);
    expect(await screen.findByRole('heading', { name: 'New optimization' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Repo', level: 2 })).toBeTruthy();
    expect(screen.getByRole('button', { name: /Local folder/ })).toBeTruthy();
    const demos = within(await screen.findByRole('group', { name: 'Demos' }));
    expect(demos.getAllByRole('button').map((b) => b.querySelector('.agent-choice__name')!.textContent)).toEqual(['Search API latency', 'CLI cold start', 'SKU demand forecast']);
    expect(screen.queryByLabelText('ratchet.yaml')).toBeNull();
    expect(screen.queryByRole('radio', { name: 'Backtest' })).toBeNull();
    expect(screen.queryByLabelText('New guardrail')).toBeNull();
    expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(true);
    // not signed in: the picker says so instead of failing
    expect(screen.getByRole('status').textContent).toMatch(/not signed in to GitHub/);
    expect((screen.getByLabelText('Search repositories') as HTMLInputElement).disabled).toBe(true);
  });

  it('searches the signed-in account repos, reads ratchet.yaml from the default branch and reviews what the repo declares', async () => {
    const h = installApi({ awsProfiles: profiles });
    await signedIn(h);
    h.github.files.set('acme/web@trunk:ratchet.yaml', YAML_OK);
    render(<App initialRoute="new" />);
    const list = within(await screen.findByRole('list', { name: 'Repositories' }));
    expect(list.getAllByRole('button')).toHaveLength(2);
    fireEvent.change(screen.getByLabelText('Search repositories'), { target: { value: 'web' } });
    await waitFor(() => expect(list.getAllByRole('button')).toHaveLength(1));
    fireEvent.click(list.getByRole('button', { name: /acme\/web/ }));
    const found = within(await screen.findByTestId('repo-found'));
    expect(found.getByText(/Found/).textContent).toMatch(/ratchet\.yaml on trunk of acme\/web: http_req_duration\.p95 \(lower is better\)/);
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));

    expect(await screen.findByRole('heading', { name: 'Review', level: 2 })).toBeTruthy();
    const facts = within(screen.getByLabelText('What will run'));
    expect(facts.getByText('http_req_duration.p95').closest('.fact')!.textContent).toMatch(/lower is better.*evaluator template http-latency.*hidden holdout/s);
    expect(facts.getByText('Toolchain').closest('.fact')!.textContent).toContain('.devcontainer/Dockerfile');
    expect(facts.getByText('Machine').closest('.fact')!.textContent).toMatch(/c7i\.4xlarge.*eu-west-2/s);
    expect(facts.getByText('Calibration').closest('.fact')!.textContent).toMatch(/Noise is measured during the baseline/);
    // the protected set is read-only: automatic entries plus the repo's own, each with a reason
    const prot = within(screen.getByLabelText('Protected paths'));
    expect(prot.getByText('ratchet.yaml')).toBeTruthy();
    expect(prot.getByText('docs/**').getAttribute('title')).toBeTruthy();
    expect(prot.getByText('.devcontainer/Dockerfile')).toBeTruthy();
    // the yaml is a read-only disclosure
    expect(screen.queryByRole('textbox', { name: /ratchet\.yaml/ })).toBeNull();
    fireEvent.click(screen.getByText('View ratchet.yaml'));
    expect(screen.getByLabelText('ratchet.yaml (read-only)').textContent).toContain('template: http-latency');
  });

  it('pre-selects the detected, signed-in agents and defaults "Where agents run" to This Mac', async () => {
    const h = installApi({ awsProfiles: profiles });
    await signedIn(h);
    h.github.files.set('acme/api@main:ratchet.yaml', YAML_OK);
    render(<App initialRoute="new" />);
    await pickRepo('acme/api');
    await waitFor(() => expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));
    const agents = within(await screen.findByRole('group', { name: 'Agents' }));
    await waitFor(() => expect(agents.getByRole('button', { name: /Claude Code/ }).getAttribute('aria-pressed')).toBe('true'));
    expect(agents.getByRole('button', { name: /Claude Code/ }).textContent).toContain('v2.1.4 · signed in');
    expect(agents.getByRole('button', { name: /Codex/ }).getAttribute('aria-pressed')).toBe('false');
    expect(agents.getByRole('button', { name: /Codex/ }).textContent).toContain('not installed');
    const where = within(screen.getByRole('radiogroup', { name: 'Where agents run' }));
    expect(where.getByRole('radio', { name: 'This Mac' }).getAttribute('aria-checked')).toBe('true');
    fireEvent.click(where.getByRole('radio', { name: 'Cloud (unattended)' }));
    // cloud runners need API keys: the short "what's missing" list says so
    await waitFor(() => expect(within(screen.getByTestId('missing')).getByText('Anthropic API key')).toBeTruthy());
    expect((screen.getByRole('button', { name: 'Continue to budget' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(where.getByRole('radio', { name: 'This Mac' }));
    await waitFor(() => expect(within(screen.getByTestId('missing')).getByText(/Everything this run needs is in place/)).toBeTruthy());
  });

  it('lists what is missing on the Review step instead of showing a settings form, and links to Settings', async () => {
    installApi({ deps: { pickFolder: async () => '/Users/me/api', inspectLocal: async (p) => ({ path: p, repo: 'acme/api', baseRef: 'main', configYaml: YAML_OK, problem: null }) } });
    render(<App initialRoute="new" />);
    fireEvent.click(await screen.findByRole('button', { name: /Local folder/ }));
    await waitFor(() => expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));
    const missing = within(await screen.findByTestId('missing'));
    await waitFor(() => expect(missing.getAllByRole('listitem').map((li) => li.textContent)).toEqual([
      expect.stringMatching(/^Sign in with GitHub/),
      expect.stringMatching(/^Connect AWS/),
    ]));
    expect(screen.queryByLabelText('AWS profile')).toBeNull();
    expect(screen.queryByLabelText(/API key/)).toBeNull();
    expect((screen.getByRole('button', { name: 'Continue to budget' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(missing.getByRole('button', { name: 'Open Settings' }));
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('settings');
  });

  it('offers the templates when the repo has no ratchet.yaml, opens the pull request and shows its link', async () => {
    const seen: unknown[] = [];
    const h = installApi({
      awsProfiles: profiles,
      engine: { proposeTemplate: async (i) => (seen.push(i), { prUrl: 'https://github.com/acme/api/pull/7', branch: 'ratchet/template-http-latency', sha: 'abc1234', templateId: i.templateId, files: ['ratchet.yaml', 'bench/http-latency.mjs', 'RATCHET.md'] }) },
    });
    await signedIn(h);
    render(<App initialRoute="new" />);
    await pickRepo('acme/api');
    const offer = within(await screen.findByTestId('template-offer'));
    expect(offer.getByText(/has no/).textContent).toMatch(/acme\/api has no ratchet\.yaml on main/);
    expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(offer.getByRole('button', { name: 'Add HTTP latency via pull request' }));
    const pr = within(await screen.findByTestId('pr-result'));
    expect(pr.getByRole('link').getAttribute('href')).toBe('https://github.com/acme/api/pull/7');
    expect(pr.getByText(/ratchet\.yaml, bench\/http-latency\.mjs, RATCHET\.md/)).toBeTruthy();
    expect(seen[0]).toMatchObject({ repoUrl: 'acme/api', baseRef: 'main', templateId: 'http-latency' });
    // after merging: check again
    h.github.files.set('acme/api@main:ratchet.yaml', YAML_OK);
    fireEvent.click(screen.getByRole('button', { name: /I merged it/ }));
    await waitFor(() => expect(screen.getByTestId('repo-found').textContent).toMatch(/Found/));
    expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(false);
  });

  it('shows why an invalid ratchet.yaml cannot be used', async () => {
    const h = installApi({ awsProfiles: profiles });
    await signedIn(h);
    h.github.files.set('acme/api@main:ratchet.yaml', 'version: 3\n');
    render(<App initialRoute="new" />);
    await pickRepo('acme/api');
    expect((await screen.findByTestId('repo-found')).textContent).toMatch(/does not parse/);
    expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('a local folder is read through the dialog; one without a GitHub remote says why it cannot be used', async () => {
    let path = '/Users/me/api';
    const h = installApi({
      awsProfiles: profiles,
      deps: {
        pickFolder: async () => path,
        inspectLocal: async (p) => (p === '/Users/me/api' ? { path: p, repo: 'acme/api', baseRef: 'main', configYaml: YAML_OK, problem: null } : { path: p, repo: null, baseRef: 'main', configYaml: null, problem: 'This folder has no GitHub remote (origin).' }),
      },
    });
    await signedIn(h);
    render(<App initialRoute="new" />);
    fireEvent.click(await screen.findByRole('button', { name: /Local folder/ }));
    await waitFor(() => expect(screen.getByTestId('repo-found').textContent).toMatch(/Found.*acme\/api/));
    path = '/Users/me/scratch';
    fireEvent.click(screen.getByRole('button', { name: /Local folder/ }));
    await waitFor(() => expect(screen.getByTestId('repo-found').textContent).toMatch(/no GitHub remote/));
    expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('a demo goes review -> budget & time (defaults $50 / 2 h) -> Start, creates it and lands on Agents', async () => {
    const h = installApi();
    const created: unknown[] = [];
    const real = h.service.createOptimization.bind(h.service);
    window.ratchet.createOptimization = async (input) => (created.push(input), real(input));
    const start = vi.spyOn(window.ratchet, 'startOptimization');
    render(<App initialRoute="new" />);
    fireEvent.click(await screen.findByRole('button', { name: /Search API latency/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));
    expect(await screen.findByRole('heading', { name: 'Review', level: 2 })).toBeTruthy();
    expect(within(screen.getByLabelText('What will run')).getByText(/mock AWS/)).toBeTruthy();
    expect(within(screen.getByTestId('missing')).getByText(/Everything this run needs is in place/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Continue to budget' }));
    expect(await screen.findByRole('heading', { name: 'Budget & time', level: 2 })).toBeTruthy();
    expect(screen.getByText('Budget · $50')).toBeTruthy();
    expect((screen.getByLabelText('Time limit') as HTMLSelectElement).value).toBe('7200');
    fireEvent.change(screen.getByLabelText('Budget'), { target: { value: '75' } });
    fireEvent.click(screen.getByRole('button', { name: 'Start' }));
    expect(await screen.findByRole('heading', { name: 'Search API latency (demo)', level: 1 })).toBeTruthy();
    expect(created[0]).toMatchObject({ demo: 'demo-search', agentsWhere: 'local', budgetUsd: 75, timeLimitSeconds: 7200, plateauEvals: 25 });
    expect(JSON.stringify(created[0])).not.toMatch(/configYaml|configPath/);
    expect(start).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('agents');
  });

  it('a real run is created for owner/name on the default branch with the chosen place for the agents', async () => {
    const h = installApi({ awsProfiles: profiles, engine: { createOptimization: async () => 'opt_demo_search' } });
    await signedIn(h);
    h.github.files.set('acme/api@main:ratchet.yaml', YAML_OK);
    const create = vi.spyOn(h.service, 'createOptimization');
    window.ratchet.createOptimization = create as unknown as RatchetApi["createOptimization"];
    render(<App initialRoute="new" />);
    await pickRepo('acme/api');
    await waitFor(() => expect((screen.getByRole('button', { name: 'Continue to review' }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));
    await waitFor(() => expect(within(screen.getByTestId('missing')).getByText(/Everything this run needs/)).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Continue to budget' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Start' }));
    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(create.mock.calls[0]![0]).toMatchObject({ repoUrl: 'acme/api', baseRef: 'main', agents: ['claude-code'], agentsWhere: 'local', budgetUsd: 50, timeLimitSeconds: 7200, name: 'Checkout latency' });
  });

  it('shows the error when creation fails and stays on the page', async () => {
    installApi({ app: { createDemo: async () => { throw new Error('Not ready: Sign in with GitHub (Settings)'); } } });
    render(<App initialRoute="new" />);
    fireEvent.click(await screen.findByRole('button', { name: /CLI cold start/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Continue to budget' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Start' }));
    expect((await screen.findByRole('alert')).textContent).toMatch(/Not ready: Sign in with GitHub/);
    expect((screen.getByRole('button', { name: 'Start' }) as HTMLButtonElement).disabled).toBe(false);
  });

  it('can go back through the steps, and a changed source resets how far you got', async () => {
    installApi();
    render(<App initialRoute="new" />);
    fireEvent.click(await screen.findByRole('button', { name: /Search API latency/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue to review' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Continue to budget' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Back' }));
    expect(screen.getByRole('heading', { name: 'Review', level: 2 })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('heading', { name: 'Repo', level: 2 })).toBeTruthy();
    fireEvent.click(await screen.findByRole('button', { name: /CLI cold start/ }));
    expect((within(screen.getByRole('navigation', { name: 'Steps' })).getByRole('button', { name: /Budget & time/ }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('Live agents', () => {
  it('renders one card per agent with hypothesis, stats, log and bench pipeline', async () => {
    installApi();
    render(<App initialRoute="agents" />);
    const cards = await screen.findAllByRole('generic', { name: /agent$/ });
    expect(cards).toHaveLength(2);
    const cc = within(screen.getByLabelText('Claude Code agent'));
    expect(cc.getByTestId('hypothesis').textContent).toBe('“Binary-search the sorted vocabulary for prefix terms”');
    expect(cc.getByText(/Recent evals/)).toBeTruthy();
    await waitFor(() => expect(cc.getAllByText(/Ratchet-Candidate/).length).toBeGreaterThan(0));
    expect(screen.getByText('Trusted evaluator')).toBeTruthy();
    expect(screen.getByText(/Confirm on hidden trace/)).toBeTruthy();
  });

  it('message / pause / resume are wired to the engine', async () => {
    const h = installApi();
    render(<App initialRoute="agents" />);
    const card = within(await screen.findByLabelText('Claude Code agent'));
    const msg = vi.spyOn(window.ratchet, 'messageAgent');
    const pause = vi.spyOn(window.ratchet, 'pauseAgent');
    const resume = vi.spyOn(window.ratchet, 'resumeAgent');
    const agent = h.ledger.listAgents('opt_demo_search').find((a) => a.kind === 'claude-code')!;

    fireEvent.click(card.getByRole('button', { name: /Message/ }));
    fireEvent.change(card.getByLabelText('Message Claude Code'), { target: { value: 'try a bloom filter' } });
    fireEvent.click(card.getByRole('button', { name: /Send/ }));
    await waitFor(() => expect(msg).toHaveBeenCalledWith(agent.id, 'try a bloom filter'));
    await waitFor(() => expect(card.getByRole('status').textContent).toContain('try a bloom filter'));

    fireEvent.click(card.getByRole('button', { name: /Pause/ }));
    await waitFor(() => expect(pause).toHaveBeenCalledWith(agent.id));
    fireEvent.click(await card.findByRole('button', { name: /Resume/ }));
    await waitFor(() => expect(resume).toHaveBeenCalledWith(agent.id));
    expect(h.ledger.getAgent(agent.id)!.status).toBe('running');
  });

  it('Stop needs a confirmation, then fires the kill switch', async () => {
    const h = installApi();
    render(<App initialRoute="agents" />);
    const stop = vi.spyOn(window.ratchet, 'stopOptimization');
    fireEvent.click(await screen.findByRole('button', { name: /Stop & revoke keys/ }));
    expect(stop).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Confirm kill switch' }));
    await waitFor(() => expect(stop).toHaveBeenCalledWith('opt_demo_search', expect.any(String)));
    await waitFor(() => expect(h.ledger.getOptimization('opt_demo_search')!.status).toBe('stopped'));
  });
});

describe('LiveLog streaming', () => {
  it('loads the newest page, then appends only events after the cursor when the engine reports activity', async () => {
    const pages = [
      { events: [{ id: 1, at: 0, type: 'status', text: 'online' }, { id: 2, at: 0, type: 'message', text: 'thinking' }], cursor: 2 },
      { events: [{ id: 3, at: 0, type: 'verdict', text: 'verified: ok' }], cursor: 3 },
      { events: [], cursor: 3 },
    ];
    const agentEvents = vi.fn(async (_id: string, _cursor?: number) => pages.shift() ?? { events: [], cursor: 3 });
    let push: (e: unknown) => void = () => undefined;
    window.ratchet = { agentEvents, onEvent: (l: (e: unknown) => void) => ((push = l), () => undefined) } as never;
    render(<LiveLog agentId="agt1" pollMs={60_000} />);
    expect(await screen.findByText('thinking')).toBeTruthy();
    expect(agentEvents.mock.calls[0]).toEqual(['agt1', undefined]);

    act(() => push({ type: 'agent-event', optimizationId: 'o', agentId: 'other' }));
    expect(agentEvents).toHaveBeenCalledTimes(1);
    act(() => push({ type: 'agent-event', optimizationId: 'o', agentId: 'agt1' }));
    expect(await screen.findByText('verified: ok')).toBeTruthy();
    expect(agentEvents.mock.calls[1]).toEqual(['agt1', 2]);
    expect(screen.getByText('online')).toBeTruthy();
  });

  it('surfaces errors', async () => {
    window.ratchet = { agentEvents: async () => { throw new Error('gone'); }, onEvent: () => () => undefined } as never;
    render(<LiveLog agentId="x" pollMs={60_000} />);
    expect(await screen.findByText('gone')).toBeTruthy();
  });
});

describe('Benchmark history', () => {
  it('draws the ratchet chart with noise band and lists every candidate with verdict pills; filters work', async () => {
    installApi();
    render(<App initialRoute="benchmarks" />);
    expect(await screen.findByRole('img', { name: /Ratchet chart/ })).toBeTruthy();
    expect(document.querySelector('[data-testid="noise-band"]')).toBeTruthy();
    expect(screen.getByRole('img', { name: 'Histogram of baseline vs best' })).toBeTruthy();
    const table = within(screen.getByRole('heading', { name: 'Candidates' }).closest('.rt-card') as HTMLElement);
    expect(table.getAllByRole('row')).toHaveLength(12);
    expect(table.getAllByText('Verified win').length).toBe(5);
    expect(table.getByText('Failed confirm')).toBeTruthy();
    expect(table.getByText('Blocked')).toBeTruthy();

    fireEvent.click(table.getByRole('button', { name: 'blocked' }));
    expect(table.getAllByRole('row')).toHaveLength(2);
    expect(table.getByText(/protected path changed/)).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Baseline vs best' })).toBeTruthy();
    expect(screen.getAllByTestId('pct-row').map((r) => r.querySelector('.pct-row__k')!.textContent)).toEqual(['p50', 'p95', 'p99', 'error_rate', 'peak_rss_mb']);
  });

  it('the metric switch lists the objective family and re-plots the chart for the picked metric', async () => {
    installApi();
    render(<App initialRoute="benchmarks" />);
    const sw = within(await screen.findByRole('radiogroup', { name: 'Metric' }));
    expect(sw.getAllByRole('radio').map((b) => b.textContent)).toEqual(['p50', 'p95', 'p99']);
    expect(sw.getByRole('radio', { name: 'p95' }).getAttribute('aria-checked')).toBe('true');
    const hero = () => document.querySelector('.ratchet-card .hero-num')!.textContent;
    expect(hero()).toBe('131 ms');
    fireEvent.click(sw.getByRole('radio', { name: 'p99' }));
    await waitFor(() => expect(hero()).toBe(`${(131 * 1.58).toFixed(0)} ms`));
    expect(screen.getByText(/Best verified · p99/)).toBeTruthy();
    expect(document.querySelector('[data-testid="noise-band"]')).toBeNull();
  });

  it('hides the metric switch when the evaluator reports a single metric of the family', async () => {
    installApi();
    render(<App initialRoute="result" />);
    fireEvent.change(await screen.findByLabelText('Optimization'), { target: { value: 'opt_demo_ingest' } });
    nav('Benchmarks');
    await screen.findByRole('img', { name: /Ratchet chart/ });
    expect(screen.queryByRole('radiogroup', { name: 'Metric' })).toBeNull();
  });
});

describe('Result', () => {
  it('shows headline, wins, spend breakdown and the teardown receipt; PR / export buttons call through', async () => {
    const h = installApi({ engine: { teardownReceipt: async (id) => (id === 'opt_demo_ingest' ? receipt() : null) } });
    render(<App initialRoute="result" />);
    // featured = running search-api; use the picker to inspect the finished ingest run
    fireEvent.change(await screen.findByLabelText('Optimization'), { target: { value: 'opt_demo_ingest' } });
    expect(await screen.findByTestId('headline')).toBeTruthy();
    await waitFor(() => expect(within(screen.getByTestId('headline')).getByText('58.9')).toBeTruthy());
    expect(within(screen.getByTestId('headline')).getByText('+43.0%')).toBeTruthy();
    expect(screen.getByText('Vectorise the parser inner loop')).toBeTruthy();
    expect(within(screen.getByRole('list', { name: 'Spend by category' })).getByText('Model tokens')).toBeTruthy();
    // the teardown checklist is gone: a receipt says what was removed, what it cost, and that nothing is left running
    expect(screen.queryByRole('list', { name: 'Teardown checklist' })).toBeNull();
    expect(screen.queryByRole('button', { name: /Destroy sandbox/ })).toBeNull();
    const rec = within(await screen.findByTestId('receipt'));
    expect(rec.getByText('Nothing left running')).toBeTruthy();
    expect(rec.getByText('Removed 2 instances; nothing is left running.')).toBeTruthy();
    expect(rec.getByLabelText('Removed').textContent).toMatch(/2 instances terminated.*1 agent identity revoked.*1 secret deleted/s);
    expect(rec.getByText('$4.20')).toBeTruthy();

    const pr = vi.spyOn(window.ratchet, 'openPullRequest');
    fireEvent.click(screen.getByRole('button', { name: /Open pull request/ }));
    expect((await screen.findByRole('status')).textContent).toMatch(/Pull request opened: https:\/\/github\.com\/acme\/ingest\/pull\/\d+/);
    expect(pr).toHaveBeenCalledWith('opt_demo_ingest');
    expect(h.settings.prUrl('opt_demo_ingest')).toMatch(/acme\/ingest/);

    const exp = vi.spyOn(window.ratchet, 'exportReport');
    fireEvent.click(screen.getByRole('button', { name: 'Export report' }));
    await waitFor(() => expect(exp).toHaveBeenCalledWith('opt_demo_ingest'));
    expect((await screen.findByRole('status')).textContent).toBe('Export cancelled');
  });

  it('shows where the time went (evaluator spans) and the winning change: squashed diff of the best commit', async () => {
    installApi();
    render(<App initialRoute="result" />);
    const spans = within(await screen.findByTestId('spans'));
    expect(spans.getByRole('heading', { name: 'Where the time went' })).toBeTruthy();
    expect(spans.getByText('p95 request, by span')).toBeTruthy();
    expect(spans.getAllByTestId('span-row').map((r) => r.querySelector('.span-row__name')!.textContent)).toEqual(['parse', 'search', 'serialize', 'wait']);
    const change = within(await screen.findByTestId('winning-change', {}, { timeout: 20_000 }));
    await waitFor(() => expect(change.getAllByTestId('diff-file').length).toBe(2), { timeout: 20_000 });
    expect(change.getByText(/5 verified commits, squashed/)).toBeTruthy();
    expect(change.getByText('src/search.ts')).toBeTruthy();
    expect(change.getByLabelText('Excerpt of src/search.ts')).toBeTruthy();
    const rows = screen.getAllByTestId('ba-row').map((r) => r.textContent);
    expect(rows[1]).toMatch(/^p95182 ms→131 ms−28%$/);
    expect(rows.at(-1)).toMatch(/peak_rss_mb.* ok$/);
  }, 30_000);

  it('explains a missing diff and hides the span panel without span data', async () => {
    installApi();
    render(<App initialRoute="result" />);
    fireEvent.change(await screen.findByLabelText('Optimization'), { target: { value: 'opt_demo_ingest' } });
    await waitFor(() => expect(within(screen.getByTestId('winning-change')).getByText(/No diff available for [0-9a-f]{7}\./)).toBeTruthy());
    expect(screen.queryByTestId('spans')).toBeNull();
    expect(screen.getByRole('heading', { name: 'Verified wins' })).toBeTruthy();
  });

  it('disables Open pull request while the run is live, and says the receipt comes once its environment is removed', async () => {
    installApi();
    render(<App initialRoute="result" />);
    await screen.findByTestId('headline');
    // the run is still live: the PR is not available yet, even though it has verified wins
    expect((screen.getByRole('button', { name: /Open pull request/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('no-receipt').textContent).toMatch(/Available once the run has finished and its environment has been removed/);
  });

  it('a finished run without a receipt yet points at Environments', async () => {
    installApi();
    render(<App initialRoute="result" />);
    fireEvent.change(await screen.findByLabelText('Optimization'), { target: { value: 'opt_demo_ingest' } });
    await waitFor(() => expect(screen.getByTestId('no-receipt').textContent).toMatch(/removed automatically after the idle window/));
    fireEvent.click(screen.getByRole('button', { name: 'Open Environments' }));
    expect(screen.getByRole('main').getAttribute('data-screen')).toBe('sandbox');
  });
});

describe('Environments (read-only)', () => {
  const activity = (over: Partial<SandboxActivity> = {}): SandboxActivity => ({
    sandboxId: 'ratchet-sandbox', status: 'applied', region: 'eu-west-2', auto: true, activeRuns: [], instancesRunning: 2, spendUsd: 1.5, idleSince: 5_000, destroyAt: Date.UTC(2026, 9, 30, 12, 30, 0), lastReceipt: null, ...over,
  });

  it('with nothing running it says so, has no Apply or Destroy, and shows no receipt yet', async () => {
    installApi();
    render(<App initialRoute="sandbox" />);
    expect(await screen.findByRole('heading', { name: 'Environments', level: 1 })).toBeTruthy();
    expect(screen.getByTestId('no-environment').textContent).toMatch(/No environment is running, so nothing is billing/);
    expect(screen.getByTestId('last-receipt').textContent).toMatch(/No environment has been removed yet/);
    expect(screen.getByText('How it fits together')).toBeTruthy();
    for (const name of [/^Plan$/, /Re-plan/, /Apply/, /Destroy sandbox/, /Run plan/]) expect(screen.queryByRole('button', { name })).toBeNull();
    expect(screen.queryByText('OpenTofu plan')).toBeNull();
  });

  it('shows what is running: phase, region, runs, instances, cost so far and when it removes itself', async () => {
    const h = installApi({ engine: { sandboxActivity: async () => activity({ activeRuns: ['opt_demo_search'], destroyAt: null }) } });
    h.ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    render(<App initialRoute="sandbox" />);
    const env = within(await screen.findByTestId('environment'));
    expect(env.getByText('ratchet-sandbox')).toBeTruthy();
    expect(env.getByText('Ready')).toBeTruthy();
    expect(env.getByText('eu-west-2')).toBeTruthy();
    expect(env.getByText('Runs active').nextSibling!.textContent).toBe('1');
    expect(env.getByText('Instances running').nextSibling!.textContent).toBe('2');
    expect(env.getByText('Cost so far').nextSibling!.textContent).toBe('$1.50');
    expect(env.getByText(/Removed automatically 30 min after the last run ends/)).toBeTruthy();
    expect((env.getByRole('button', { name: 'End sandbox now' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('End sandbox now asks for a confirmation, then shows the receipt it produced', async () => {
    let alive = true;
    const r = receipt({ trigger: 'manual' });
    const h = installApi({
      engine: {
        sandboxActivity: async () => (alive ? activity() : activity({ status: 'destroyed', lastReceipt: r })),
        endSandbox: async () => ((alive = false), h.ledger.setSandboxStatus('ratchet-sandbox', 'destroyed'), r),
      },
    });
    h.ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    const end = vi.spyOn(window.ratchet, 'endSandbox');
    render(<App initialRoute="sandbox" />);
    const env = within(await screen.findByTestId('environment'));
    expect(env.getByText('Idle')).toBeTruthy();
    expect(env.getByText(/Removes itself at/)).toBeTruthy();
    fireEvent.click(env.getByRole('button', { name: 'End sandbox now' }));
    expect(end).not.toHaveBeenCalled(); // behind a confirm
    const confirm = within(screen.getByRole('group', { name: 'Confirm end sandbox' }));
    fireEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('group', { name: 'Confirm end sandbox' })).toBeNull();
    fireEvent.click(env.getByRole('button', { name: 'End sandbox now' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm end sandbox' }));
    await waitFor(() => expect(end).toHaveBeenCalledWith('ratchet-sandbox'));
    const rec = within(await screen.findByTestId('receipt'));
    expect(rec.getByText('Nothing left running')).toBeTruthy();
    expect(rec.getByText(/ended by you/)).toBeTruthy();
    expect(rec.getByText('Removed 2 instances; nothing is left running.')).toBeTruthy();
    expect(screen.queryByTestId('environment')).toBeNull();
  });

  it('shows a refusal from the engine', async () => {
    const h = installApi({
      engine: {
        sandboxActivity: async () => activity(),
        endSandbox: async () => { throw new Error('sandbox_in_use: a run is active'); },
      },
    });
    h.ledger.recordSandbox({ id: 'ratchet-sandbox', provider: 'aws', region: 'eu-west-2', status: 'applied' });
    render(<App initialRoute="sandbox" />);
    fireEvent.click(await screen.findByRole('button', { name: 'End sandbox now' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm end sandbox' }));
    expect((await screen.findByRole('alert')).textContent).toMatch(/sandbox_in_use/);
  });
});

describe('Benchmarks: Templates gallery', () => {
  const openGallery = async () => {
    fireEvent.click(await screen.findByRole('radio', { name: 'Templates' }));
    return screen.findByRole('region', { name: 'Evaluator templates' });
  };

  it('is a second view of the Benchmarks tab, next to the history', async () => {
    installApi();
    render(<App initialRoute="benchmarks" />);
    expect(await screen.findByRole('radio', { name: 'Benchmark history' })).toBeTruthy();
    expect((await screen.findAllByTestId('ratchet-card')).length).toBe(1);
    await openGallery();
    expect(screen.getByRole('radio', { name: 'Templates' }).getAttribute('aria-checked')).toBe('true');
    expect(screen.queryByTestId('ratchet-card')).toBeNull();
    fireEvent.click(screen.getByRole('radio', { name: 'Benchmark history' }));
    expect(await screen.findByTestId('ratchet-card')).toBeTruthy();
  });

  it('a card shows the title, what it measures, the holdout and the machine', async () => {
    installApi();
    render(<App initialRoute="benchmarks" />);
    const gallery = within(await openGallery());
    const cards = gallery.getAllByTestId('template-card');
    expect(cards.map((c) => c.getAttribute('data-template'))).toEqual(expect.arrayContaining(['http-latency', 'cli-cold-start', 'test-suite-time', 'build-size', 'memory', 'forecast-backtest', 'llm-eval']));
    const http = within(cards.find((c) => c.getAttribute('data-template') === 'http-latency')!);
    expect(http.getByText('HTTP latency')).toBeTruthy();
    expect(http.getByText(/p50 \/ p95 \/ p99 latency of an open-loop load test/)).toBeTruthy();
    expect(http.getByText('Holdout').nextSibling!.textContent).toBe('hidden request mix');
    expect(http.getByText('Machine').nextSibling!.textContent).toBe('c7i.2xlarge');
    const suite = within(cards.find((c) => c.getAttribute('data-template') === 'test-suite-time')!);
    expect(suite.getByText('Holdout').nextSibling!.textContent).toBe('none');
    expect(http.getByRole('button', { name: 'Use template HTTP latency' })).toBeTruthy();
  });

  it('Use template: repo picker -> pull request -> link', async () => {
    const seen: unknown[] = [];
    const h = installApi({
      engine: { proposeTemplate: async (i) => (seen.push(i), { prUrl: 'https://github.com/acme/web/pull/3', branch: 'ratchet/template-cli-cold-start', sha: 'def5678', templateId: i.templateId, files: ['ratchet.yaml', 'bench/cold-start.mjs', 'RATCHET.md'] }) },
    });
    await h.service.setSecret('githubToken', GH_TOKEN);
    render(<App initialRoute="benchmarks" />);
    await openGallery();
    fireEvent.click(screen.getByRole('button', { name: 'Use template CLI cold start' }));
    const panel = within(await screen.findByTestId('use-template'));
    expect((panel.getByRole('button', { name: 'Open pull request' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(await panel.findByRole('button', { name: /acme\/web/ }));
    expect(panel.getByText(/Target:/).textContent).toContain('acme/web (trunk)');
    fireEvent.click(panel.getByRole('button', { name: 'Open pull request' }));
    const pr = within(await screen.findByTestId('pr-result'));
    expect(pr.getByRole('link').getAttribute('href')).toBe('https://github.com/acme/web/pull/3');
    expect(seen[0]).toMatchObject({ repoUrl: 'acme/web', baseRef: 'trunk', templateId: 'cli-cold-start' });
  });

  it('Use template without a GitHub sign-in points at Settings', async () => {
    installApi();
    render(<App initialRoute="benchmarks" />);
    await openGallery();
    fireEvent.click(screen.getByRole('button', { name: 'Use template HTTP latency' }));
    const panel = within(await screen.findByTestId('use-template'));
    expect(await panel.findByText(/not signed in to GitHub/)).toBeTruthy();
    expect((panel.getByRole('button', { name: 'Open pull request' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('Try a demo lists the demos; running one starts a simulation run and opens Agents', async () => {
    const demos: string[] = [];
    installApi({ app: { createDemo: async (d) => (demos.push(d), 'opt_demo_search') } });
    render(<App initialRoute="benchmarks" />);
    await openGallery();
    const section = within(screen.getByTestId('try-demo'));
    expect(section.getAllByTestId('demo-card').map((c) => c.getAttribute('data-template'))).toEqual(['demo-search', 'demo-cli', 'demo-forecast']);
    expect(section.getByText(/no setup, nothing billed/)).toBeTruthy();
    fireEvent.click(section.getByRole('button', { name: 'Run demo CLI cold start' }));
    await waitFor(() => expect(screen.getByRole('main').getAttribute('data-screen')).toBe('agents'));
    expect(demos).toEqual(['demo-cli']);
  });

  it('shows why a demo could not start', async () => {
    installApi({ app: { createDemo: async () => { throw new Error('the demo project is not built'); } } });
    render(<App initialRoute="benchmarks" />);
    await openGallery();
    fireEvent.click(screen.getByRole('button', { name: 'Run demo Search API latency' }));
    expect((await screen.findByRole('alert')).textContent).toMatch(/not built/);
  });
});
