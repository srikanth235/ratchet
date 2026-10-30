// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { SandboxActivity } from '@ratchet/core';
import type { OptimizationsSnapshot, RatchetApi } from '../shared/types';
import { HarnessCache } from '../main/harnesses';
import { ANTHROPIC_KEY, DEVICE_TOKEN, GH_TOKEN, YAML_OK, claude, codex, receipt } from '../main/test-support';
import { App } from './App';
import { LiveLog } from './components/LiveLog';
import { installApi } from './test-utils';

const nav = (label: string) => fireEvent.click(within(screen.getByRole('navigation', { name: 'Primary' })).getByRole('button', { name: new RegExp(`^${label}( \\d+| \\d+ live)?$`) }));

describe('Optimizations (real ledger data)', () => {
  it('shows KPIs, the live optimization, activity and the table', async () => {
    installApi();
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'search-api p95 latency', level: 2 })).toBeTruthy();
    const kpis = within(screen.getByRole('region', { name: 'Key metrics' }));
    expect(kpis.getByText('Verified wins')).toBeTruthy();
    // default range 30d: 7 wins, +6 vs the 30 days before (one win, the edge worker run)
    const wins = within(document.querySelector('[data-kpi="wins"]') as HTMLElement);
    expect(wins.getByText('7')).toBeTruthy();
    expect(wins.getByText('+6')).toBeTruthy();
    expect(within(document.querySelector('[data-kpi="value"]') as HTMLElement).getByText('$61.2k')).toBeTruthy();
    expect(screen.getByTestId('featured').textContent).toContain('≈ $61.2k / year');
    expect(screen.getByRole('img', { name: /Ratchet chart, best down from 182 to 131/ })).toBeTruthy();
    const table = within(screen.getByRole('table'));
    expect(table.getByText('Ingest throughput')).toBeTruthy();
    expect(table.getByText('Web render p99')).toBeTruthy();
    expect(table.getByText('182 ms')).toBeTruthy();
    expect(table.getAllByText('131 ms').length).toBeGreaterThan(0);
    expect(screen.getByText(/Dev engine/)).toBeTruthy();
  });

  it('the range switch reloads the KPIs for that window; no chip without data in the window before', async () => {
    installApi();
    render(<App />);
    const wins = () => document.querySelector('[data-kpi="wins"]') as HTMLElement;
    await waitFor(() => expect(within(wins()).getByText('7')).toBeTruthy());
    fireEvent.click(screen.getByRole('radio', { name: '90d' }));
    await waitFor(() => expect(within(wins()).getByText('8')).toBeTruthy());
    expect(within(wins()).getByText('last 90d')).toBeTruthy();
    expect(document.querySelectorAll('.kpi__badge')).toHaveLength(0);
    expect(screen.getByRole('radio', { name: '90d' }).getAttribute('aria-checked')).toBe('true');
    fireEvent.click(screen.getByRole('radio', { name: '7d' }));
    await waitFor(() => expect(within(wins()).getByText('last 7d')).toBeTruthy());
    expect(document.body.textContent).not.toMatch(/NaN|undefined/);
  });

  it('clicking a finished optimization opens its Result; New optimization opens the form', async () => {
    installApi();
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Ingest throughput' }));
    expect(await screen.findByRole('heading', { name: 'Ingest throughput', level: 1 })).toBeTruthy();
    expect((await screen.findAllByText('41.2 k/s')).length).toBeGreaterThan(0);
    nav('Optimizations');
    fireEvent.click(await screen.findByRole('button', { name: /New optimization/ }));
    expect(await screen.findByRole('heading', { name: 'New optimization' })).toBeTruthy();
  });

  it('shows an empty state for a fresh ledger', async () => {
    const h = installApi();
    window.ratchet.listOptimizations = async () => ({ ...(await h.service.listOptimizations()), optimizations: [], featuredId: null, featuredSeries: null, activity: [] }) as OptimizationsSnapshot;
    render(<App />);
    expect(await screen.findByText('No optimizations yet')).toBeTruthy();
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

describe('Settings: GitHub, AWS, Agents', () => {
  const profiles = {
    profiles: [
      { name: 'default', kind: 'keys' as const, source: 'credentials' as const, ssoSession: null },
      { name: 'corp-sso', kind: 'sso' as const, source: 'config' as const, ssoSession: 'corp' },
    ],
    ssoSessions: ['corp'],
    filesFound: true,
  };

  it('is three rows, and none of the old per-repo knobs', async () => {
    installApi();
    render(<App initialRoute="settings" />);
    expect(await screen.findByRole('heading', { name: 'Settings', level: 1 })).toBeTruthy();
    expect(await screen.findByTestId('row-github')).toBeTruthy();
    expect(screen.getByTestId('row-aws')).toBeTruthy();
    expect(screen.getByTestId('row-agents')).toBeTruthy();
    expect(screen.queryByRole('radio', { name: /Simulation|Live/ })).toBeNull();
    for (const label of [/GitHub repository/, /Runner AMI/, /Bench AMI/, /OpenTofu/]) {
      // the development overrides live in a collapsed disclosure, never in the open
      const hits = screen.queryAllByLabelText(label);
      for (const el of hits) expect(el.closest('details')?.open).toBe(false);
    }
    expect(screen.getByText('Advanced (development)').closest('details')!.open).toBe(false);
  });

  it('Sign in with GitHub: shows the device code, then the signed-in account with a sign-out', async () => {
    const h = installApi();
    h.github.clientIdConfigured = false;
    h.github.polls = [{ status: 'pending' }, { status: 'ok', token: DEVICE_TOKEN }];
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-github'));
    expect(await row.findByText('Not signed in')).toBeTruthy();
    expect(row.getByText(/built-in placeholder OAuth client id/)).toBeTruthy();
    fireEvent.click(row.getByRole('button', { name: 'Sign in with GitHub' }));
    expect((await row.findByTestId('device-code')).textContent).toBe('ABCD-1234');
    expect(h.opened).toEqual(['https://github.com/login/device']);
    expect(row.getByText(/Waiting for you to approve/)).toBeTruthy();
    await waitFor(() => expect(row.getByText('Signed in as octocat')).toBeTruthy(), { timeout: 10_000 });
    expect(row.queryByTestId('device-code')).toBeNull();
    expect(row.getByText(/token is kept in your OS keychain/)).toBeTruthy();
    expect(document.body.textContent).not.toContain(DEVICE_TOKEN);
    fireEvent.click(row.getByRole('button', { name: 'Sign out' }));
    expect(await row.findByText('Not signed in')).toBeTruthy();
    expect(h.settings.hasSecret('githubToken')).toBe(false);
  }, 20_000);

  it('a denied sign-in is explained', async () => {
    const h = installApi();
    h.github.polls = [{ status: 'denied' }];
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-github'));
    fireEvent.click(await row.findByRole('button', { name: 'Sign in with GitHub' }));
    await waitFor(() => expect(row.getByRole('alert').textContent).toMatch(/denied/), { timeout: 10_000 });
  }, 20_000);

  it('pasting a token is an Advanced fallback, checked before it is kept', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-github'));
    const adv = row.getByText('Advanced: paste a token instead').closest('details')!;
    expect(adv.open).toBe(false);
    fireEvent.click(row.getByText('Advanced: paste a token instead'));
    h.github.userFails = true;
    fireEvent.change(row.getByLabelText('GitHub token value'), { target: { value: GH_TOKEN } });
    fireEvent.click(row.getByRole('button', { name: 'Save' }));
    expect((await screen.findByRole('alert')).textContent).toMatch(/rejected the sign-in/);
    expect(h.settings.hasSecret('githubToken')).toBe(false);
    h.github.userFails = false;
    fireEvent.click(row.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(row.getByText('Signed in as octocat')).toBeTruthy());
    expect(row.getByText(/Using a pasted token/)).toBeTruthy();
    expect(document.body.textContent).not.toContain(GH_TOKEN);
  });

  it('Connect AWS: lists profile names from ~/.aws, picks one, shows the region default and its note, and tests', async () => {
    const tester = { aws: vi.fn(async () => ({ ok: true, detail: 'signed in to account 123456789012' })), anthropic: vi.fn(), openai: vi.fn() };
    const h = installApi({ awsProfiles: profiles, connectionTester: tester as never });
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-aws'));
    expect(await row.findByText('Not connected')).toBeTruthy();
    const list = within(await row.findByRole('radiogroup', { name: 'AWS profile' }));
    expect(list.getAllByRole('radio').map((r) => r.querySelector('.agent-choice__name')!.textContent)).toEqual(['default', 'corp-sso']);
    expect(list.getByRole('radio', { name: /corp-sso/ }).textContent).toContain('SSO · session corp');
    expect((row.getByLabelText('AWS region') as HTMLInputElement).value).toBe('us-east-1');
    expect(row.getByText(/a repo can override it in ratchet\.yaml/)).toBeTruthy();
    expect((row.getByRole('button', { name: 'Test connection' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(list.getByRole('radio', { name: /corp-sso/ }));
    await waitFor(() => expect(row.getByText('Connected · corp-sso')).toBeTruthy());
    expect(h.settings.awsProfile).toBe('corp-sso');
    expect(row.queryByRole('radiogroup', { name: 'AWS profile' })).toBeNull();
    fireEvent.click(row.getByRole('button', { name: 'Test connection' }));
    expect((await row.findByTestId('test-aws')).textContent).toContain('signed in to account 123456789012');
    fireEvent.click(row.getByRole('button', { name: 'Change profile' }));
    expect(row.getByRole('radiogroup', { name: 'AWS profile' })).toBeTruthy();
  });

  it('validates the region while typing and saves a valid one', async () => {
    const h = installApi({ awsProfiles: profiles });
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-aws'));
    const region = await row.findByLabelText('AWS region');
    fireEvent.change(region, { target: { value: 'mars-north-9' } });
    expect(row.getByRole('alert').textContent).toMatch(/region id/);
    expect(row.queryByRole('button', { name: 'Save region' }) === null || (row.getByRole('button', { name: 'Save region' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(region, { target: { value: 'eu-west-2' } });
    fireEvent.click(row.getByRole('button', { name: 'Save region' }));
    await waitFor(() => expect(h.settings.live.awsRegion).toBe('eu-west-2'));
  });

  it('with no ~/.aws it says how to create a profile and offers a rescan', async () => {
    installApi();
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-aws'));
    expect((await row.findByRole('status')).textContent).toMatch(/No ~\/\.aws\/config or ~\/\.aws\/credentials.*aws configure sso/);
    expect(row.getByRole('button', { name: 'Rescan' })).toBeTruthy();
  });

  it('Agents: each detected harness with version and sign-in status; the install hint and a key field only when it is not signed in', async () => {
    installApi();
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-agents'));
    const cc = within(await row.findByTestId('harness-claude-code'));
    expect(cc.getByText('Claude Code')).toBeTruthy();
    expect(cc.getByText('v2.1.4')).toBeTruthy();
    expect(cc.getByText('Signed in')).toBeTruthy();
    expect(cc.queryByRole('status')).toBeNull();
    expect(cc.queryByLabelText('Anthropic API key value')).toBeNull();
    const cx = within(row.getByTestId('harness-codex'));
    expect(cx.getByText('Not installed')).toBeTruthy();
    expect(cx.getByRole('status').textContent).toContain('npm i -g @openai/codex');
    expect(cx.getByLabelText('OpenAI API key value')).toBeTruthy();
    expect(row.getByText('1 ready')).toBeTruthy();
  });

  it('Re-detect probes again and picks up an install', async () => {
    let probes = 0;
    installApi({
      deps: {
        harnesses: new HarnessCache({
          loginPath: async () => '/bin',
          detect: async () => (probes++, probes === 1 ? [claude(), codex()] : [claude(), codex({ installed: true, version: '0.9.0', path: '/bin/codex', auth: 'signed-in', hint: 'signed in' })]),
        }),
      },
    });
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-agents'));
    expect((await row.findByTestId('harness-codex')).getAttribute('data-installed')).toBe('false');
    fireEvent.click(row.getByRole('button', { name: 'Re-detect' }));
    await waitFor(() => expect(row.getByTestId('harness-codex').getAttribute('data-installed')).toBe('true'));
    expect(row.getByText('v0.9.0')).toBeTruthy();
    expect(row.getByText('2 ready')).toBeTruthy();
    expect(within(row.getByTestId('harness-codex')).queryByLabelText('OpenAI API key value')).toBeNull();
    expect(probes).toBe(2);
  });

  it('a signed-out harness shows its hint and the key field; storing a key never echoes it back', async () => {
    const h = installApi({ harnesses: [claude({ auth: 'signed-out', hint: 'Run `claude` and sign in' }), codex()] });
    render(<App initialRoute="settings" />);
    const cc = within(await screen.findByTestId('harness-claude-code'));
    expect(cc.getByText('Signed out')).toBeTruthy();
    expect(cc.getByRole('status').textContent).toBe('Run `claude` and sign in');
    const input = cc.getByLabelText('Anthropic API key value') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-proj-wrong-box-wrong-box' } });
    expect(cc.getByRole('alert').textContent).toMatch(/Anthropic keys start with sk-ant-/);
    expect((cc.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(input, { target: { value: ANTHROPIC_KEY } });
    fireEvent.click(cc.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(h.settings.getSecret('anthropicApiKey')).toBe(ANTHROPIC_KEY));
    await waitFor(() => expect(cc.getByText('Configured')).toBeTruthy());
    expect(input.value).toBe('');
    expect(document.body.textContent).not.toContain(ANTHROPIC_KEY);
  });

  it('"Cloud runners need API keys" holds both keys, collapsed by default', async () => {
    installApi();
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-agents'));
    const d = (await row.findByText('Cloud runners need API keys')).closest('details')!;
    expect(d.open).toBe(false);
    expect(within(d).getAllByLabelText(/API key value/)).toHaveLength(2);
  });

  it('explains when the OS keychain is unavailable and disables key entry', async () => {
    installApi({ encryption: false });
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('row-agents'));
    expect((await row.findByRole('alert')).textContent).toMatch(/OS keychain is unavailable/);
    for (const input of row.getAllByLabelText('OpenAI API key value')) expect((input as HTMLInputElement).disabled).toBe(true);
  });

  it('Advanced (development) holds the AMI and OpenTofu overrides, validated while typing', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    fireEvent.click(await screen.findByText('Advanced (development)'));
    const ami = screen.getByLabelText('Bench AMI');
    fireEvent.change(ami, { target: { value: 'not-an-ami' } });
    expect(screen.getByText(/Use an AMI id such as ami-/)).toBeTruthy();
    const dev = screen.getByText('Advanced (development)').closest('details')!;
    expect((within(dev).getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(ami, { target: { value: 'ami-0123456789abcdef0' } });
    fireEvent.click(within(dev).getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(h.settings.live.benchImageRef).toBe('ami-0123456789abcdef0'));
  });
});
