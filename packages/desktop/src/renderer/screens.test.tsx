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

  it('evaluator cards, guardrail ticks and the value fields edit the yaml; yaml edits update the form', async () => {
    await openForm();
    const yamlText = () => screen.getByLabelText('Generated ratchet.yaml').textContent ?? '';
    expect(yamlText()).toMatch(/^evaluator:\n {2}kind: benchmark/m);
    expect(yamlText()).not.toMatch(/^bench:/m);
    const kinds = within(screen.getByRole('radiogroup', { name: 'Evaluator kind' }));
    expect(kinds.getAllByRole('radio')).toHaveLength(4);
    fireEvent.click(kinds.getByRole('radio', { name: 'Backtest' }));
    await waitFor(() => expect(yamlText()).toContain('kind: backtest'));

    const guards = within(screen.getByRole('list', { name: 'Guardrails' }));
    fireEvent.click(guards.getByRole('checkbox', { name: 'peak_rss: < 1.2x' }));
    await waitFor(() => expect(yamlText()).not.toContain('peak_rss'));
    expect((guards.getByRole('checkbox', { name: 'peak_rss: < 1.2x' }) as HTMLInputElement).checked).toBe(false);
    fireEvent.click(guards.getByRole('checkbox', { name: 'peak_rss: < 1.2x' }));
    await waitFor(() => expect(yamlText()).toContain('peak_rss: < 1.2x'));
    fireEvent.change(screen.getByLabelText('New guardrail'), { target: { value: 'run: pnpm lint' } });
    fireEvent.click(guards.getByRole('button', { name: /Add/ }));
    await waitFor(() => expect(yamlText()).toContain('run: pnpm lint'));

    fireEvent.change(screen.getByLabelText('USD per unit of improvement'), { target: { value: '1200' } });
    fireEvent.change(screen.getByLabelText(/^Unit/), { target: { value: 'ms' } });
    await waitFor(() => expect(yamlText()).toMatch(/value:\n {4}usd_per_unit: 1200\n {4}period: year\n {4}basis: estimated\n {4}unit: ms/));
    expect(await screen.findByText('$1,200 per ms / year · estimated')).toBeTruthy();

    // the other way: edit the text, the form follows
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    const editor = screen.getByLabelText('ratchet.yaml') as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: editor.value.replace('kind: backtest', 'kind: simulation').replace('metric: http_req_duration.p95', 'metric: cost_usd') } });
    await waitFor(() => expect((kinds.getByRole('radio', { name: 'Simulation' }) as HTMLInputElement).checked).toBe(true));
    expect((screen.getByDisplayValue('cost_usd') as HTMLInputElement).tagName).toBe('INPUT');
    await screen.findByText('Valid config');
  });

  it('locks the form while the yaml does not parse, and unlocks once it does', async () => {
    await openForm();
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    const editor = screen.getByLabelText('ratchet.yaml') as HTMLTextAreaElement;
    const good = editor.value;
    fireEvent.change(editor, { target: { value: 'objective: [unclosed' } });
    expect(await screen.findByText(/does not parse as YAML/)).toBeTruthy();
    expect(screen.getByRole('radio', { name: 'Backtest' }).matches(':disabled')).toBe(true);
    fireEvent.change(editor, { target: { value: good } });
    await waitFor(() => expect(screen.getByRole('radio', { name: 'Backtest' }).matches(':disabled')).toBe(false));
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

  it('lands on Live agents immediately and reports a failed start there', async () => {
    await openForm();
    vi.spyOn(window.ratchet, 'startOptimization').mockRejectedValue(new Error("Error invoking remote method 'ratchet:startOptimization': Error: baseline blew up"));
    fireEvent.change(screen.getByPlaceholderText('search-api p95 latency'), { target: { value: 'Doomed run' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run baseline & start' }));
    expect(await screen.findByRole('heading', { name: 'Doomed run', level: 1 })).toBeTruthy();
    expect((await screen.findByRole('alert')).textContent).toBe('Could not start: baseline blew up');
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
  it('shows the plan, account and limits before anything is applied', async () => {
    installApi();
    render(<App initialRoute="sandbox" />);
    const plan = within(await screen.findByRole('list', { name: 'Plan changes' }));
    expect(plan.getByText('aws_s3_bucket.artifacts')).toBeTruthy();
    expect(plan.getByText('~ update')).toBeTruthy();
    expect(screen.getByText(/Plan: 9 to add, 1 to change, 0 to destroy/)).toBeTruthy();
    expect(screen.getByText('Plan ready · 9 to add')).toBeTruthy();
    expect(screen.getAllByText('after apply')).toHaveLength(5);
    for (const t of ['c7i.2xlarge', 'c7i.xlarge', 'm7i.large']) expect(screen.getByText(t)).toBeTruthy();
    expect(screen.getByText('Simulated sandbox')).toBeTruthy();
    expect(screen.getByRole('img', { name: /Sandbox architecture/ })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Destroy sandbox' })).toBeNull();
  });

  it('applies after a confirmation, shows the real outputs, then destroys after a confirmation', async () => {
    const h = installApi();
    render(<App initialRoute="sandbox" />);
    await screen.findByRole('list', { name: 'Plan changes' });

    // Apply asks first and shows the plan being applied; cancelling changes nothing.
    fireEvent.click(screen.getByRole('button', { name: 'Apply sandbox' }));
    const confirm = within(await screen.findByRole('group', { name: 'Confirm apply' }));
    expect(confirm.getByText(/Plan: 9 to add, 1 to change, 0 to destroy/)).toBeTruthy();
    fireEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('group', { name: 'Confirm apply' })).toBeNull();
    expect(h.ledger.getSandbox('ratchet-sim')?.status).toBe('planned');

    fireEvent.click(screen.getByRole('button', { name: 'Apply sandbox' }));
    fireEvent.click(within(await screen.findByRole('group', { name: 'Confirm apply' })).getByRole('button', { name: 'Confirm apply' }));
    expect(await screen.findByText('Applied')).toBeTruthy();
    await waitFor(() => expect(screen.getByText('ratchet-ratchet-sim-sealed')).toBeTruthy());
    expect(screen.queryByText('after apply')).toBeNull();
    expect(screen.getByText('0000 ···· 0000')).toBeTruthy();
    expect(h.ledger.getSandbox('ratchet-sim')).toMatchObject({ status: 'applied', provider: 'mock' });
    expect(within(screen.getByRole('list', { name: 'Plan changes' })).getAllByText('no change').length).toBeGreaterThan(5);
    expect(screen.queryByRole('button', { name: 'Apply sandbox' })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Destroy sandbox' }));
    const destroy = within(await screen.findByRole('group', { name: 'Confirm destroy' }));
    fireEvent.click(destroy.getByRole('button', { name: 'Confirm destroy' }));
    expect(await screen.findByText('Destroyed')).toBeTruthy();
    expect(h.ledger.getSandbox('ratchet-sim')).toMatchObject({ status: 'destroyed', outputs: null });
    await waitFor(() => expect(screen.getAllByText('after apply')).toHaveLength(5));
    expect(screen.getByRole('button', { name: 'Apply sandbox' })).toBeTruthy();
  });

  it('disables Destroy while an optimization in the sandbox is running and says why', async () => {
    const h = installApi();
    await h.service.applySandbox();
    h.ledger.createOptimization({ id: 'opt-live', name: 'Running one', repo: 'acme/x', baseRef: 'main', metric: 'm', direction: 'minimize', minGain: 0.1, budgetUsd: 10, timeLimitS: 600, configYaml: 'x', status: 'running', sandboxId: 'ratchet-sim' });
    render(<App initialRoute="sandbox" />);
    const destroy = (await screen.findByRole('button', { name: 'Destroy sandbox' })) as HTMLButtonElement;
    expect(destroy.disabled).toBe(true);
    expect(screen.getByText(/Destroy unavailable: Stop optimization opt-live first/)).toBeTruthy();
    // and the service refuses even if asked directly
    await expect(h.service.destroySandbox()).rejects.toThrow(/still running/);
  });

  it('shows a failed apply with its reason, and a real plan only when asked for', async () => {
    const h = installApi();
    const base = await h.service.sandboxInfo();
    let planned = false;
    window.ratchet.sandboxInfo = async () => ({ ...base, plan: planned ? base.plan : null, account: { ...base.account, simulated: false } });
    window.ratchet.planSandbox = async () => ((planned = true), { ...base, plan: base.plan, account: { ...base.account, simulated: false } });
    window.ratchet.applySandbox = async () => {
      throw new Error('tofu apply failed (exit 1): Error: AccessDenied');
    };
    render(<App initialRoute="sandbox" />);
    expect(await screen.findByText(/A real plan runs/)).toBeTruthy();
    expect(screen.getByText('Live account')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Apply sandbox' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Run plan' }));
    await screen.findByRole('list', { name: 'Plan changes' });
    fireEvent.click(screen.getByRole('button', { name: 'Apply sandbox' }));
    fireEvent.click(within(await screen.findByRole('group', { name: 'Confirm apply' })).getByRole('button', { name: 'Confirm apply' }));
    expect((await screen.findByRole('alert')).textContent).toContain('AccessDenied');
    expect(screen.queryByRole('group', { name: 'Confirm apply' })).toBeNull();
  });
});

/** The Save button of the live-settings form (the secret rows have their own Save buttons). */
const liveSave = () => within(screen.getByLabelText('AWS profile').closest('form') as HTMLElement).getByRole('button', { name: 'Save' }) as HTMLButtonElement;

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

  it('lists what a live run still needs and updates as settings are saved', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    const list = within(await screen.findByRole('list', { name: 'Live readiness' }));
    expect(screen.getByText('Not ready')).toBeTruthy();
    for (const label of ['AWS sign-in', 'GitHub repository and token', 'Model API key', 'Runner and bench AMIs', 'Sandbox applied']) expect(list.getByText(new RegExp(label))).toBeTruthy();
    expect(list.getByText(/OpenTofu/).closest('li')?.getAttribute('data-state')).toBe('ok');

    fireEvent.change(screen.getByLabelText('AWS profile'), { target: { value: 'ratchet-sandbox' } });
    fireEvent.click(liveSave());
    await waitFor(() => expect(h.settings.awsProfile).toBe('ratchet-sandbox'));
    await waitFor(() => expect(list.getByText(/AWS sign-in/).closest('li')?.getAttribute('data-state')).toBe('ok'));
  });

  it('validates live settings as you type and only saves valid ones', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    const region = (await screen.findByLabelText('AWS region')) as HTMLInputElement;
    const save = liveSave();
    expect(save.disabled).toBe(true); // nothing changed

    fireEvent.change(region, { target: { value: 'moon-base-1' } });
    expect((await screen.findAllByRole('alert')).some((a) => /region id such as us-east-1/.test(a.textContent ?? ''))).toBe(true);
    expect(save.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText('Runner AMI'), { target: { value: 'ami-123' } });
    expect(screen.getByText(/AMI id such as ami-0123456789abcdef0/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText('GitHub repository'), { target: { value: 'not a repo' } });
    expect(screen.getByText('Use owner/name, for example acme/search-api')).toBeTruthy();

    fireEvent.change(region, { target: { value: 'eu-west-2' } });
    fireEvent.change(screen.getByLabelText('Runner AMI'), { target: { value: 'ami-0123456789abcdef0' } });
    fireEvent.change(screen.getByLabelText('GitHub repository'), { target: { value: 'acme/search-api' } });
    fireEvent.click(screen.getByRole('radio', { name: 'SSO' }));
    await waitFor(() => expect(save.disabled).toBe(false));
    fireEvent.click(save);
    await waitFor(() => expect(h.settings.live).toMatchObject({ awsRegion: 'eu-west-2', runnerImageRef: 'ami-0123456789abcdef0', githubRepo: 'acme/search-api', awsAuth: 'sso' }));
    await waitFor(() => expect(liveSave().disabled).toBe(true));
  });

  it('checks the key format before storing, and tests a stored connection without ever showing the secret', async () => {
    const calls: unknown[] = [];
    const ok = (target: string) => async (i: unknown) => (calls.push([target, i]), { target, ok: true, detail: `${target} is fine` }) as never;
    const h = installApi({ connectionTester: { aws: ok('aws'), github: ok('github'), anthropic: ok('anthropic'), openai: ok('openai'), tofu: ok('tofu') } });
    render(<App initialRoute="settings" />);
    const row = within(await screen.findByTestId('secret-anthropicApiKey'));
    const input = row.getByLabelText('Anthropic API key value') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-proj-this-is-the-wrong-kind-of-key' } });
    expect(row.getByText('Anthropic keys start with sk-ant-')).toBeTruthy();
    expect((row.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(input, { target: { value: 'sk-ant-api03-VERYSECRET' } });
    fireEvent.click(row.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(row.getByText('Configured')).toBeTruthy());
    fireEvent.click(row.getByRole('button', { name: 'Test connection' }));
    expect(await row.findByText(/anthropic is fine/)).toBeTruthy();
    expect(calls).toEqual([['anthropic', { apiKey: 'sk-ant-api03-VERYSECRET' }]]); // read in the main process, not typed again
    expect(document.body.textContent).not.toContain('VERYSECRET');
    expect(h.settings.getSecret('anthropicApiKey')).toBe('sk-ant-api03-VERYSECRET');

    // OpenTofu check has no secret at all and reports through the same path
    fireEvent.click(screen.getAllByRole('button', { name: 'Test connection' }).at(-1)!);
    expect(await screen.findByText(/tofu is fine/)).toBeTruthy();
  });

  it('shows a failed connection test as a failure', async () => {
    installApi({ connectionTester: { aws: async () => ({ target: 'aws', ok: false, detail: 'The SSO session associated with this profile has expired' }), github: async () => ({ target: 'github', ok: true, detail: '' }), anthropic: async () => ({ target: 'anthropic', ok: true, detail: '' }), openai: async () => ({ target: 'openai', ok: true, detail: '' }), tofu: async () => ({ target: 'tofu', ok: true, detail: '' }) } });
    render(<App initialRoute="settings" />);
    fireEvent.change(await screen.findByLabelText('AWS profile'), { target: { value: 'corp' } });
    fireEvent.click(liveSave());
    await waitFor(() => expect((screen.getAllByRole('button', { name: 'Test connection' })[0] as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getAllByRole('button', { name: 'Test connection' })[0]!);
    const result = await screen.findByTestId('test-aws');
    expect(result.textContent).toContain('SSO session');
    expect(result.className).toContain('test-result--fail');
  });

  it('disables secret entry and explains when the OS keychain is unavailable', async () => {
    installApi({ encryption: false });
    render(<App initialRoute="settings" />);
    expect((await screen.findByRole('alert')).textContent).toContain('keychain is unavailable');
    expect((screen.getByLabelText('GitHub token value') as HTMLInputElement).disabled).toBe(true);
  });
});
