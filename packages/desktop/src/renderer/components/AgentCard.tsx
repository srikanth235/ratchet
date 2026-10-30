import { useState } from 'react';
import { AgentAvatar, Button, Card, EvalBars, Pill, agentLabel, type Bar, type PillTone } from '@ratchet/ui';
import type { AgentVM, Verdict } from '../../shared/types';
import { errMsg } from '../data';
import { fmtMetric, fmtUsd } from '../format';
import { Icon } from './icons';
import { LiveLog } from './LiveLog';

const STATUS_TONE: Record<AgentVM['status'], PillTone> = {
  pending: 'neutral', launching: 'brass', running: 'brass', paused: 'neutral', stopped: 'neutral', done: 'mint', failed: 'rose',
};
const STATUS_LABEL: Record<AgentVM['status'], string> = {
  pending: 'Pending', launching: 'Launching', running: 'Running', paused: 'Paused', stopped: 'Stopped', done: 'Done', failed: 'Failed',
};

export const VERDICT_TONE: Record<Verdict, Bar['tone']> = {
  verified: 'mint', rejected: 'neutral', failed_confirm: 'neutral', blocked: 'rose', pending: 'brass',
};
const RING: Record<string, string> = { 'claude-code': 'rgba(184,166,255,.45)', codex: 'rgba(124,196,242,.45)', mock: 'rgba(133,146,166,.45)' };
const TINT: Record<string, string> = { 'claude-code': 'rgba(184,166,255,.32)', codex: 'rgba(124,196,242,.32)', mock: 'rgba(133,146,166,.32)' };

export function AgentCard({ agent, unit = '', metric = '', best = null, onChanged }: { agent: AgentVM; unit?: string; metric?: string; best?: number | null; onChanged?: () => void }) {
  const [text, setText] = useState('');
  const [composing, setComposing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState<string | null>(null);
  const active = agent.status === 'running' || agent.status === 'paused' || agent.status === 'launching';
  const last = agent.recent.at(-1);

  async function act(fn: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
      onChanged?.();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  const send = () => {
    const msg = text.trim();
    if (!msg) return;
    void act(async () => {
      await window.ratchet.messageAgent(agent.id, msg);
      setText('');
      setSent(msg);
      setComposing(false);
    });
  };

  return (
    <Card className="agent-card" aria-label={`${agentLabel(agent.kind)} agent`}>
      <div className="agent-card__head">
        <span style={{ display: 'inline-flex', borderRadius: '50%', boxShadow: `0 0 0 1px ${RING[agent.kind]}` }}><AgentAvatar agent={agent.kind} size={42} /></span>
        <div className="agent-card__id">
          <div className="agent-card__name">{agentLabel(agent.kind)}</div>
          <div className="mono agent-card__branch">{agent.runnerInstanceId ? `runner ${agent.runnerInstanceId} · ` : ''}{agent.branch}</div>
        </div>
        <Pill tone={STATUS_TONE[agent.status]} dot>{STATUS_LABEL[agent.status]}</Pill>
      </div>

      <div className="agent-card__body">
        <div className="agent-card__hyp">
          <span className="rt-eyebrow">Current hypothesis</span>
          <p data-testid="hypothesis">{agent.hypothesis ? `“${agent.hypothesis}”` : 'Waiting for the agent’s first message…'}</p>
        </div>

        <div className="agent-card__evals">
          <span className="row-between">
            <span className="rt-eyebrow">Recent evals{metric ? ` · ${metric}` : ''}</span>
            {best !== null && <span className="mono faint" style={{ fontSize: 11 }}>best {fmtMetric(best, unit)}</span>}
          </span>
          <EvalBars
            best={best}
            bars={agent.recent.map((r) => ({ value: r.value, tone: VERDICT_TONE[r.verdict], label: r.label, verdict: r.verdict, tint: TINT[agent.kind] ?? TINT['mock']! }))}
          />
        </div>

        <dl className="stats">
          <div><dt>Spend</dt><dd className="mono">{fmtUsd(agent.stats.spendUsd)}</dd></div>
          <div><dt>Wins</dt><dd className="mono gain">{agent.stats.verified}</dd></div>
          <div><dt>Last eval</dt><dd className="mono" style={{ color: last?.verdict === 'verified' ? 'var(--rt-color-mint)' : 'var(--rt-color-text-secondary)' }}>{last ? `${fmtMetric(last.value, '')} ${last.verdict === 'verified' ? '✓' : last.verdict === 'blocked' ? '⊘' : '✕'}` : '—'}</dd></div>
          <div><dt>Candidates</dt><dd className="mono">{agent.stats.candidates}</dd></div>
        </dl>

        <LiveLog agentId={agent.id} />

        {composing && (
          <form className="agent-card__msg" onSubmit={(e) => { e.preventDefault(); send(); }}>
            <input autoFocus value={text} onChange={(e) => setText(e.target.value)} placeholder="Message this agent…" aria-label={`Message ${agentLabel(agent.kind)}`} disabled={!active} />
            <Button type="submit" variant="subtle" disabled={!active || busy || text.trim() === ''}><Icon name="send" size={13} /> Send</Button>
          </form>
        )}
        <div className="agent-card__actions">
          <Button variant="subtle" disabled={!active} aria-expanded={composing} onClick={() => setComposing((v) => !v)}><Icon name="message" size={13} /> Message</Button>
          {agent.status === 'paused' ? (
            <Button variant="subtle" className="push-right" disabled={busy} onClick={() => act(() => window.ratchet.resumeAgent(agent.id))}><Icon name="play" size={12} /> Resume</Button>
          ) : (
            <Button variant="subtle" className="push-right" disabled={busy || agent.status !== 'running'} onClick={() => act(() => window.ratchet.pauseAgent(agent.id))}><Icon name="pause" size={12} /> Pause</Button>
          )}
        </div>
        {sent && <div className="muted agent-card__sent" role="status">Sent: “{sent}”</div>}
        {error && <p role="alert" className="error-note">{error}</p>}
      </div>
    </Card>
  );
}
