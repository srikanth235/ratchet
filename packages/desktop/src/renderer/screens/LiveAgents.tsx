import { useState } from 'react';
import { Button, StatusPill } from '@ratchet/ui';
import { AgentCard } from '../components/AgentCard';
import { ErrorNote, Empty, OptimizationPicker, PageHeader, useSelectedId } from '../components/common';
import { Icon } from '../components/icons';
import { Pipeline } from '../components/Pipeline';
import { errMsg, forOptimization, useQuery } from '../data';
import { fmtDuration, fmtMetric } from '../format';

export function LiveAgents() {
  const id = useSelectedId();
  const q = useQuery(() => window.ratchet.getOptimization(id!), [id], { skip: id === null, relevant: forOptimization(id), intervalMs: 5000 });
  const [confirmStop, setConfirmStop] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (id === null) return <><PageHeader title="Live agents" /><Empty title="No optimization selected">Start one from Optimizations.</Empty></>;
  if (q.error && !q.data) return <ErrorNote error={q.error} />;
  if (!q.data) return <p className="muted">Loading…</p>;
  const d = q.data;
  const s = d.summary;
  const live = s.status === 'running' || s.status === 'baselining';

  async function stop() {
    setError(null);
    try {
      await window.ratchet.stopOptimization(id!, 'stopped from the Live agents screen');
      setConfirmStop(false);
      q.refresh();
    } catch (e) {
      setError(errMsg(e));
    }
  }

  return (
    <>
      <PageHeader
        title="Live agents"
        sub={<><span className="mono">{s.name}</span> · {fmtMetric(s.baseline, s.unit)} → <span className="brass mono">{fmtMetric(s.best, s.unit)}</span> · {fmtDuration(d.elapsedS)} · ${s.spendUsd.toFixed(2)} of ${s.budgetUsd.toFixed(0)}</>}
        actions={
          <>
            <OptimizationPicker />
            <StatusPill status={s.status} />
            {live && !confirmStop && <Button variant="danger" onClick={() => setConfirmStop(true)}><Icon name="stop" size={14} /> Stop</Button>}
            {live && confirmStop && (
              <>
                <Button variant="danger" onClick={stop}>Confirm kill switch</Button>
                <Button variant="ghost" onClick={() => setConfirmStop(false)}>Cancel</Button>
              </>
            )}
          </>
        }
      />
      <ErrorNote error={error} />

      {d.agents.length === 0 ? (
        <Empty title="No agents">This optimization has no agents yet.</Empty>
      ) : (
        <section className="agents-grid" aria-label="Agents">
          {d.agents.map((a) => <AgentCard key={a.id} agent={a} onChanged={q.refresh} />)}
        </section>
      )}
      <Pipeline pipeline={d.pipeline} />
    </>
  );
}
