import { useEffect, useMemo, useRef, useState } from 'react';
import { Button } from '@ratchet/ui';
import { validateLiveSetting, validateSecretFormat, type LiveSettingKey } from '../../shared/live-settings';
import type {
  AgentConnectorVM,
  AwsConnectorVM,
  AwsProfilesVM,
  ConnectorAgent,
  ConnectorKind,
  ConnectorsVM,
  ConnectorVM,
  HarnessVM,
  LiveSettings,
  SettingsVM,
} from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { errMsg, useQuery } from '../data';
import '../connectors.css';

const AGENT_NAME: Record<ConnectorAgent, string> = { 'claude-code': 'Claude Code', codex: 'Codex' };

/** "run `codex login`": backticks mark a command shown in monospace. */
function Detail({ text }: { text: string }) {
  return (
    <>
      {text.split('`').map((part, i) => (i % 2 === 1 ? <span key={i} className="mono">{part}</span> : <span key={i}>{part}</span>))}
    </>
  );
}

// ------------------------------------------------------------------ status

type Tone = 'ok' | 'wait' | 'off' | 'bad';

function statusOf(c: ConnectorVM): { word: string; tone: Tone } {
  const login = c.kind === 'agent' && c.source === 'login';
  switch (c.status) {
    case 'connected':
      return { word: login ? 'Signed in' : 'Connected', tone: 'ok' };
    case 'valid':
      return { word: 'Valid', tone: 'ok' };
    case 'expired':
      return { word: 'Expired', tone: 'wait' };
    case 'signed-out':
      return { word: 'Signed out', tone: 'off' };
    case 'invalid':
      return { word: 'Invalid', tone: 'bad' };
  }
}

// ------------------------------------------------------------------ the row's quiet menu

interface MenuItem {
  label: string;
  onSelect(): void;
  danger?: boolean;
}

function RowMenu({ name, items }: { name: string; items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', esc);
    };
  }, [open]);
  return (
    <span className="cn-menu" ref={ref}>
      <button type="button" className="cn-menu__button" aria-label={`Actions for ${name}`} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
          <circle cx="3.5" cy="8" r="1.2" />
          <circle cx="8" cy="8" r="1.2" />
          <circle cx="12.5" cy="8" r="1.2" />
        </svg>
      </button>
      {open && (
        <div className="cn-menu__list" role="menu" aria-label={`${name} actions`}>
          {items.map((it) => (
            <button key={it.label} type="button" role="menuitem" className={`cn-menu__item${it.danger ? ' cn-menu__item--danger' : ''}`} onClick={() => { setOpen(false); it.onSelect(); }}>
              {it.label}
            </button>
          ))}
        </div>
      )}
    </span>
  );
}

// ------------------------------------------------------------------ one connector

type Mode = 'idle' | 'rename' | 'edit' | 'remove' | 'ssoHint';

interface RowActions {
  run(fn: () => Promise<ConnectorsVM>): Promise<void>;
  signIn(connectorId: string): Promise<void>;
}

