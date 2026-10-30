import { useState } from 'react';
import { AgentAvatar, BarSpark, Button, Card, Pill, agentLabel, type PillTone, type Bar } from '@ratchet/ui';
import type { AgentVM, Verdict } from '../../shared/types';
import { errMsg } from '../data';
import { fmtUsd } from '../format';
import { Icon } from './icons';
import { LiveLog } from './LiveLog';

const STATUS_TONE: Record<AgentVM['status'], PillTone> = {
  pending: 'neutral', launching: 'brass', running: 'brass', paused: 'neutral', stopped: 'neutral', done: 'mint', failed: 'rose',
};

export const VERDICT_TONE: Record<Verdict, Bar['tone']> = {
  verified: 'mint', rejected: 'neutral', failed_confirm: 'rose', blocked: 'rose', pending: 'brass',
};

export function AgentCard({ agent, onChanged }: { agent: AgentVM; onChanged?: () => void }) {
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState<string | null>(null);
  const active = agent.status === 'running' || agent.status === 'paused' || agent.status === 'launching';

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
    });
  };

  return (
    <Card className="agent-card" aria-label={`${agentLabel(agent.kind)} agent`}>
      <div className="agent-card__head">
        <AgentAvatar agent={agent.kind} size={32} />
        <div className="agent-card__id">
          <div className="agent-card__name">{agentLabel(agent.kind)}</div>
          <div className="mono muted agent-card__branch">{agent.branch}</div>
        </div>
        <Pill tone={STATUS_TONE[agent.status]} dot live={agent.status === 'running'}>{agent.status}</Pill>
      </div>

      <div className="agent-card__hyp">
        <span className="rt-eyebrow">Hypothesis</span>
        <p data-testid="hypothesis">{agent.hypothesis ?? 'Waiting for the agent’s first message…'}</p>
      </div>

      <div className="agent-card__evals">
        <span className="rt-eyebrow">Recent evals</span>
        <BarSpark
          width={180}
          height={34}
          bars={agent.recent.map((r) => ({ value: r.value, tone: VERDICT_TONE[r.verdict], label: r.label }))}
        />
      </div>

      <dl className="stats">
        <div><dt>Candidates</dt><dd className="mono">{agent.stats.candidates}</dd></div>
        <div><dt>Verified</dt><dd className="mono gain">{agent.stats.verified}</dd></div>
        <div><dt>Rejected</dt><dd className="mono">{agent.stats.rejected}</dd></div>
        <div><dt>Blocked</dt><dd className="mono">{agent.stats.blocked}</dd></div>
        <div><dt>Spend</dt><dd className="mono">{fmtUsd(agent.stats.spendUsd)}</dd></div>
      </dl>

      <LiveLog agentId={agent.id} />

      <form className="agent-card__msg" onSubmit={(e) => { e.preventDefault(); send(); }}>
        <input value={text} onChange={(e) => setText(e.target.value)} placeholder="Message this agent…" aria-label={`Message ${agentLabel(agent.kind)}`} disabled={!active} />
        <Button type="submit" variant="ghost" disabled={!active || busy || text.trim() === ''}><Icon name="send" size={14} /> Send</Button>
        {agent.status === 'paused' ? (
          <Button variant="ghost" disabled={busy} onClick={() => act(() => window.ratchet.resumeAgent(agent.id))}><Icon name="play" size={14} /> Resume</Button>
        ) : (
          <Button variant="ghost" disabled={busy || agent.status !== 'running'} onClick={() => act(() => window.ratchet.pauseAgent(agent.id))}><Icon name="pause" size={14} /> Pause</Button>
        )}
      </form>
      {sent && <div className="muted agent-card__sent" role="status">Sent: “{sent}”</div>}
      {error && <p role="alert" className="error-note">{error}</p>}
    </Card>
  );
}
