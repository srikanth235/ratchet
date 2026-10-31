// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { HarnessCache } from '../main/harnesses';
import { ANTHROPIC_KEY, DEVICE_TOKEN, GH_TOKEN, OPENAI_KEY, claude, codex } from '../main/test-support';
import { App } from './App';
import { installApi } from './test-utils';

const ORG = 'github_pat_ORGTOKENORGTOKENORGTOKEN12';
const profiles = {
  profiles: [
    { name: 'acme-prod', kind: 'sso' as const, source: 'config' as const, ssoSession: 'acme' },
    { name: 'personal-dev', kind: 'keys' as const, source: 'credentials' as const, ssoSession: null },
  ],
  ssoSessions: ['acme'],
  filesFound: true,
};

const group = async (kind: 'github' | 'aws' | 'agent') => within(await screen.findByTestId(`connectors-${kind}`));
const row = (name: string) => within(screen.getByText(name, { selector: '.cn-row__name' }).closest('li')!);
const openMenu = (name: string, item: string) => {
  fireEvent.click(screen.getByRole('button', { name: `Actions for ${name}` }));
  fireEvent.click(screen.getByRole('menuitem', { name: item }));
};
const openAdd = () => fireEvent.click(screen.getByRole('button', { name: 'Add connector' }));
const panel = () => within(screen.getByRole('region', { name: 'Add a connector' }));
const agentKind = () => fireEvent.click(panel().getByRole('button', { name: /^Agent\s+A Claude Code/ }));
const accounts = (h: { connectors: { snapshot(): Array<{ kind: string }> } }, kind: string) => h.connectors.snapshot().filter((c) => c.kind === kind);