function Row({ c, actions, disabled }: { c: ConnectorVM; actions: RowActions; disabled: boolean }) {
  const [mode, setMode] = useState<Mode>('idle');
  const [name, setName] = useState(c.name);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const st = statusOf(c);
  const env = c.kind === 'github' && c.source === 'env';

  const test = async () => {
    setNote(null);
    try {
      const r = await window.ratchet.testConnector(c.id);
      setNote({ ok: r.ok, text: r.detail });
      await actions.run(() => window.ratchet.listConnectors());
    } catch (e) {
      setNote({ ok: false, text: errMsg(e) });
    }
  };

  const items: MenuItem[] = [];
  if (!c.isDefault) items.push({ label: 'Set as default', onSelect: () => void actions.run(() => window.ratchet.setDefaultConnector(c.id)) });
  items.push({ label: 'Check connection', onSelect: () => void test() });
  if (c.kind === 'github' && !env) items.push({ label: 'Sign in again', onSelect: () => void actions.signIn(c.id) });
  if (c.kind === 'aws') items.push({ label: 'Edit region and cap', onSelect: () => setMode('edit') });
  if (!env) items.push({ label: 'Rename', onSelect: () => { setName(c.name); setMode('rename'); } });
  if (!env) items.push({ label: 'Remove', danger: true, onSelect: () => setMode('remove') });

  // A visible way back in for a connector that stopped working (as the design's "Sign in" on an expired account).
  const signInButton =
    c.kind === 'github' && !env && (c.status === 'expired' || c.status === 'signed-out') ? (
      <button type="button" className="cn-btn" disabled={disabled} onClick={() => void actions.signIn(c.id)}>Sign in</button>
    ) : c.kind === 'aws' && c.status === 'expired' ? (
      <button type="button" className="cn-btn" onClick={() => setMode(mode === 'ssoHint' ? 'idle' : 'ssoHint')}>Sign in</button>
    ) : null;

  return (
    <li className="cn-row" data-testid={`connector-${c.id}`} data-kind={c.kind} data-status={c.status} data-default={c.isDefault}>
      <div className="cn-row__line">
        <div className="cn-row__main">
          {mode === 'rename' ? (
            <form
              className="cn-inline"
              onSubmit={(e) => {
                e.preventDefault();
                void actions.run(() => window.ratchet.updateConnector(c.id, { name })).then(() => setMode('idle'), () => undefined);
              }}
            >
              <input aria-label={`Name of ${c.name}`} value={name} autoFocus maxLength={80} onChange={(e) => setName(e.target.value)} />
              <button type="submit" className="cn-btn" disabled={name.trim() === '' || name.trim() === c.name}>Save</button>
              <button type="button" className="cn-link" onClick={() => setMode('idle')}>Cancel</button>
            </form>
          ) : (
            <div className="cn-row__name">{c.name}</div>
          )}
          <div className="cn-row__detail"><Detail text={c.detail} /></div>
        </div>
        {c.isDefault && <span className="cn-default">Default</span>}
        {signInButton}
        <span className="cn-status"><i className={`cn-dot cn-dot--${st.tone}`} aria-hidden="true" />{st.word}</span>
        <RowMenu name={c.name} items={items} />
      </div>

      {mode === 'remove' && (
        <div className="cn-row__extra" role="group" aria-label={`Remove ${c.name}`}>
          <span>Remove {c.name}? Its {c.kind === 'aws' ? 'saved choice' : 'token or key'} is deleted from this Mac{c.kind === 'agent' && c.source === 'login' ? '; the CLI itself stays signed in' : ''}.</span>
          <button type="button" className="cn-btn cn-btn--danger" disabled={disabled} onClick={() => void actions.run(() => window.ratchet.removeConnector(c.id))}>Remove</button>
          <button type="button" className="cn-link" onClick={() => setMode('idle')}>Keep</button>
        </div>
      )}
      {mode === 'ssoHint' && c.kind === 'aws' && (
        <div className="cn-row__extra" role="status">
          <span>Run <span className="mono">aws sso login --profile {c.profile}</span> in a terminal, then check again.</span>
          <button type="button" className="cn-btn" onClick={() => void test()}>Check again</button>
        </div>
      )}
      {mode === 'edit' && c.kind === 'aws' && <AwsEdit c={c} actions={actions} done={() => setMode('idle')} />}
      {note && <div className={`cn-row__extra cn-note cn-note--${note.ok ? 'ok' : 'bad'}`} role="status">{note.text}</div>}
    </li>
  );
}

function AwsEdit({ c, actions, done }: { c: AwsConnectorVM; actions: RowActions; done(): void }) {
  const [region, setRegion] = useState(c.region);
  const [cap, setCap] = useState(c.monthlyCapUsd === null ? '' : String(c.monthlyCapUsd));
  const regionProblem = validateLiveSetting('awsRegion', region);
  const capNum = cap.trim() === '' ? null : Number(cap);
  const capProblem = capNum !== null && (!Number.isFinite(capNum) || capNum <= 0) ? 'Use a positive number of dollars' : null;
  return (
    <form
      className="cn-row__extra cn-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (regionProblem || capProblem || region.trim() === '') return;
        void actions.run(() => window.ratchet.updateConnector(c.id, { region: region.trim(), monthlyCapUsd: capNum })).then(done, () => undefined);
      }}
    >
      <label className="cn-field">
        <span>Region</span>
        <input className="mono" aria-label="Region" aria-invalid={regionProblem ? true : undefined} value={region} spellCheck={false} onChange={(e) => setRegion(e.target.value)} />
      </label>
      <label className="cn-field">
        <span>Monthly cap (USD)</span>
        <input aria-label="Monthly cap in dollars" inputMode="decimal" placeholder="none" aria-invalid={capProblem ? true : undefined} value={cap} onChange={(e) => setCap(e.target.value)} />
      </label>
      <button type="submit" className="cn-btn" disabled={!!regionProblem || !!capProblem || region.trim() === ''}>Save</button>
      <button type="button" className="cn-link" onClick={done}>Cancel</button>
      {(regionProblem || capProblem) && <span className="cn-problem" role="alert">{regionProblem ?? capProblem}</span>}
    </form>
  );
}

