import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import { validateLiveSetting, validateSecretFormat, type LiveSettingKey } from '../../shared/live-settings';
import type { AwsProfileVM, AwsProfilesVM, ConnectionResultVM, ConnectionTarget, GithubStatusVM, HarnessVM, LiveSettings, SecretName, SettingsVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { errMsg, useQuery } from '../data';

/** "Test connection": one read-only probe through the main process, which reads the stored secret itself. */
function useConnectionTest(target: ConnectionTarget) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ConnectionResultVM | null>(null);
  const run = async () => {
    setBusy(true);
    setResult(null);
    try {
      setResult(await window.ratchet.testConnection(target));
    } catch (e) {
      setResult({ target, ok: false, detail: errMsg(e) });
    } finally {
      setBusy(false);
    }
  };
  return { busy, result, run };
}

function TestResult({ result }: { result: ConnectionResultVM | null }) {
  if (!result) return null;
  return (
    <span role="status" className={`test-result ${result.ok ? 'test-result--ok' : 'test-result--fail'}`} data-testid={`test-${result.target}`}>
      {result.ok ? '✓' : '✕'} {result.detail}
    </span>
  );
}

function TestButton({ target, label = 'Test connection', disabled = false }: { target: ConnectionTarget; label?: string; disabled?: boolean }) {
  const t = useConnectionTest(target);
  return (
    <span className="test-row">
      <Button variant="subtle" disabled={disabled || t.busy} onClick={() => void t.run()}>{t.busy ? 'Testing…' : label}</Button>
      <TestResult result={t.result} />
    </span>
  );
}

const SECRET_META: Record<SecretName, { label: string; hint: string; test: ConnectionTarget }> = {
  anthropicApiKey: { label: 'Anthropic API key', hint: 'Used by Claude Code', test: 'anthropic' },
  openaiApiKey: { label: 'OpenAI API key', hint: 'Used by Codex', test: 'openai' },
  githubToken: { label: 'GitHub token', hint: 'A personal access token with repo scope', test: 'github' },
};

function SecretRow({ name, configured, disabled, onSave, onClear }: {
  name: SecretName; configured: boolean; disabled: boolean;
  onSave(name: SecretName, value: string): Promise<void>; onClear(name: SecretName): Promise<void>;
}) {
  const { label, hint, test } = SECRET_META[name];
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  const problem = value === '' ? null : validateSecretFormat(name, value);
  return (
    <div className="secret-row" data-testid={`secret-${name}`}>
      <div className="secret-row__info">
        <div className="secret-row__label">{label}</div>
        <div className="muted">{hint}</div>
      </div>
      <Pill tone={configured ? 'mint' : 'neutral'} dot>{configured ? 'Configured' : 'Not set'}</Pill>
      <input type="password" autoComplete="off" spellCheck={false} aria-label={`${label} value`} aria-invalid={problem ? true : undefined} placeholder={configured ? '•••••••• (hidden)' : 'Paste to store in the keychain'}
        value={value} disabled={disabled} onChange={(e) => setValue(e.target.value)} />
      <Button variant="ghost" disabled={disabled || busy || value.length < 8 || problem !== null}
        onClick={async () => { setBusy(true); try { await onSave(name, value); setValue(''); } catch { /* shown by the screen */ } finally { setBusy(false); } }}>Save</Button>
      <Button variant="danger" disabled={!configured || busy} onClick={async () => { setBusy(true); try { await onClear(name); } catch { /* shown by the screen */ } finally { setBusy(false); } }}>Clear</Button>
      {(problem || configured) && (
        <div className="secret-test">
          {problem && <span className="field__error" role="alert">{problem}</span>}
          {configured && !problem && <TestButton target={test} />}
        </div>
      )}
    </div>
  );
}

/** One settings row: title, what it is for, its state on the right, then the body. */
function Row({ id, title, sub, state, children }: { id: string; title: string; sub: string; state: ReactNode; children: ReactNode }) {
  return (
    <Card className="settings-row" data-testid={`row-${id}`}>
      <div className="settings-row__head">
        <div className="settings-row__titles">
          <Eyebrow>{title}</Eyebrow>
          <p className="muted settings-row__sub">{sub}</p>
        </div>
        <div className="settings-row__state">{state}</div>
      </div>
      {children}
    </Card>
  );
}

// ------------------------------------------------------------------ GitHub

