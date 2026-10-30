import { useEffect, useMemo, useState } from 'react';
import { Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import { LIVE_SETTING_DEFAULTS, validateLiveSetting, validateSecretFormat, type LiveSettingKey } from '../../shared/live-settings';
import type { ConnectionResultVM, ConnectionTarget, LiveSettings, Mode, SecretName, SettingsVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { errMsg, useQuery } from '../data';

const SECRETS: { name: SecretName; label: string; hint: string; test: ConnectionTarget }[] = [
  { name: 'anthropicApiKey', label: 'Anthropic API key', hint: 'Used by Claude Code agents', test: 'anthropic' },
  { name: 'openaiApiKey', label: 'OpenAI API key', hint: 'Used by Codex agents', test: 'openai' },
  { name: 'githubToken', label: 'GitHub token', hint: 'Repo-scoped token to push candidate branches and open the PR', test: 'github' },
];

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

function SecretRow({ name, label, hint, test, configured, disabled, onSave, onClear }: {
  name: SecretName; label: string; hint: string; test: ConnectionTarget; configured: boolean; disabled: boolean;
  onSave(name: SecretName, value: string): Promise<void>; onClear(name: SecretName): Promise<void>;
}) {
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

const LIVE_KEYS = Object.keys(LIVE_SETTING_DEFAULTS) as LiveSettingKey[];
const pickLive = (l: LiveSettings): LiveSettings => Object.fromEntries(LIVE_KEYS.map((k) => [k, l[k]])) as unknown as LiveSettings;

function TextField({ id, label, hint, value, error, placeholder, mono = true, onChange }: {
  id: LiveSettingKey; label: string; hint?: string; value: string; error: string | null | undefined; placeholder?: string; mono?: boolean; onChange(v: string): void;
}) {
  return (
    <label className="field">
      <span className="field__label">{label}</span>
      <input id={`live-${id}`} className={`${mono ? 'mono' : ''}${error ? ' invalid' : ''}`} value={value} placeholder={placeholder} aria-label={label} aria-invalid={error ? true : undefined}
        spellCheck={false} autoComplete="off" onChange={(e) => onChange(e.target.value)} />
      {error ? <span className="field__error" role="alert">{error}</span> : hint ? <span className="field__hint">{hint}</span> : null}
    </label>
  );
}

/** The live-mode form: AWS, GitHub, images and OpenTofu. Nothing secret is in it; secrets have their own rows. */
function LiveForm({ s, onSave }: { s: SettingsVM; onSave(input: Partial<LiveSettings>): Promise<void> }) {
  const saved = useMemo(() => pickLive(s.live), [s.live]);
  const [draft, setDraft] = useState<LiveSettings>(saved);
  const [busy, setBusy] = useState(false);
  useEffect(() => setDraft(saved), [saved]);

  const errors = Object.fromEntries(LIVE_KEYS.map((k) => [k, validateLiveSetting(k, draft[k])])) as Record<LiveSettingKey, string | null>;
  const invalid = LIVE_KEYS.some((k) => errors[k]);
  const dirty = LIVE_KEYS.some((k) => draft[k].trim() !== saved[k]);
  const set = (k: LiveSettingKey) => (v: string) => setDraft((d) => ({ ...d, [k]: v }));
  const fieldProps = (k: LiveSettingKey) => ({ id: k, value: draft[k] as string, error: errors[k], onChange: set(k) });

  return (
    <form className="live-form" onSubmit={(e) => {
      e.preventDefault();
      if (invalid || !dirty) return;
      setBusy(true);
      void onSave(Object.fromEntries(LIVE_KEYS.filter((k) => draft[k].trim() !== saved[k]).map((k) => [k, draft[k]]))).catch(() => undefined).finally(() => setBusy(false));
    }}>
      <Card>
        <Eyebrow>AWS</Eyebrow>
        <div className="live-grid">
          <TextField {...fieldProps('awsProfile')} label="AWS profile" placeholder="ratchet-sandbox" hint="Name of a profile in ~/.aws/config for the sandbox account. Only the name is stored." />
          <TextField {...fieldProps('awsRegion')} label="AWS region" placeholder="us-east-1" hint="Where the sandbox is created." />
          <div className="field">
            <span className="field__label">Sign-in</span>
            <div className="segmented" role="radiogroup" aria-label="AWS sign-in">
              {(['profile', 'sso'] as const).map((k) => (
                <button key={k} type="button" role="radio" aria-checked={draft.awsAuth === k} onClick={() => setDraft((d) => ({ ...d, awsAuth: k }))}>{k === 'sso' ? 'SSO' : 'Profile'}</button>
              ))}
            </div>
            <span className="field__hint">{draft.awsAuth === 'sso' ? 'Run "aws sso login --profile <name>" in a terminal when the session expires.' : 'Static keys or an assume-role profile.'}</span>
          </div>
        </div>
        <div className="live-actions"><TestButton target="aws" disabled={dirty || s.live.awsProfile === ''} /></div>
      </Card>

      <Card>
        <Eyebrow>GitHub</Eyebrow>
        <div className="live-grid">
          <TextField {...fieldProps('githubRepo')} label="GitHub repository" placeholder="acme/search-api" hint="owner/name. Live optimizations run on this repository; the token is stored above." />
        </div>
        <div className="live-actions"><TestButton target="github" disabled={dirty || s.live.githubRepo === '' || !s.secrets.githubToken} /></div>
      </Card>

      <Card>
        <Eyebrow>Runner images</Eyebrow>
        <div className="live-grid">
          <TextField {...fieldProps('runnerImageRef')} label="Runner AMI" placeholder="ami-0123456789abcdef0" hint="Agent runners boot from this image (Node, git, agent CLIs, ratchet)." />
          <TextField {...fieldProps('benchImageRef')} label="Bench AMI" placeholder="ami-0123456789abcdef0" hint="Bench and confirm instances boot from this image." />
        </div>
      </Card>

      <Card>
        <Eyebrow>OpenTofu</Eyebrow>
        <div className="live-grid">
          <TextField {...fieldProps('tofuBinary')} label="OpenTofu binary" placeholder={s.live.defaults.tofuBinary} hint="Empty = the default shown here." />
          <TextField {...fieldProps('moduleDir')} label="OpenTofu module directory" placeholder={s.live.defaults.moduleDir} hint="Empty = the bundled infra/aws." />
        </div>
        <div className="live-actions"><TestButton target="tofu" disabled={dirty} /></div>
      </Card>

      <div className="live-actions">
        <Button type="submit" variant="ghost" disabled={!dirty || invalid || busy}>Save</Button>
        {dirty && invalid && <span className="field__error">Fix the highlighted fields to save.</span>}
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

  const apply = async (fn: () => Promise<SettingsVM>) => {
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

  const modes: { id: Mode; label: string; text: string }[] = [
    { id: 'simulation', label: 'Simulation', text: 'Everything runs on this machine: mock AWS, scripted agents, local evaluator. No cloud spend.' },
    { id: 'live', label: 'Live', text: 'Real sandbox account, real agents on runners, real PRs. Requires keys and an AWS profile.' },
  ];

  return (
    <>
      <PageHeader title="Settings" sub="Credentials are stored in your OS keychain and never leave the main process." />
      <ErrorNote error={error} />

      <Card>
        <div className="row-between"><Eyebrow>Mode</Eyebrow>
          <Pill tone={s.engine.kind === 'dev' ? 'neutral' : 'brass'}>{s.engine.kind === 'dev' ? 'Dev engine (simulated)' : 'Engine'} · {s.engine.mode}</Pill></div>
        <div className="mode-choices" role="radiogroup" aria-label="Mode">
          {modes.map((m) => (
            <button key={m.id} type="button" role="radio" aria-checked={s.mode === m.id} className={`agent-choice${s.mode === m.id ? ' agent-choice--on' : ''}`}
              onClick={() => void apply(() => window.ratchet.setMode(m.id)).catch(() => undefined)}>
              <span className="agent-choice__name">{m.label}</span>
              <span className="agent-choice__hint">{m.text}</span>
            </button>
          ))}
        </div>
      </Card>

      <Card>
        <div className="row-between">
          <Eyebrow>Live readiness</Eyebrow>
          <Pill tone={s.live.ready ? 'mint' : 'brass'} dot>{s.live.ready ? 'Ready for a live run' : 'Not ready'}</Pill>
        </div>
        <ul className="readiness" aria-label="Live readiness">
          {s.live.readiness.map((r) => (
            <li key={r.id} data-state={r.state}>
              <span className="readiness__label">{r.state === 'ok' ? '✓' : '○'} {r.label}</span>
              <span className="readiness__detail">{r.detail}</span>
            </li>
          ))}
        </ul>
        {s.live.restartRequired && <p role="status" className="muted">Saved. An optimization is running, so the live engine picks up these settings as soon as it has stopped.</p>}
      </Card>

      <Card>
        <Eyebrow>API keys &amp; tokens</Eyebrow>
        {!s.encryptionAvailable && (
          <p role="alert" className="error-note">The OS keychain is unavailable, so secrets cannot be stored. Environment variables (ANTHROPIC_API_KEY, OPENAI_API_KEY, GITHUB_TOKEN) are still honoured.</p>
        )}
        <div className="secrets">
          {SECRETS.map((x) => (
            <SecretRow key={x.name} {...x} configured={s.secrets[x.name]} disabled={!s.encryptionAvailable}
              onSave={(n, v) => apply(() => window.ratchet.setSecret(n, v))}
              onClear={(n) => apply(() => window.ratchet.clearSecret(n))} />
          ))}
        </div>
      </Card>

      <LiveForm s={s} onSave={(input) => apply(() => window.ratchet.saveLiveSettings(input))} />
    </>
  );
}