// ------------------------------------------------------------------ the add flow

type Step = 'kind' | ConnectorKind;

function AddConnector({ vm, initial, onClose, onChange }: { vm: ConnectorsVM; initial: Step; onClose(): void; onChange(v: ConnectorsVM): void }) {
  const [step, setStep] = useState<Step>(initial);
  return (
    <section className="cn-add" aria-label="Add a connector">
      <div className="cn-add__head">
        <h2>{step === 'kind' ? 'Add a connector' : step === 'github' ? 'Add a GitHub account' : step === 'aws' ? 'Add an AWS profile' : 'Add an agent'}</h2>
        <button type="button" className="cn-link" onClick={step === 'kind' ? onClose : () => setStep('kind')}>{step === 'kind' ? 'Cancel' : 'Back'}</button>
      </div>
      {step === 'kind' && (
        <div className="cn-kinds" role="group" aria-label="Kind of connector">
          <button type="button" className="cn-kind" onClick={() => setStep('github')}>
            <span className="cn-kind__name">GitHub account</span>
            <span className="cn-kind__sub">Sign in, or paste a token for an organization</span>
          </button>
          <button type="button" className="cn-kind" onClick={() => setStep('aws')}>
            <span className="cn-kind__name">AWS profile</span>
            <span className="cn-kind__sub">From ~/.aws, with a region and an optional cap</span>
          </button>
          <button type="button" className="cn-kind" onClick={() => setStep('agent')}>
            <span className="cn-kind__name">Agent</span>
            <span className="cn-kind__sub">A Claude Code or Codex login, or an API key</span>
          </button>
        </div>
      )}
      {step === 'github' && <AddGithub vm={vm} onChange={onChange} onDone={onClose} />}
      {step === 'aws' && <AddAws vm={vm} onChange={onChange} onDone={onClose} />}
      {step === 'agent' && <AddAgent vm={vm} onChange={onChange} onDone={onClose} />}
    </section>
  );
}