describe('Connectors screen', () => {
  it('has the title, the one-line sub, a primary Add connector, three groups and Advanced collapsed at the bottom', async () => {
    installApi();
    render(<App initialRoute="settings" />);
    expect(await screen.findByRole('heading', { name: 'Connectors', level: 1 })).toBeTruthy();
    expect(screen.getByText('Accounts each optimization can use. Secrets stay in your keychain.')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Add connector' })).toBeTruthy();
    expect([...document.querySelectorAll('.cn-group > h2')].map((h) => h.textContent)).toEqual(['GitHub', 'AWS', 'Agents']);
    expect((await group('github')).getByText(/No GitHub account yet/)).toBeTruthy();
    expect((await group('aws')).getByText(/No AWS profile yet/)).toBeTruthy();
    const adv = screen.getByText('Advanced (development)').closest('details')!;
    expect(adv.open).toBe(false);
    expect(adv.compareDocumentPosition(screen.getByTestId('connectors-agent')) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy(); // after the groups
    // no old per-repo knobs and no boxes: rows are plain list items
    expect(screen.queryByRole('radio', { name: /Simulation|Live/ })).toBeNull();
    expect(document.querySelector('.settings-row')).toBeNull();
  });

  it('each row: name, one muted detail line, Default in text, status as a dot and a word; the agents show their login and key', async () => {
    const h = installApi({ awsProfiles: profiles, harnesses: [claude({ version: '2.1.314' }), codex({ installed: true, version: '0.166', path: '/bin/codex', auth: 'signed-out', hint: 'x' })] });
    h.connectors.upsertGithub({ login: 'srikanth235', token: GH_TOKEN, source: 'device' });
    h.connectors.upsertGithub({ login: 'acme-org', token: ORG, source: 'token' });
    h.connectors.addAws({ profile: 'acme-prod', region: 'us-east-1', monthlyCapUsd: 500 });
    h.connectors.addAws({ profile: 'personal-dev', region: 'us-west-2' });
    await h.connectors.addAgent({ agent: 'claude-code', source: 'api-key', apiKey: ANTHROPIC_KEY, name: 'Claude Code · team key' });
    render(<App initialRoute="settings" />);
    const gh = await group('github');
    await waitFor(() => expect(gh.getAllByRole('listitem')).toHaveLength(2));
    const first = within(gh.getAllByRole('listitem')[0]!);
    expect(first.getByText('srikanth235')).toBeTruthy();
    expect(first.getByText('Signed in with GitHub')).toBeTruthy();
    expect(first.getByText('Default')).toBeTruthy();
    expect(first.getByText('Connected')).toBeTruthy();
    expect(within(gh.getAllByRole('listitem')[1]!).queryByText('Default')).toBeNull();
    expect(gh.getByText('Access token')).toBeTruthy();

    const aws = await group('aws');
    expect(aws.getByText('SSO · us-east-1 · cap $500 / month')).toBeTruthy();
    expect(aws.getByText('~/.aws profile · us-west-2')).toBeTruthy();

    const agents = await group('agent');
    await waitFor(() => expect(agents.getAllByRole('listitem')).toHaveLength(3));
    expect(agents.getAllByRole('listitem').map((li) => li.querySelector('.cn-row__name')!.textContent)).toEqual(['Claude Code · your login', 'Claude Code · team key', 'Codex · your login']);
    expect(agents.getByText('Found on this Mac · v2.1.314')).toBeTruthy();
    expect(agents.getByText('Signed in')).toBeTruthy();
    expect(agents.getByText(`API key ····${ANTHROPIC_KEY.slice(-4)} · also runs in the cloud`)).toBeTruthy();
    expect(agents.getByText('Valid')).toBeTruthy();
    expect(agents.getByText('Signed out')).toBeTruthy();
    expect(agents.getByText('codex login').className).toContain('mono'); // a command is shown in monospace
    expect(agents.getAllByText('Default')).toHaveLength(1); // one default for the kind
    expect(document.body.textContent).not.toMatch(/ghp_|sk-ant-api|github_pat_/);
  });

  it('an expired AWS profile offers Sign in with the command to run, and checking again clears it', async () => {
    const aws = vi.fn(async () => ({ ok: false, detail: 'The SSO session has expired' }));
    const h = installApi({ awsProfiles: profiles, connectionTester: { aws, anthropic: vi.fn(), openai: vi.fn() } as never });
    h.connectors.addAws({ profile: 'acme-prod', region: 'eu-west-1' });
    render(<App initialRoute="settings" />);
    // A late response of the mount-time connection probe can briefly show the row as Connected again, so click
    // inside the retry loop: it only goes through once the Sign in button is really there.
    await waitFor(() => {
      expect(row('acme-prod').getByText('Expired')).toBeTruthy();
      fireEvent.click(row('acme-prod').getByRole('button', { name: 'Sign in' }));
    });
    expect(row('acme-prod').getByRole('status').textContent).toContain('aws sso login --profile acme-prod');
    aws.mockResolvedValue({ ok: true, detail: 'signed in to account 123456789012' } as never);
    fireEvent.click(row('acme-prod').getByRole('button', { name: 'Check again' }));
    await waitFor(() => expect(row('acme-prod').getByText('Connected')).toBeTruthy());
  });

  it('the row menu: set default, check connection, rename, remove with a confirmation (no dialogs)', async () => {
    const h = installApi();
    h.connectors.upsertGithub({ login: 'srikanth235', token: GH_TOKEN, source: 'device' });
    h.github.login = 'acme-org';
    h.connectors.upsertGithub({ login: 'acme-org', token: ORG, source: 'token' });
    render(<App initialRoute="settings" />);
    await screen.findByText('acme-org', { selector: '.cn-row__name' });
    expect(row('srikanth235').getByText('Default')).toBeTruthy();

    openMenu('acme-org', 'Set as default');
    await waitFor(() => expect(row('acme-org').getByText('Default')).toBeTruthy());
    expect(row('srikanth235').queryByText('Default')).toBeNull();
    expect(h.connectors.defaultOf('github')?.name).toBe('acme-org');

    openMenu('acme-org', 'Check connection');
    expect((await screen.findAllByRole('status')).some((s) => /signed in as acme-org/.test(s.textContent ?? ''))).toBe(true);

    openMenu('acme-org', 'Rename');
    const input = screen.getByLabelText('Name of acme-org');
    fireEvent.change(input, { target: { value: 'Acme organisation' } });
    fireEvent.click(within(input.closest('form')!).getByRole('button', { name: 'Save' }));
    await screen.findByText('Acme organisation', { selector: '.cn-row__name' });

    openMenu('Acme organisation', 'Remove');
    expect(screen.getByText(/Remove Acme organisation\?/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(screen.queryByText(/Remove Acme organisation\?/)).toBeNull();
    openMenu('Acme organisation', 'Remove');
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(screen.queryByText('Acme organisation', { selector: '.cn-row__name' })).toBeNull());
    expect(accounts(h, 'github').map((c) => (c as unknown as { name: string }).name)).toEqual(['srikanth235']);
    expect(row('srikanth235').getByText('Default')).toBeTruthy();
  });

  it('Add connector > GitHub: the device flow shows the code, then the account appears and the panel closes', async () => {
    const h = installApi();
    h.github.clientIdConfigured = false;
    render(<App initialRoute="settings" />);
    await screen.findByTestId('connectors');
    openAdd();
    const panel = within(screen.getByRole('region', { name: 'Add a connector' }));
    expect(panel.getAllByRole('button').map((b) => b.querySelector('.cn-kind__name')?.textContent).filter(Boolean)).toEqual(['GitHub account', 'AWS profile', 'Agent']);
    fireEvent.click(panel.getByRole('button', { name: /GitHub account/ }));
    expect(panel.getByText(/built-in placeholder OAuth client id/)).toBeTruthy();
    h.github.polls = [{ status: 'pending' }, { status: 'ok', token: DEVICE_TOKEN }];
    fireEvent.click(panel.getByRole('button', { name: 'Sign in with GitHub' }));
    expect((await panel.findByTestId('device-code')).textContent).toBe('ABCD-1234');
    expect(h.opened).toEqual(['https://github.com/login/device']);
    await screen.findByText('octocat', { selector: '.cn-row__name' }, { timeout: 10_000 });
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Add a connector' })).toBeNull(), { timeout: 10_000 });
    expect(row('octocat').getByText('Default')).toBeTruthy();
    expect(document.body.textContent).not.toContain(DEVICE_TOKEN);
  }, 25_000);

  it('a denied sign-in is explained inside the panel', async () => {
    const h = installApi();
    h.github.polls = [{ status: 'denied' }];
    render(<App initialRoute="settings" />);
    await screen.findByTestId('connectors');
    openAdd();
    fireEvent.click(screen.getByRole('button', { name: /GitHub account/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Sign in with GitHub' }));
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/denied/), { timeout: 10_000 });
  }, 25_000);

  it('Add connector > GitHub: a pasted token is checked before it is kept, and never echoed', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    await screen.findByTestId('connectors');
    openAdd();
    fireEvent.click(screen.getByRole('button', { name: /GitHub account/ }));
    const more = screen.getByText('Use an access token instead').closest('details')!;
    expect(more.open).toBe(false);
    fireEvent.click(screen.getByText('Use an access token instead'));
    const input = screen.getByLabelText('GitHub token') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'not-a-token-at-all' } });
    expect(screen.getByRole('alert').textContent).toMatch(/GitHub tokens start with/);
    h.github.userFails = true;
    fireEvent.change(input, { target: { value: GH_TOKEN } });
    fireEvent.click(within(more).getByRole('button', { name: 'Add' }));
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/rejected the sign-in/));
    expect(accounts(h, 'github')).toEqual([]);
    h.github.userFails = false;
    fireEvent.click(within(more).getByRole('button', { name: 'Add' }));
    await screen.findByText('octocat', { selector: '.cn-row__name' });
    expect(row('octocat').getByText('Access token')).toBeTruthy();
    expect(document.body.textContent).not.toContain(GH_TOKEN);
  });

  it('Add connector > AWS: pick a profile from ~/.aws, set the region and a cap; already added profiles are not offered again', async () => {
    const h = installApi({ awsProfiles: profiles });
    render(<App initialRoute="settings" />);
    await screen.findByTestId('connectors');
    openAdd();
    fireEvent.click(screen.getByRole('button', { name: /AWS profile/ }));
    const list = within(await screen.findByRole('radiogroup', { name: 'AWS profile' }));
    expect(list.getAllByRole('radio').map((r) => r.querySelector('.cn-choice__name')!.textContent)).toEqual(['acme-prod', 'personal-dev']);
    expect(list.getByRole('radio', { name: /acme-prod/ }).textContent).toContain('SSO · session acme');
    expect((screen.getByLabelText('AWS region') as HTMLInputElement).value).toBe('us-east-1');
    const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement;
    expect(add.disabled).toBe(true); // nothing picked yet
    fireEvent.click(list.getByRole('radio', { name: /acme-prod/ }));
    fireEvent.change(screen.getByLabelText('AWS region'), { target: { value: 'mars-north-9' } });
    expect(screen.getByRole('alert').textContent).toMatch(/region id/);
    expect(add.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText('AWS region'), { target: { value: 'eu-west-2' } });
    fireEvent.change(screen.getByLabelText('Monthly cap in dollars'), { target: { value: '500' } });
    fireEvent.click(add);
    await screen.findByText('SSO · eu-west-2 · cap $500 / month');
    expect(accounts(h, 'aws')).toMatchObject([{ kind: 'aws', profile: 'acme-prod', region: 'eu-west-2', monthlyCapUsd: 500, isDefault: true }]);
    expect(screen.queryByRole('region', { name: 'Add a connector' })).toBeNull();

    openAdd();
    fireEvent.click(screen.getByRole('button', { name: /AWS profile/ }));
    const again = within(await screen.findByRole('radiogroup', { name: 'AWS profile' }));
    expect(again.getAllByRole('radio').map((r) => r.querySelector('.cn-choice__name')!.textContent)).toEqual(['personal-dev']);
  });

  it('Add connector > AWS with no ~/.aws says how to create a profile and offers a rescan', async () => {
    installApi();
    render(<App initialRoute="settings" />);
    await screen.findByTestId('connectors');
    openAdd();
    fireEvent.click(screen.getByRole('button', { name: /AWS profile/ }));
    expect((await screen.findByRole('status')).textContent).toMatch(/No ~\/\.aws\/config or ~\/\.aws\/credentials.*aws configure sso/);
    expect(screen.getByRole('button', { name: 'Rescan' })).toBeTruthy();
  });

  it('AWS rows: edit the region and the cap in place', async () => {
    const h = installApi({ awsProfiles: profiles });
    h.connectors.addAws({ profile: 'acme-prod', monthlyCapUsd: 500 });
    render(<App initialRoute="settings" />);
    await screen.findByText('acme-prod', { selector: '.cn-row__name' });
    openMenu('acme-prod', 'Edit region and cap');
    fireEvent.change(screen.getByLabelText('Region'), { target: { value: 'eu-west-1' } });
    fireEvent.change(screen.getByLabelText('Monthly cap in dollars'), { target: { value: '' } });
    fireEvent.click(within(screen.getByLabelText('Region').closest('form')!).getByRole('button', { name: 'Save' }));
    await screen.findByText('SSO · eu-west-1');
    expect(accounts(h, 'aws')[0]).toMatchObject({ region: 'eu-west-1', monthlyCapUsd: null });
  });

  it('Add connector > Agent: an API key is validated while typing, then listed as Valid (and never shown again); a detected login can be added back', async () => {
    const tester = { aws: vi.fn(), anthropic: vi.fn(async () => ({ ok: true, detail: 'ok' })), openai: vi.fn(async () => ({ ok: true, detail: 'ok' })) };
    const h = installApi({ connectionTester: tester as never });
    render(<App initialRoute="settings" />);
    const agents = await group('agent');
    await waitFor(() => expect(agents.getByText('Claude Code · your login')).toBeTruthy());
    openMenu('Claude Code · your login', 'Remove');
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(agents.queryByText('Claude Code · your login')).toBeNull());

    openAdd();
    agentKind();
    expect(await screen.findByText(/Claude Code · v2\.1\.4/)).toBeTruthy();
    const key = screen.getByLabelText('API key') as HTMLInputElement;
    fireEvent.change(screen.getByLabelText('Agent'), { target: { value: 'claude-code' } });
    fireEvent.change(key, { target: { value: OPENAI_KEY } });
    expect(screen.getByRole('alert').textContent).toMatch(/Anthropic keys start with sk-ant-/);
    fireEvent.change(key, { target: { value: ANTHROPIC_KEY } });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Claude Code · team key' } });
    fireEvent.click(panel().getByRole('button', { name: 'Add' }));
    await (await group('agent')).findByText('Claude Code · team key');
    expect(row('Claude Code · team key').getByText('Valid')).toBeTruthy();
    expect(row('Claude Code · team key').getByText(/also runs in the cloud/)).toBeTruthy();
    expect(document.body.textContent).not.toContain(ANTHROPIC_KEY);
    expect(tester.anthropic).toHaveBeenCalled();

    openAdd();
    agentKind();
    fireEvent.click(await panel().findByRole('button', { name: 'Add your login' }));
    await (await group('agent')).findByText('Claude Code · your login');
    expect(accounts(h, 'agent')).toHaveLength(2);
  });

  it('Re-detect picks up a newly installed CLI', async () => {
    let probes = 0;
    installApi({
      deps: {
        harnesses: new HarnessCache({
          loginPath: async () => '/bin',
          detect: async () => (probes++, probes <= 2 ? [claude(), codex()] : [claude(), codex({ installed: true, version: '0.9.0', path: '/bin/codex', auth: 'signed-in', hint: 'ok' })]),
        }),
      },
    });
    render(<App initialRoute="settings" />);
    await screen.findByText('Claude Code · your login');
    expect(screen.queryByText('Codex · your login')).toBeNull();
    openAdd();
    agentKind();
    await screen.findByText(/already listed/);
    fireEvent.click(screen.getByRole('button', { name: 'Re-detect' }));
    await screen.findByText('Codex · your login');
  });

  it('explains when the OS keychain is unavailable and disables token and key entry', async () => {
    installApi({ encryption: false });
    render(<App initialRoute="settings" />);
    expect((await screen.findByRole('alert')).textContent).toMatch(/OS keychain is unavailable/);
    openAdd();
    agentKind();
    expect((await screen.findByLabelText('API key') as HTMLInputElement).disabled).toBe(true);
  });

  it('Advanced (development) holds the AMI and OpenTofu overrides, validated while typing', async () => {
    const h = installApi();
    render(<App initialRoute="settings" />);
    fireEvent.click(await screen.findByText('Advanced (development)'));
    const ami = await screen.findByLabelText('Bench AMI');
    fireEvent.change(ami, { target: { value: 'not-an-ami' } });
    expect(screen.getByText(/Use an AMI id such as ami-/)).toBeTruthy();
    const dev = screen.getByText('Advanced (development)').closest('details')!;
    expect((within(dev).getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(ami, { target: { value: 'ami-0123456789abcdef0' } });
    fireEvent.click(within(dev).getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(h.settings.live.benchImageRef).toBe('ami-0123456789abcdef0'));
    for (const label of ['Runner AMI', 'OpenTofu binary', 'OpenTofu module directory']) expect(within(dev).getByLabelText(label)).toBeTruthy();
  });
});
