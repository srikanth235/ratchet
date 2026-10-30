import { useState } from 'react';
import { Button, Card, Eyebrow } from '@ratchet/ui';
import { AgentCard } from '../components/AgentCard';
import { Crumbs, Empty, ErrorNote, OptTabs, OptimizationPicker, PageHeader, StateBadge, useSelectedId } from '../components/common';
import { Icon } from '../components/icons';
import { Pipeline } from '../components/Pipeline';
import { errMsg, forOptimization, useQuery } from '../data';
import { fmtDuration, fmtNum, gainLabel } from '../format';

const TICK: Record<string, string> = { verified: '#7BD8B0', blocked: '#F08A93', failed_confirm: '#8592A6', rejected: '#3A4558', pending: '#3A4558' };

/** Remaining-time ring: brass arc over a slate track. */
function Ring({ fraction }: { fraction: number }) {
  const r = 20, c = 2 * Math.PI * r;
  return (
    <svg width="52" height="52" viewBox="0 0 52 52" aria-hidden="true">
      <circle cx="26" cy="26" r={r} fill="none" stroke="rgba(148,163,190,.16)" strokeWidth="5" />
      <circle cx="26" cy="26" r={r} fill="none" stroke="#D9B26F" strokeWidth="5" strokeLinecap="round" strokeDasharray={`${c * Math.max(0, Math.min(1, fraction))} ${c}`} transform="rotate(-90 26 26)" />
    </svg>
  );
}

export function LiveAgents() {
  const id = useSelectedId();
  const opts = { skip: id === null, relevant: forOptimization(id), intervalMs: 5000 };
  const q = useQuery(() => window.ratchet.getOptimization(id!), [id], opts);
  const cands = useQuery(() => window.ratchet.candidatesWithResults(id!), [id], opts);
  const [confirmStop, setConfirmStop] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (id === null) return <><PageHeader title="Agents" /><Empty title="No optimization selected">Start one from Optimizations.</Empty></>;
  if (q.error && !q.data) return <ErrorNote error={q.error} />;
  if (!q.data) return <p className="muted">Loading…</p>;
  const d = q.data;
  const s = d.summary;
  const live = s.status === 'running' || s.status === 'baselining';
  const gain = gainLabel(s, 1);
  const remainingS = Math.max(0, d.timeLimitS - d.elapsedS);
  const spendPct = s.budgetUsd > 0 ? Math.min(100, (s.spendUsd / s.budgetUsd) * 100) : 0;
  const blocked = d.agents.reduce((n, a) => n + a.stats.blocked, 0);
  const recent = [...(cands.data ?? [])].sort((a, b) => a.submittedAt - b.submittedAt).slice(-23);
  const runningAgents = d.agents.filter((a) => a.status === 'running');
  const pausedAgents = d.agents.filter((a) => a.status === 'paused');
  const allPaused = runningAgents.length === 0 && pausedAgents.length > 0;

  async function stop() {
    setError(null);
    try {
      await window.ratchet.stopOptimization(id!, 'stopped from the Agents screen');
      setConfirmStop(false);
      q.refresh();
    } catch (e) {
      setError(errMsg(e));
    }
  }

  async function pauseAll() {
    setBusy(true);
    setError(null);
    try {
      await Promise.all((allPaused ? pausedAgents : runningAgents).map((a) => (allPaused ? window.ratchet.resumeAgent(a.id) : window.ratchet.pauseAgent(a.id))));
      q.refresh();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <PageHeader
        compact
        eyebrow={<Crumbs repo={s.repo} />}
        title={s.name}
        badge={<StateBadge detail={d} />}
        actions={
          <>
            <OptimizationPicker />
            {live && (
              <Button variant="ghost" disabled={busy || (runningAgents.length === 0 && pausedAgents.length === 0)} onClick={pauseAll}>
                <Icon name={allPaused ? 'play' : 'pause'} size={13} /> {allPaused ? 'Resume all' : 'Pause all'}
              </Button>
            )}
            {live && !confirmStop && <Button variant="danger" onClick={() => setConfirmStop(true)}><Icon name="stop" size={12} /> Stop &amp; revoke keys</Button>}
            {live && confirmStop && (
              <>
                <Button variant="danger" onClick={stop}>Confirm kill switch</Button>
                <Button variant="ghost" onClick={() => setConfirmStop(false)}>Cancel</Button>
              </>
            )}
          </>
        }
        tabs={<OptTabs current="agents" />}
      />
      <ErrorNote error={error} />

      <section className="stat-cards" aria-label="Run summary">
        <Card className="stat-card">
          <Eyebrow>Best verified {s.metric.split('.').at(-1)}</Eyebrow>
          <span className="stat-card__row">
            <span className="stat-card__num stat-card__num--best">{s.best === null ? '—' : fmtNum(s.best)}</span>
            <span className="stat-card__unit">{s.unit}</span>
            {gain && <span className="stat-card__aside mono gain">{gain.replace(/%$/, '')}%</span>}
          </span>
          <span className="stat-card__foot">baseline {s.baseline === null ? '—' : fmtNum(s.baseline)}{d.noiseFloor !== null ? ` · noise ±${fmtNum(d.noiseFloor)}` : ''}</span>
        </Card>
        <Card className="stat-card">
          <Eyebrow>Spend</Eyebrow>
          <span className="stat-card__row">
            <span className="stat-card__num">${s.spendUsd.toFixed(0)}</span>
            <span className="stat-card__unit">of ${s.budgetUsd.toFixed(0)}</span>
          </span>
          <span className="budget-track" aria-hidden="true"><span style={{ width: `${spendPct}%` }} /><span className="budget-marker" style={{ left: '80%' }} /></span>
          <span className="stat-card__foot">warning at 80%</span>
        </Card>
        <Card className="stat-card stat-card--clock">
          <Ring fraction={d.timeLimitS > 0 ? remainingS / d.timeLimitS : 0} />
          <span className="stat-card__clock-text">
            <Eyebrow>Time remaining</Eyebrow>
            <span className="serif">{live ? fmtDuration(remainingS) : '—'}</span>
          </span>
        </Card>
        <Card className="stat-card">
          <Eyebrow>Evals</Eyebrow>
          <span className="stat-card__row">
            <span className="stat-card__num">{s.candidateCount}</span>
            <span className="stat-card__unit">{s.verifiedWins} verified · {blocked} blocked</span>
          </span>
          <span className="tick-row" aria-hidden="true">{recent.map((c) => <span key={c.id} style={{ background: TICK[c.verdict] ?? '#3A4558' }} />)}</span>
        </Card>
      </section>

      {d.agents.length === 0 ? (
        <Empty title="No agents">This optimization has no agents yet.</Empty>
      ) : (
        <section className="agents-grid" aria-label="Agents">
          {d.agents.map((a) => <AgentCard key={a.id} agent={a} unit={s.unit} metric={s.metric.split('.').at(-1) ?? s.metric} best={s.best} onChanged={q.refresh} />)}
        </section>
      )}
      <Pipeline pipeline={d.pipeline} />
    </>
  );
}