function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async <T,>(fn: () => Promise<T>): Promise<T | undefined> => {
    setBusy(true);
    setError(null);
    try {
      return await fn();
    } catch (e) {
      setError(errMsg(e));
      return undefined;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

function AddGithub({ vm, onChange, onDone }: { vm: ConnectorsVM; onChange(v: ConnectorsVM): void; onDone(): void }) {
  const { busy, error, run } = useAction();
  const [token, setToken] = useState('');
  const sawPending = useRef(false);
  const cancelled = useRef(false);
  const flow = vm.githubFlow;
  // The device flow finished in the background (the token is stored): the panel's job is done.
  useEffect(() => {
    if (flow?.state === 'pending') sawPending.current = true;
    else if (flow === null && sawPending.current && !cancelled.current) onDone();
  }, [flow, onDone]);
  const problem = token === '' ? null : validateSecretFormat('githubToken', token);
  return (
    <div className="cn-add__body">
      {flow?.state === 'pending' ? (
        <div className="cn-code" role="status" aria-label="GitHub sign-in in progress">
          <span>Enter this code at <span className="mono">{flow.verificationUri.replace(/^https:\/\//, '')}</span>, then approve Ratchet.</span>
          <span className="cn-code__value mono" data-testid="device-code">{flow.userCode}</span>
          <span className="cn-muted">Waiting for you to approve in the browser…</span>
          <button type="button" className="cn-link" onClick={() => { cancelled.current = true; void run(() => window.ratchet.cancelGithubConnectorSignIn().then(onChange)); }}>Cancel</button>
        </div>
      ) : (
        <>
          {flow && <p className="cn-problem" role="alert">{flow.message ?? 'Sign-in did not finish.'}</p>}
          <div className="cn-actions">
            <Button disabled={busy} onClick={() => void run(async () => { cancelled.current = false; onChange(await window.ratchet.startGithubConnectorSignIn()); })}>Sign in with GitHub</Button>
            {!vm.clientIdConfigured && <span className="cn-muted">Using the built-in placeholder OAuth client id: set <span className="mono">RATCHET_GITHUB_CLIENT_ID</span> to your OAuth app.</span>}
          </div>
        </>
      )}
      <details className="cn-more">
        <summary>Use an access token instead</summary>
        {!vm.encryptionAvailable && <p className="cn-problem" role="alert">The OS keychain is unavailable, so a token cannot be stored. GITHUB_TOKEN from the environment is still honoured.</p>}
        <form
          className="cn-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (problem || token.length < 8) return;
            void run(async () => { onChange(await window.ratchet.addGithubConnector({ token })); setToken(''); onDone(); });
          }}
        >
          <label className="cn-field cn-field--wide">
            <span>Token</span>
            <input type="password" autoComplete="off" spellCheck={false} aria-label="GitHub token" aria-invalid={problem ? true : undefined} placeholder="ghp_… or github_pat_…" value={token} disabled={!vm.encryptionAvailable} onChange={(e) => setToken(e.target.value)} />
          </label>
          <button type="submit" className="cn-btn" disabled={busy || !vm.encryptionAvailable || token.length < 8 || problem !== null}>Add</button>
          {problem && <span className="cn-problem" role="alert">{problem}</span>}
        </form>
        <p className="cn-muted">For an organization, use a fine-grained token or a GitHub App installation token with repository access. It is checked, then kept in your keychain.</p>
      </details>
      {error && <p className="cn-problem" role="alert">{error}</p>}
    </div>
  );
}

function AddAws({ vm, onChange, onDone }: { vm: ConnectorsVM; onChange(v: ConnectorsVM): void; onDone(): void }) {
  const { busy, error, run } = useAction();
  const q = useQuery(() => window.ratchet.awsProfiles(), []);
  const list: AwsProfilesVM | null = q.data;
  const have = new Set(vm.connectors.filter((c): c is AwsConnectorVM => c.kind === 'aws').map((c) => c.profile));
  const [pick, setPick] = useState('');
  const [region, setRegion] = useState('us-east-1');
  const [cap, setCap] = useState('');
  const regionProblem = validateLiveSetting('awsRegion', region);
  const capNum = cap.trim() === '' ? null : Number(cap);
  const capProblem = capNum !== null && (!Number.isFinite(capNum) || capNum <= 0) ? 'Use a positive number of dollars' : null;
  const available = list?.profiles.filter((p) => !have.has(p.name)) ?? [];
  const KIND: Record<string, string> = { sso: 'SSO', role: 'assume role', keys: 'access keys', process: 'credential process', other: 'profile' };
  return (
    <div className="cn-add__body">
      {list === null ? (
        <p className="cn-muted">Looking for profiles…</p>
      ) : available.length === 0 ? (
        <p className="cn-muted" role="status">
          {list.profiles.length > 0
            ? 'Every profile in ~/.aws is already connected.'
            : list.filesFound
              ? 'No profiles found in ~/.aws/config or ~/.aws/credentials.'
              : 'No ~/.aws/config or ~/.aws/credentials on this machine.'}{' '}
          Run <span className="mono">aws configure sso</span> in a terminal, then rescan.
        </p>
      ) : (
        <div role="radiogroup" aria-label="AWS profile" className="cn-choices">
          {available.map((p) => (
            <button key={p.name} type="button" role="radio" aria-checked={pick === p.name} className={`cn-choice${pick === p.name ? ' cn-choice--on' : ''}`} onClick={() => setPick(p.name)}>
              <span className="cn-choice__name mono">{p.name}</span>
              <span className="cn-choice__hint">{KIND[p.kind]}{p.ssoSession ? ` · session ${p.ssoSession}` : ''}</span>
            </button>
          ))}
        </div>
      )}
      {list && list.ssoSessions.length > 0 && <p className="cn-muted">SSO sessions found: <span className="mono">{list.ssoSessions.join(', ')}</span>. Pick a profile that uses one.</p>}
      <form
        className="cn-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (!pick || regionProblem || capProblem || region.trim() === '') return;
          void run(async () => {
            onChange(await window.ratchet.addAwsConnector({ profile: pick, region: region.trim(), ...(capNum !== null ? { monthlyCapUsd: capNum } : {}) }));
            onDone();
            void window.ratchet.refreshConnectors().then(onChange, () => undefined);
          });
        }}
      >
        <label className="cn-field">
          <span>Region</span>
          <input className="mono" aria-label="AWS region" aria-invalid={regionProblem ? true : undefined} value={region} spellCheck={false} onChange={(e) => setRegion(e.target.value)} />
        </label>
        <label className="cn-field">
          <span>Monthly cap (USD), optional</span>
          <input aria-label="Monthly cap in dollars" inputMode="decimal" placeholder="none" aria-invalid={capProblem ? true : undefined} value={cap} onChange={(e) => setCap(e.target.value)} />
        </label>
        <button type="submit" className="cn-btn cn-btn--primary" disabled={busy || !pick || !!regionProblem || !!capProblem || region.trim() === ''}>Add</button>
        <button type="button" className="cn-link" onClick={() => q.refresh()}>Rescan</button>
        {(regionProblem || capProblem) && <span className="cn-problem" role="alert">{regionProblem ?? capProblem}</span>}
      </form>
      <p className="cn-muted">Only the profile name is stored; credentials stay where AWS keeps them. A repository can override the region in ratchet.yaml.</p>
      {error && <p className="cn-problem" role="alert">{error}</p>}
    </div>
  );
}

