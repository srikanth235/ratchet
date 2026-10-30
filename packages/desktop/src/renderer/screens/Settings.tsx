import { useEffect, useState } from 'react';
import { Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import type { Mode, SecretName, SettingsVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { errMsg, useQuery } from '../data';

const SECRETS: { name: SecretName; label: string; hint: string }[] = [
  { name: 'anthropicApiKey', label: 'Anthropic API key', hint: 'Used by Claude Code agents' },
  { name: 'openaiApiKey', label: 'OpenAI API key', hint: 'Used by Codex agents' },
  { name: 'githubToken', label: 'GitHub token', hint: 'Repo-scoped token to push candidate branches and open the PR' },
];

function SecretRow({ name, label, hint, configured, disabled, onSave, onClear }: {
  name: SecretName; label: string; hint: string; configured: boolean; disabled: boolean;
  onSave(name: SecretName, value: string): Promise<void>; onClear(name: SecretName): Promise<void>;
}) {
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  return (
    <div className="secret-row" data-testid={`secret-${name}`}>
      <div className="secret-row__info">
        <div className="secret-row__label">{label}</div>
        <div className="muted">{hint}</div>
      </div>
      <Pill tone={configured ? 'mint' : 'neutral'} dot>{configured ? 'Configured' : 'Not set'}</Pill>
      <input type="password" autoComplete="off" spellCheck={false} aria-label={`${label} value`} placeholder={configured ? '•••••••• (hidden)' : 'Paste to store in the keychain'}
        value={value} disabled={disabled} onChange={(e) => setValue(e.target.value)} />
      <Button variant="ghost" disabled={disabled || busy || value.length < 8}
        onClick={async () => { setBusy(true); try { await onSave(name, value); setValue(''); } catch { /* shown by the screen */ } finally { setBusy(false); } }}>Save</Button>
      <Button variant="danger" disabled={!configured || busy} onClick={async () => { setBusy(true); try { await onClear(name); } catch { /* shown by the screen */ } finally { setBusy(false); } }}>Clear</Button>
    </div>
  );
}

export function Settings() {
  const q = useQuery(() => window.ratchet.getSettings(), []);
  const [s, setS] = useState<SettingsVM | null>(null);
  const [profile, setProfile] = useState('');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (q.data) {
      setS(q.data);
      setProfile(q.data.awsProfile);
    }
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
    { id: 'simulation', label: 'Simulation', text: 'Everything runs on this machine: mock AWS, scripted agent, local bench. No cloud spend.' },
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

      <Card>
        <Eyebrow>AWS</Eyebrow>
        <form className="aws-row" onSubmit={(e) => { e.preventDefault(); void apply(() => window.ratchet.setAwsProfile(profile)).catch(() => undefined); }}>
          <label className="field">
            <span className="field__label">Profile</span>
            <input className="mono" value={profile} onChange={(e) => setProfile(e.target.value)} placeholder="ratchet-sandbox" aria-label="AWS profile" />
            <span className="field__hint">Name of a profile in ~/.aws/config for the sandbox account. Only the name is stored.</span>
          </label>
          <Button type="submit" variant="ghost" disabled={profile === s.awsProfile}>Save</Button>
        </form>
      </Card>
    </>
  );
}
