import { useState } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, Sparkline, StatusPill, agentLabel } from '@ratchet/ui';
import { ErrorNote, Empty, OptimizationPicker, PageHeader, useSelectedId } from '../components/common';
import { Icon } from '../components/icons';
import { errMsg, forOptimization, useQuery } from '../data';
import { fmtMetric, fmtUsd } from '../format';

export function Result() {
  const id = useSelectedId();
  const opts = { skip: id === null, relevant: forOptimization(id), intervalMs: 8000 };
  const detail = useQuery(() => window.ratchet.getOptimization(id!), [id], opts);
  const cands = useQuery(() => window.ratchet.candidatesWithResults(id!), [id], opts);
  const spend = useQuery(() => window.ratchet.spendBreakdown(id!), [id], opts);
  const [note, setNote] = useState<{ tone: 'ok' | 'error'; text: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  if (id === null) return <><PageHeader title="Result" /><Empty title="No optimization selected">Start one from Optimizations.</Empty></>;
  if ((detail.error ?? cands.error ?? spend.error) && !detail.data) return <ErrorNote error={detail.error ?? cands.error ?? spend.error} />;
  if (!detail.data || !cands.data || !spend.data) return <p className="muted">Loading…</p>;

  const d = detail.data;
  const s = d.summary;
  const wins = cands.data.filter((c) => c.movedRatchet);
  const sp = spend.data;
  const maxCat = Math.max(0.0001, ...sp.byCategory.map((c) => c.usd));

  async function run(key: string, fn: () => Promise<string>) {
    setBusy(key);
    setNote(null);
    try {
      setNote({ tone: 'ok', text: await fn() });
      detail.refresh();
    } catch (e) {
      setNote({ tone: 'error', text: errMsg(e) });
    } finally {
      setBusy(null);
    }
  }

  const finished = ['completed', 'stopped', 'failed', 'pr-open', 'merged'].includes(s.status);

  return (
    <>
      <PageHeader
        title="Result"
        sub={<span className="mono">{s.name} · {s.repo}</span>}
        actions={
          <>
            <OptimizationPicker />
            <StatusPill status={s.status} />
            <Button variant="ghost" disabled={busy !== null} onClick={() => run('export', async () => {
              const r = await window.ratchet.exportReport(id);
              return r.saved ? `Report saved to ${r.path}` : 'Export cancelled';
            })}>Export report</Button>
            <Button disabled={busy !== null || wins.length === 0} onClick={() => run('pr', async () => `Pull request opened: ${(await window.ratchet.openPullRequest(id)).url}`)}>
              <Icon name="external" size={14} /> {d.prUrl ? 'Reopen pull request' : 'Open pull request'}
            </Button>
          </>
        }
      />
      {note && <p role={note.tone === 'error' ? 'alert' : 'status'} className={note.tone === 'error' ? 'error-note' : 'ok-note'}>{note.text}</p>}
      {d.prUrl && !note && <p className="ok-note" role="status">Pull request: <span className="mono">{d.prUrl}</span></p>}

      <Card className="headline">
        <Eyebrow>Headline</Eyebrow>
        <div className="featured__hero">
          <span className="hero-num hero-num--xl">{fmtMetric(s.baseline, s.unit)}</span>
          <span className="arrow">→</span>
          <span className="hero-num hero-num--xl brass">{fmtMetric(s.best, s.unit)}</span>
          {s.gainPct !== null && <span className="mono gain headline__gain">{s.gainPct.toFixed(1)}% better</span>}
        </div>
        <p className="muted">{s.verifiedWins} verified win{s.verifiedWins === 1 ? '' : 's'} from {s.candidateCount} candidate{s.candidateCount === 1 ? '' : 's'} · {s.metric}</p>
      </Card>

      <section className="grid-two">
        <Card>
          <Eyebrow>Verified wins</Eyebrow>
          {wins.length === 0 ? (
            <p className="muted">No verified wins yet.</p>
          ) : (
            <ol className="wins">
              {wins.map((w) => (
                <li key={w.id}>
                  <AgentAvatar agent={w.agent} size={22} />
                  <div className="wins__body">
                    <div>{w.message || 'Candidate'}</div>
                    <div className="mono muted">{w.sha} · {fmtMetric(w.benchValue, s.unit)}{w.confirmValue !== null && ` · hidden trace ${fmtMetric(w.confirmValue, s.unit)}`}</div>
                  </div>
                  <span className="mono gain">{w.vsBaselinePct === null ? '' : `${w.vsBaselinePct.toFixed(1)}%`}</span>
                </li>
              ))}
            </ol>
          )}
        </Card>

        <Card>
          <div className="row-between"><Eyebrow>Spend</Eyebrow><span className="mono">{fmtUsd(sp.total)} <span className="muted">/ {fmtUsd(sp.budgetUsd)}</span></span></div>
          <ul className="bars" aria-label="Spend by category">
            {sp.byCategory.map((c) => (
              <li key={c.id}>
                <span>{c.label}</span>
                <span className="bars__track"><span className="bars__fill" style={{ width: `${(c.usd / maxCat) * 100}%` }} /></span>
                <span className="mono">{fmtUsd(c.usd)}</span>
              </li>
            ))}
          </ul>
          <div className="row-between spend-foot">
            <span className="muted">{sp.estimated > 0 ? `Includes ${fmtUsd(sp.estimated)} estimated` : 'Reconciled with billing'}</span>
            <Sparkline values={sp.series.map((p) => p.usd)} width={140} height={32} />
          </div>
          {sp.byAgent.length > 0 && (
            <ul className="agent-spend">
              {sp.byAgent.map((a) => (
                <li key={a.agentId ?? 'shared'}>
                  {a.kind ? <><AgentAvatar agent={a.kind} size={18} /> {agentLabel(a.kind)}</> : 'Shared'}
                  <span className="mono">{fmtUsd(a.usd)}</span>
                </li>
              ))}
            </ul>
          )}
        </Card>
      </section>

      <Card>
        <div className="row-between">
          <Eyebrow>Teardown</Eyebrow>
          <Button variant="ghost" disabled={busy !== null || !finished || d.teardown.every((t) => t.done)} onClick={() => run('teardown', async () => { await window.ratchet.teardown(id); return 'Sandbox torn down'; })}>
            Run teardown
          </Button>
        </div>
        <ul className="checklist" aria-label="Teardown checklist">
          {d.teardown.map((t) => (
            <li key={t.id} data-done={t.done}>
              <span className={`check${t.done ? ' check--on' : ''}`} aria-hidden="true">{t.done && <Icon name="check" size={12} />}</span>
              <span>{t.label}</span>
              <span className="sr-only">{t.done ? 'done' : 'pending'}</span>
            </li>
          ))}
        </ul>
        {!finished && <p className="muted">Available once the optimization has finished or been stopped.</p>}
      </Card>
    </>
  );
}