function AddAgent({ vm, onChange, onDone }: { vm: ConnectorsVM; onChange(v: ConnectorsVM): void; onDone(): void }) {
  const { busy, error, run } = useAction();
  const q = useQuery(() => window.ratchet.detectHarnesses(), []);
  const [found, setFound] = useState<HarnessVM[] | null>(null);
  useEffect(() => { if (q.data) setFound(q.data); }, [q.data]);
  const present = new Set(vm.connectors.filter((c): c is AgentConnectorVM => c.kind === 'agent' && c.source === 'login').map((c) => c.agent));
  const addable = (found ?? []).filter((h) => h.installed && !present.has(h.kind));
  const [agent, setAgent] = useState<ConnectorAgent>('claude-code');
  const [key, setKey] = useState('');
  const [name, setName] = useState('');
  const secret = agent === 'claude-code' ? 'anthropicApiKey' : 'openaiApiKey';
  const problem = key === '' ? null : validateSecretFormat(secret, key);
  return (
    <div className="cn-add__body">
      <div className="cn-sub">
        <div className="cn-sub__title">Found on this Mac</div>
        {found === null ? (
          <p className="cn-muted">Looking…</p>
        ) : addable.length > 0 ? (
          <ul className="cn-found">
            {addable.map((h) => (
              <li key={h.kind}>
                <span>{AGENT_NAME[h.kind]}{h.version ? ` · v${h.version}` : ''}</span>
                <button type="button" className="cn-btn" disabled={busy} onClick={() => void run(async () => { onChange(await window.ratchet.addAgentConnector({ agent: h.kind, source: 'login' })); onDone(); })}>Add your login</button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="cn-muted" role="status">{found.some((h) => h.installed) ? 'Your Claude Code and Codex logins are already listed.' : 'Neither Claude Code nor Codex was found.'}</p>
        )}
        <button type="button" className="cn-link" onClick={() => void run(async () => { setFound(await window.ratchet.redetectHarnesses()); onChange(await window.ratchet.listConnectors()); })}>Re-detect</button>
      </div>
      <div className="cn-sub">
        <div className="cn-sub__title">Or paste an API key</div>
        <p className="cn-muted">A key can also run in the cloud, with your laptop closed. It is kept in the OS keychain.</p>
        {!vm.encryptionAvailable && <p className="cn-problem" role="alert">The OS keychain is unavailable, so an API key cannot be stored. ANTHROPIC_API_KEY and OPENAI_API_KEY from the environment are still honoured.</p>}
        <form
          className="cn-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (problem || key.length < 8) return;
            void run(async () => {
              onChange(await window.ratchet.addAgentConnector({ agent, source: 'api-key', apiKey: key, ...(name.trim() ? { name: name.trim() } : {}) }));
              setKey('');
              onDone();
              void window.ratchet.refreshConnectors().then(onChange, () => undefined);
            });
          }}
        >
          <label className="cn-field">
            <span>Agent</span>
            <select aria-label="Agent" value={agent} onChange={(e) => setAgent(e.target.value as ConnectorAgent)}>
              <option value="claude-code">Claude Code (Anthropic key)</option>
              <option value="codex">Codex (OpenAI key)</option>
            </select>
          </label>
          <label className="cn-field cn-field--wide">
            <span>API key</span>
            <input type="password" autoComplete="off" spellCheck={false} aria-label="API key" aria-invalid={problem ? true : undefined} value={key} disabled={!vm.encryptionAvailable} onChange={(e) => setKey(e.target.value)} />
          </label>
          <label className="cn-field">
            <span>Name, optional</span>
            <input aria-label="Name" placeholder={`${AGENT_NAME[agent]} · team key`} maxLength={80} value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <button type="submit" className="cn-btn cn-btn--primary" disabled={busy || !vm.encryptionAvailable || key.length < 8 || problem !== null}>Add</button>
          {problem && <span className="cn-problem" role="alert">{problem}</span>}
        </form>
      </div>
      {error && <p className="cn-problem" role="alert">{error}</p>}
    </div>
  );
}

// ------------------------------------------------------------------ Advanced (development)

const DEV_KEYS: LiveSettingKey[] = ['runnerImageRef', 'benchImageRef', 'tofuBinary', 'moduleDir'];
const DEV_LABELS: Record<string, { label: string; placeholder?: string; hint: string }> = {
  runnerImageRef: { label: 'Runner AMI', placeholder: 'ami-0123456789abcdef0', hint: 'Overrides the tag lookup for cloud agent runners.' },
  benchImageRef: { label: 'Bench AMI', placeholder: 'ami-0123456789abcdef0', hint: 'Overrides the tag lookup for the evaluator instances.' },
  tofuBinary: { label: 'OpenTofu binary', hint: 'Empty = the bundled one.' },
  moduleDir: { label: 'OpenTofu module directory', hint: 'Empty = the bundled infra/aws.' },
};

function DevForm({ s, onSave }: { s: SettingsVM; onSave(input: Partial<LiveSettings>): Promise<void> }) {
  const saved = useMemo(() => Object.fromEntries(DEV_KEYS.map((k) => [k, s.dev[k]])) as Record<LiveSettingKey, string>, [s.dev]);
  const savedKey = JSON.stringify(saved);
  const [draft, setDraft] = useState(saved);
  const [draftOf, setDraftOf] = useState(savedKey);
  if (draftOf !== savedKey) {
    setDraftOf(savedKey);
    setDraft(saved);
  }
  const [busy, setBusy] = useState(false);
  const errors = Object.fromEntries(DEV_KEYS.map((k) => [k, validateLiveSetting(k, draft[k])])) as Record<LiveSettingKey, string | null>;
  const invalid = DEV_KEYS.some((k) => errors[k]);
  const dirty = DEV_KEYS.some((k) => draft[k].trim() !== saved[k]);
  return (
    <form className="live-form" onSubmit={(e) => {
      e.preventDefault();
      if (invalid || !dirty) return;
      setBusy(true);
      void onSave(Object.fromEntries(DEV_KEYS.filter((k) => draft[k].trim() !== saved[k]).map((k) => [k, draft[k]]))).catch(() => undefined).finally(() => setBusy(false));
    }}>
      <div className="live-grid">
        {DEV_KEYS.map((k) => (
          <label key={k} className="field">
            <span className="field__label">{DEV_LABELS[k]!.label}</span>
            <input id={`live-${k}`} className={`mono${errors[k] ? ' invalid' : ''}`} value={draft[k]} aria-label={DEV_LABELS[k]!.label} aria-invalid={errors[k] ? true : undefined} spellCheck={false} autoComplete="off"
              placeholder={DEV_LABELS[k]!.placeholder ?? (k === 'tofuBinary' ? s.dev.defaults.tofuBinary : k === 'moduleDir' ? s.dev.defaults.moduleDir : '')}
              onChange={(e) => setDraft((d) => ({ ...d, [k]: e.target.value }))} />
            {errors[k] ? <span className="field__error" role="alert">{errors[k]}</span> : <span className="field__hint">{DEV_LABELS[k]!.hint}</span>}
          </label>
        ))}
      </div>
      <div className="live-actions">
        <Button type="submit" variant="ghost" disabled={!dirty || invalid || busy}>Save</Button>
        {dirty && !invalid && <span className="muted">Unsaved changes</span>}
      </div>
    </form>
  );
}

function Advanced() {
  const q = useQuery(() => window.ratchet.getSettings(), []);
  const [s, setS] = useState<SettingsVM | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { if (q.data) setS(q.data); }, [q.data]);
  return (
    <details className="cn-advanced">
      <summary>Advanced (development)</summary>
      <p className="cn-muted">Overrides for working on Ratchet itself. Normally the images are found by tag for this app version and the OpenTofu module ships with the app.</p>
      <ErrorNote error={error ?? q.error} />
      {s && (
        <DevForm
          s={s}
          onSave={async (input) => {
            setError(null);
            try {
              setS(await window.ratchet.saveLiveSettings(input));
            } catch (e) {
              setError(errMsg(e));
              throw e;
            }
          }}
        />
      )}
    </details>
  );
}

