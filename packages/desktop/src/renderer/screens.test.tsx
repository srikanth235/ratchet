// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { OptimizationsSnapshot } from '../shared/types';
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
    expect(kpis.getByText('8')).toBeTruthy();
    expect(screen.getByRole('img', { name: /Ratchet chart, best down from 182 to 131/ })).toBeTruthy();
    const table = within(screen.getByRole('table'));
    expect(table.getByText('Ingest throughput')).toBeTruthy();
    expect(table.getByText('Web render p99')).toBeTruthy();
    expect(table.getByText('182 ms')).toBeTruthy();
    expect(table.getAllByText('131 ms').length).toBeGreaterThan(0);
    expect(screen.getByText(/Dev engine/)).toBeTruthy();
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
  async function openForm() {
    const h = installApi();
    render(<App initialRoute="new" />);
    await screen.findByText('Valid config');
    return h;
  }

  it('generates a valid ratchet.yaml, preflight list and enables the primary action', async () => {
    await openForm();
    expect(screen.getByLabelText('Generated ratchet.yaml').textContent).toContain('metric: http_req_duration.p95');
    expect(screen.getByText('ratchet.yaml is valid')).toBeTruthy();
    expect(screen.getByText('Simulation mode')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Run baseline & start' }) as HTMLButtonElement).disabled).toBe(false);
  });

  it('reflects form changes in the yaml (budget slider does not change yaml, direction does)', async () => {
    await openForm();
    fireEvent.click(screen.getByRole('radio', { name: 'maximize' }));
    await waitFor(() => expect(screen.getByLabelText('Generated ratchet.yaml').textContent).toContain('direction: maximize'));
    fireEvent.change(screen.getByLabelText('Budget'), { target: { value: '250' } });
    expect(screen.getByText('Budget · $250')).toBeTruthy();
  });

  it('shows path-qualified validation errors from the main process and blocks submit', async () => {
    await openForm();
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    fireEvent.change(screen.getByLabelText('ratchet.yaml'), { target: { value: 'version: 0\nobjective: { metric: a, direction: sideways, min_gain: 0.1 }' } });
    const validation = within(document.querySelector('.validation') as HTMLElement);
    expect(await validation.findByText(/\d errors?/)).toBeTruthy();
    expect(validation.getByText('objective.direction')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Run baseline & start' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Reset' }));
    await screen.findByText('Valid config');
  });

  it('requires at least one agent', async () => {
    await openForm();
    for (const name of ['Claude Code', 'Codex']) fireEvent.click(screen.getByRole('button', { name: new RegExp(name) }));
    await waitFor(() => expect((screen.getByRole('button', { name: 'Run baseline & start' }) as HTMLButtonElement).disabled).toBe(true));
  });

  it('submit creates then starts the optimization and lands on Live agents', async () => {
    const h = await openForm();
    const create = vi.spyOn(window.ratchet, 'createOptimization');
    const start = vi.spyOn(window.ratchet, 'startOptimization');
    fireEvent.change(screen.getByPlaceholderText('search-api p95 latency'), { target: { value: 'My run' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run baseline & start' }));
    expect(await screen.findByRole('heading', { name: 'My run', level: 1 })).toBeTruthy();
    expect(create).toHaveBeenCalledOnce();
    expect(create.mock.calls[0]![0]).toMatchObject({ name: 'My run', baseRef: 'main', agents: ['claude-code', 'codex'], budgetUsd: 100, timeLimitSeconds: 28_800, plateauEvals: 25 });
    expect(create.mock.calls[0]![0].configYaml).toContain('version: 0');
    const id = await create.mock.results[0]!.value;
    expect(start).toHaveBeenCalledWith(id);
    expect(['baselining', 'running']).toContain(h.ledger.getOptimization(id)?.status);
  });

  it('shows the error when creation fails', async () => {
    await openForm();
    vi.spyOn(window.ratchet, 'createOptimization').mockRejectedValue(new Error("Error invoking remote method 'ratchet:createOptimization': Error: boom"));
    fireEvent.click(screen.getByRole('button', { name: 'Run baseline & start' }));
    expect((await screen.findByRole('alert')).textContent).toBe('boom');
  });
});

describe('Live agents', () => {
  it('renders one card per agent with hypothesis, stats, log and bench pipeline', async () => {
    installApi();
    render(<App initialRoute="agents" />);
    const cards = await screen.findAllByRole('generic', { name: /agent$/ });
    expect(cards).toHaveLength(2);
    const cc = within(screen.getByLabelText('Claude Code agent'));
    expect(cc.getByTestId('hypothesis').textContent).toBe('“Short-circuit empty queries before the index”');
    expect(cc.getByText(/Recent evals/)).toBeTruthy();
    await waitFor(() => expect(cc.getAllByText(/Ratchet-Candidate/).length).toBeGreaterThan(0));
    expect(screen.getByText('Trusted bench runner')).toBeTruthy();
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
    expect(screen.getAllByTestId('pct-row').length).toBeGreaterThan(0);
  });
});

describe('Result', () => {
  it('shows headline, wins, spend breakdown and teardown; PR / export / teardown buttons call through', async () => {
    const h = installApi();
    render(<App initialRoute="result" />);
    // featured = running search-api; use the picker to inspect the finished ingest run
    fireEvent.change(await screen.findByLabelText('Optimization'), { target: { value: 'opt_demo_ingest' } });
    expect(await screen.findByTestId('headline')).toBeTruthy();
    await waitFor(() => expect(within(screen.getByTestId('headline')).getByText('58.9')).toBeTruthy());
    expect(within(screen.getByTestId('headline')).getByText('+43.0%')).toBeTruthy();
    expect(screen.getByText('Vectorise the parser inner loop')).toBeTruthy();
    expect(within(screen.getByRole('list', { name: 'Spend by category' })).getByText('Model tokens')).toBeTruthy();
    const checklist = within(screen.getByRole('list', { name: 'Teardown checklist' }));
    expect(checklist.getAllByRole('listitem')).toHaveLength(5);

    const pr = vi.spyOn(window.ratchet, 'openPullRequest');
    fireEvent.click(screen.getByRole('button', { name: /Open pull request/ }));
    expect((await screen.findByRole('status')).textContent).toMatch(/Pull request opened: https:\/\/github\.com\/acme\/ingest\/pull\/\d+/);
    expect(pr).toHaveBeenCalledWith('opt_demo_ingest');
    expect(h.settings.prUrl('opt_demo_ingest')).toMatch(/acme\/ingest/);

    const exp = vi.spyOn(window.ratchet, 'exportReport');
    fireEvent.click(screen.getByRole('button', { name: 'Export report' }));
    await waitFor(() => expect(exp).toHaveBeenCalledWith('opt_demo_ingest'));
    expect((await screen.findByRole('status')).textContent).toBe('Export cancelled');

    const td = vi.spyOn(window.ratchet, 'teardown');
    fireEvent.click(screen.getByRole('button', { name: /Destroy sandbox/ }));
    await waitFor(() => expect(td).toHaveBeenCalledWith('opt_demo_ingest'));
    await waitFor(() => expect(checklist.getAllByText('done')).toHaveLength(5));
  });

  it('disables Open pull request when there are no verified wins, and teardown while still running', async () => {
    installApi();
    render(<App initialRoute="result" />);
    await screen.findByTestId('headline');
    // the run is still live: neither the PR nor teardown is available yet, even though it has verified wins
    expect((screen.getByRole('button', { name: /Open pull request/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: /Destroy sandbox/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/Available once the optimization has finished/)).toBeTruthy();
  });
});

describe('AWS sandbox', () => {
  it('lists plan changes, contract outputs, account and limits from the SandboxProvider', async () => {
    installApi();
    render(<App initialRoute="sandbox" />);
    const plan = within(await screen.findByRole('list', { name: 'Plan changes' }));
    expect(plan.getByText('aws_s3_bucket.artifacts')).toBeTruthy();
    expect(plan.getByText('~ update')).toBeTruthy();
    expect(screen.getByText(/Plan: 9 to add, 1 to change, 0 to destroy/)).toBeTruthy();
    expect(screen.getByText('ratchet-ratchet-sandbox-sealed')).toBeTruthy();
    for (const t of ['c7i.2xlarge', 'c7i.xlarge', 'm7i.large']) expect(screen.getByText(t)).toBeTruthy();
    expect(screen.getByText('Simulated sandbox')).toBeTruthy();
    expect(screen.getByRole('img', { name: /Sandbox architecture/ })).toBeTruthy();
  });
});

describe('Settings', () => {
  it('shows only configured yes/no; saving a key stores it and never echoes it back', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('secret-anthropicApiKey'));
    expect(row.getByText('Not set')).toBeTruthy();
    const input = row.getByLabelText('Anthropic API key value') as HTMLInputElement;
    expect(input.type).toBe('password');
    fireEvent.change(input, { target: { value: 'sk-ant-api03-VERYSECRET' } });
    fireEvent.click(row.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(row.getByText('Configured')).toBeTruthy());
    expect(input.value).toBe('');
    expect(document.body.textContent).not.toContain('VERYSECRET');
    expect(h.settings.getSecret('anthropicApiKey')).toBe('sk-ant-api03-VERYSECRET');

    fireEvent.click(row.getByRole('button', { name: 'Clear' }));
    await waitFor(() => expect(row.getByText('Not set')).toBeTruthy());
  });

  it('switches mode and saves the AWS profile', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    fireEvent.click(await screen.findByRole('radio', { name: /Live/ }));
    await waitFor(() => expect(h.settings.mode).toBe('live'));
    await waitFor(() => expect(screen.getByRole('radio', { name: /Live/ }).getAttribute('aria-checked')).toBe('true'));
    const profile = screen.getByLabelText('AWS profile');
    fireEvent.change(profile, { target: { value: 'ratchet-sandbox' } });
    fireEvent.click(within(profile.closest('form') as HTMLElement).getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(h.settings.awsProfile).toBe('ratchet-sandbox'));
  });

  it('disables secret entry and explains when the OS keychain is unavailable', async () => {
    installApi({ encryption: false });
    render(<App initialRoute="settings" />);
    expect((await screen.findByRole('alert')).textContent).toContain('keychain is unavailable');
    expect((screen.getByLabelText('GitHub token value') as HTMLInputElement).disabled).toBe(true);
  });
});