function GithubRow({ keychain, onSecret, onClear }: {
  keychain: boolean; onSecret(n: SecretName, v: string): Promise<void>; onClear(n: SecretName): Promise<void>;
}) {
  const [flowPending, setFlowPending] = useState(false);
  const q = useQuery(() => window.ratchet.githubStatus(), [], { intervalMs: flowPending ? 2_500 : undefined });
  const [status, setStatus] = useState<GithubStatusVM | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (q.data) {
      setStatus(q.data);
      setFlowPending(q.data.flow?.state === 'pending' && !q.data.signedIn);
    }
  }, [q.data]);

  const act = async (fn: () => Promise<GithubStatusVM>) => {
    setBusy(true);
    setError(null);
    try {
      const s = await fn();
      setStatus(s);
      setFlowPending(s.flow?.state === 'pending' && !s.signedIn);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  };

  const s = status;
  const flow = s?.flow ?? null;
  return (
    <Row id="github" title="GitHub" sub="Who Ratchet acts as: reads your repositories, pushes candidate branches and opens the pull request."
      state={s ? (s.signedIn ? <Pill tone="mint" dot>{s.login ? `Signed in as ${s.login}` : 'Signed in'}</Pill> : <Pill tone="neutral" dot>Not signed in</Pill>) : <Pill>Checking…</Pill>}>
      <ErrorNote error={error} />
      {s?.signedIn ? (
        <div className="settings-row__actions">
          <span className="muted">{s.source === 'device' ? 'Signed in with GitHub; the token is kept in your OS keychain.' : s.source === 'token' ? 'Using a pasted token, kept in your OS keychain.' : 'Using the GITHUB_TOKEN from the environment.'}</span>
          <TestButton target="github" />
          <Button variant="danger" disabled={busy} onClick={() => void act(() => window.ratchet.githubSignOut())}>Sign out</Button>
        </div>
      ) : (
        <>
          {flow?.state === 'pending' ? (
            <div className="device-code" role="status" aria-label="GitHub sign-in in progress">
              <span className="muted">Enter this code at <span className="mono">{flow.verificationUri.replace(/^https:\/\//, '')}</span>, then approve Ratchet.</span>
              <span className="device-code__code mono" data-testid="device-code">{flow.userCode}</span>
              <span className="muted">Waiting for you to approve in the browser…</span>
            </div>
          ) : (
            flow && <p role="alert" className="error-note">{flow.message ?? 'Sign-in did not finish.'}</p>
          )}
          <div className="settings-row__actions">
            <Button disabled={busy || flow?.state === 'pending'} onClick={() => void act(() => window.ratchet.githubSignIn())}>{flow?.state === 'pending' ? 'Waiting…' : 'Sign in with GitHub'}</Button>
            {s && !s.clientIdConfigured && <span className="muted">Using the built-in placeholder OAuth client id: set <span className="mono">RATCHET_GITHUB_CLIENT_ID</span> to your OAuth app.</span>}
          </div>
        </>
      )}
      <details className="advanced">
        <summary>Advanced: paste a token instead</summary>
        {!keychain && <p role="alert" className="error-note">The OS keychain is unavailable, so a token cannot be stored. GITHUB_TOKEN from the environment is still honoured.</p>}
        <SecretRow name="githubToken" configured={!!s?.signedIn && s.source !== 'device' && s.source !== null} disabled={!keychain}
          onSave={async (n, v) => { await onSecret(n, v); q.refresh(); }} onClear={async (n) => { await onClear(n); q.refresh(); }} />
      </details>
    </Row>
  );
}

// ------------------------------------------------------------------ AWS

const KIND_LABEL: Record<AwsProfileVM['kind'], string> = { sso: 'SSO', role: 'assume role', keys: 'access keys', process: 'credential process', other: 'profile' };

function AwsRow({ s, apply, error }: { s: SettingsVM; apply(fn: () => Promise<SettingsVM>): Promise<void>; error: boolean }) {
  const q = useQuery(() => window.ratchet.awsProfiles(), [s.awsProfile]);
  const [list, setList] = useState<AwsProfilesVM | null>(null);
  const [picking, setPicking] = useState<boolean | null>(null);
  const [region, setRegion] = useState(s.awsRegion);
  useEffect(() => { if (q.data) setList(q.data); }, [q.data]);
  useEffect(() => setRegion(s.awsRegion), [s.awsRegion]);

  const connected = s.awsProfile !== '';
  const open = picking ?? !connected;
  const regionProblem = validateLiveSetting('awsRegion', region);
  const regionDirty = region.trim() !== s.awsRegion;

  return (
    <Row id="aws" title="AWS" sub="Where the evaluator runs: a sandbox in your AWS account, created for a run and removed afterwards."
      state={connected ? <Pill tone="mint" dot>Connected · {s.awsProfile}</Pill> : <Pill tone="neutral" dot>Not connected</Pill>}>
      {open ? (
        <div className="aws-profiles">
          {list && list.profiles.length > 0 ? (
            <div role="radiogroup" aria-label="AWS profile" className="profile-list">
              {list.profiles.map((p) => (
                <button key={p.name} type="button" role="radio" aria-checked={p.name === s.awsProfile} className={`agent-choice profile-choice${p.name === s.awsProfile ? ' agent-choice--on' : ''}`}
                  disabled={error}
                  onClick={() => { setPicking(false); void apply(() => window.ratchet.setAwsProfile(p.name)); }}>
                  <span className="agent-choice__text">
                    <span className="agent-choice__name mono">{p.name}</span>
                    <span className="agent-choice__hint">{KIND_LABEL[p.kind]}{p.ssoSession ? ` · session ${p.ssoSession}` : ''}</span>
                  </span>
                </button>
              ))}
            </div>
          ) : list ? (
            <p className="muted" role="status">
              {list.filesFound ? 'No profiles found in ~/.aws/config or ~/.aws/credentials.' : 'No ~/.aws/config or ~/.aws/credentials on this machine.'} Run <span className="mono">aws configure sso</span> in a terminal, then rescan.
            </p>
          ) : (
            <p className="muted">Looking for profiles…</p>
          )}
          {list && list.ssoSessions.length > 0 && <p className="muted">SSO sessions found: <span className="mono">{list.ssoSessions.join(', ')}</span>. Pick a profile that uses one.</p>}
          <div className="settings-row__actions">
            <Button variant="ghost" onClick={() => q.refresh()}>Rescan</Button>
            {connected && <Button variant="subtle" onClick={() => setPicking(false)}>Cancel</Button>}
          </div>
        </div>
      ) : (
        <div className="settings-row__actions">
          <span className="muted">Profile <span className="mono">{s.awsProfile}</span>. Only the name is stored; credentials stay where AWS keeps them.</span>
          <Button variant="ghost" onClick={() => setPicking(true)}>Change profile</Button>
        </div>
      )}
      <div className="live-grid">
        <label className="field">
          <span className="field__label">Region</span>
          <input id="aws-region" className={`mono${regionProblem ? ' invalid' : ''}`} value={region} aria-label="AWS region" aria-invalid={regionProblem ? true : undefined} spellCheck={false} autoComplete="off"
            onChange={(e) => setRegion(e.target.value)} />
          {regionProblem ? <span className="field__error" role="alert">{regionProblem}</span> : <span className="field__hint">Default us-east-1; a repo can override it in ratchet.yaml.</span>}
        </label>
      </div>
      <div className="settings-row__actions">
        {regionDirty && <Button variant="ghost" disabled={regionProblem !== null || region.trim() === ''} onClick={() => void apply(() => window.ratchet.saveLiveSettings({ awsRegion: region.trim() }))}>Save region</Button>}
        <TestButton target="aws" disabled={!connected || regionDirty} />
      </div>
    </Row>
  );
}

// ------------------------------------------------------------------ agents

const HARNESS_NAME: Record<HarnessVM['kind'], string> = { 'claude-code': 'Claude Code', codex: 'Codex' };
const HARNESS_SECRET: Record<HarnessVM['kind'], SecretName> = { 'claude-code': 'anthropicApiKey', codex: 'openaiApiKey' };

function authPill(h: HarnessVM) {
  if (!h.installed) return <Pill tone="neutral" dot>Not installed</Pill>;
  if (h.auth === 'signed-in') return <Pill tone="mint" dot>Signed in</Pill>;
  if (h.auth === 'api-key') return <Pill tone="mint" dot>API key</Pill>;
  return <Pill tone="brass" dot>Signed out</Pill>;
}

function AgentsRow({ s, apply }: { s: SettingsVM; apply(fn: () => Promise<SettingsVM>): Promise<void> }) {
  const q = useQuery(() => window.ratchet.detectHarnesses(), []);
  const [list, setList] = useState<HarnessVM[] | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (q.data) setList(q.data); }, [q.data]);
  const redetect = async () => {
    setBusy(true);
    try {
      setList(await window.ratchet.redetectHarnesses());
    } finally {
      setBusy(false);
    }
  };
  const ready = list?.filter((h) => h.installed && h.auth !== 'signed-out').length ?? 0;
  const keys = (names: SecretName[]) =>
    names.map((n) => (
      <SecretRow key={n} name={n} configured={s.secrets[n]} disabled={!s.encryptionAvailable}
        onSave={(name, v) => apply(() => window.ratchet.setSecret(name, v))} onClear={(name) => apply(() => window.ratchet.clearSecret(name))} />
    ));
  return (
    <Row id="agents" title="Agents" sub="The Claude Code and Codex CLIs installed on this machine, with the login you already have."
      state={list ? <Pill tone={ready > 0 ? 'mint' : 'neutral'} dot>{ready > 0 ? `${ready} ready` : 'None ready'}</Pill> : <Pill>Detecting…</Pill>}>
      {!s.encryptionAvailable && <p role="alert" className="error-note">The OS keychain is unavailable, so API keys cannot be stored. ANTHROPIC_API_KEY and OPENAI_API_KEY from the environment are still honoured.</p>}
      <div className="harnesses" role="list" aria-label="Detected agents">
        {list?.map((h) => (
          <div key={h.kind} role="listitem" className="harness" data-testid={`harness-${h.kind}`} data-auth={h.auth} data-installed={h.installed}>
            <div className="harness__head">
              <AgentAvatar agent={h.kind} size={34} />
              <span className="harness__titles">
                <span className="harness__name">{HARNESS_NAME[h.kind]}</span>
                <span className="muted mono harness__version">{h.installed ? (h.version ? `v${h.version}` : 'version unknown') : 'not found'}</span>
              </span>
              {authPill(h)}
            </div>
            {h.path && <span className="faint mono harness__path">{h.path}</span>}
            {(!h.installed || h.auth === 'signed-out') && <p className="harness__hint" role="status">{h.hint}</p>}
            {(!h.installed || h.auth === 'signed-out') && <div className="harness__key">{keys([HARNESS_SECRET[h.kind]])}</div>}
          </div>
        ))}
      </div>
      <div className="settings-row__actions">
        <Button variant="ghost" disabled={busy} onClick={() => void redetect()}>{busy ? 'Detecting…' : 'Re-detect'}</Button>
        <span className="muted">Probed when the app starts. Install or sign in, then re-detect.</span>
      </div>
      <details className="advanced">
        <summary>Cloud runners need API keys</summary>
        <p className="muted">Agents that run in the cloud (unattended, laptop closed) cannot use your local login. Keys are kept in the OS keychain.</p>
        <div className="secrets">{keys(['anthropicApiKey', 'openaiApiKey'])}</div>
      </details>
    </Row>
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

export function Settings() {
  const q = useQuery(() => window.ratchet.getSettings(), []);
  const [s, setS] = useState<SettingsVM | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (q.data) setS(q.data);
  }, [q.data]);

  const apply = async (fn: () => Promise<SettingsVM>): Promise<void> => {
    setError(null);
    try {
      setS(await fn());
    } catch (e) {
      setError(errMsg(e));
      throw e;
    }
  };

  if (q.error && !s) return <ErrorNote error={q.error} />;
  if (!s) return <p className="muted">Loading…</p>;

  return (
    <>
      <PageHeader title="Settings" sub="Who you are. What to optimize lives in the repository. Credentials are kept in your OS keychain and never leave the main process." />
      <ErrorNote error={error} />
      <GithubRow keychain={s.encryptionAvailable}
        onSecret={(n, v) => apply(() => window.ratchet.setSecret(n, v))} onClear={(n) => apply(() => window.ratchet.clearSecret(n))} />
      <AwsRow s={s} apply={(fn) => apply(fn).catch(() => undefined)} error={false} />
      <AgentsRow s={s} apply={apply} />
      <details className="advanced advanced--card">
        <summary>Advanced (development)</summary>
        <p className="muted">Overrides for working on Ratchet itself. Normally the images are found by tag for this app version and the OpenTofu module ships with the app.</p>
        <DevForm s={s} onSave={(input) => apply(() => window.ratchet.saveLiveSettings(input))} />
      </details>
    </>
  );
}