// ------------------------------------------------------------------ the screen

const GROUPS: Array<{ kind: ConnectorKind; title: string; empty: string }> = [
  { kind: 'github', title: 'GitHub', empty: 'No GitHub account yet. Ratchet pushes candidate branches and opens the pull request as you.' },
  { kind: 'aws', title: 'AWS', empty: 'No AWS profile yet. The evaluator runs in a sandbox in your AWS account, removed after each run.' },
  { kind: 'agent', title: 'Agents', empty: 'No agent yet. Install Claude Code or Codex on this Mac, or add an API key.' },
];

export function Connectors() {
  const [flowPending, setFlowPending] = useState(false);
  const q = useQuery(() => window.ratchet.listConnectors(), [], { intervalMs: flowPending ? 2_500 : undefined });
  const [vm, setVm] = useState<ConnectorsVM | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [add, setAdd] = useState<{ step: Step; n: number } | null>(null);
  const [busy, setBusy] = useState(false);

  const adopt = (v: ConnectorsVM) => {
    setVm(v);
    setFlowPending(v.githubFlow?.state === 'pending');
  };
  useEffect(() => { if (q.data) adopt(q.data); }, [q.data]);

  // Check every connector with its provider in the background; the list shows what it knows meanwhile.
  useEffect(() => {
    let live = true;
    window.ratchet.refreshConnectors().then((v) => live && adopt(v), () => undefined);
    return () => { live = false; };
  }, []);

  const run = async (fn: () => Promise<ConnectorsVM>): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      adopt(await fn());
    } catch (e) {
      setError(errMsg(e));
      throw e;
    } finally {
      setBusy(false);
    }
  };
  const actions: RowActions = {
    run: (fn) => run(fn).catch(() => undefined),
    signIn: async (connectorId) => {
      setAdd((a) => ({ step: 'github', n: (a?.n ?? 0) + 1 }));
      await run(() => window.ratchet.startGithubConnectorSignIn({ connectorId })).catch(() => undefined);
    },
  };

  if (q.error && !vm) return <ErrorNote error={q.error} />;
  if (!vm) return <p className="muted">Loading…</p>;

  return (
    <div className="cn" data-testid="connectors">
      <PageHeader
        title="Connectors"
        sub="Accounts each optimization can use. Secrets stay in your keychain."
        actions={<Button onClick={() => setAdd((a) => (a ? null : { step: 'kind', n: Date.now() }))} aria-expanded={add !== null}>Add connector</Button>}
      />
      <ErrorNote error={error} />
      {add && <AddConnector key={add.n} vm={vm} initial={add.step} onChange={adopt} onClose={() => setAdd(null)} />}
      {!vm.encryptionAvailable && !add && <p className="cn-problem" role="alert">The OS keychain is unavailable, so tokens and API keys cannot be stored. GITHUB_TOKEN, ANTHROPIC_API_KEY and OPENAI_API_KEY from the environment are still honoured.</p>}
      {GROUPS.map((g) => {
        const rows = vm.connectors.filter((c) => c.kind === g.kind);
        return (
          <section key={g.kind} className="cn-group" data-testid={`connectors-${g.kind}`} aria-label={g.title}>
            <h2>{g.title}</h2>
            {rows.length > 0 ? <ul className="cn-list">{rows.map((c) => <Row key={c.id} c={c} actions={actions} disabled={busy} />)}</ul> : <p className="cn-empty">{g.empty}</p>}
          </section>
        );
      })}
      <Advanced />
    </div>
  );
}
