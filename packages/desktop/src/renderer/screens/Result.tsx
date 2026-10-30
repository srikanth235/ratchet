import { useState } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, agentColor, agentLabel } from '@ratchet/ui';
import { Crumbs, Empty, ErrorNote, OptimizationPicker, PageHeader, StateBadge, useSelectedId } from '../components/common';
import { Icon } from '../components/icons';
import { errMsg, forOptimization, useQuery } from '../data';
import { fmtDuration, fmtNum, fmtUsd, gainLabel } from '../format';
import { fmtRoi, fmtUsdCompact, valueOf } from '../value';

const SPEND_COLORS = ['#EBCB8E', '#D9B26F', '#8C7448', '#4A5568', '#3A4558'];

/** The whole ratchet in one stripe: best-so-far as a brass staircase across the run. */
function Staircase({ steps, tMax }: { steps: { t: number; value: number }[]; tMax: number }) {
  if (steps.length === 0) return null;
  const vs = steps.map((s) => s.value);
  const lo = Math.min(...vs), hi = Math.max(...vs), span = hi - lo || 1;
  const x = (t: number) => (t / (tMax || 1)) * 1000;
  const y = (v: number) => 8 + ((v - lo) / span) * 76;
  let d = `M0,${y(steps[0]!.value).toFixed(1)}`;
  for (const s of steps.slice(1)) d += ` H${x(s.t).toFixed(1)} V${y(s.value).toFixed(1)}`;
  d += ' H1000';
  return (
    <svg viewBox="0 0 1000 100" preserveAspectRatio="none" style={{ width: '100%', height: 90, display: 'block' }} role="img" aria-label={`${steps.length - 1} ratchet steps from ${fmtNum(steps[0]!.value)} to ${fmtNum(steps.at(-1)!.value)}`}>
      <defs><linearGradient id="res-fill" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#D9B26F" stopOpacity=".28" /><stop offset="1" stopColor="#D9B26F" stopOpacity="0" /></linearGradient></defs>
      <path d={`${d} V100 H0 Z`} fill="url(#res-fill)" />
      <path d={d} fill="none" stroke="#D9B26F" strokeWidth="2.4" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export function Result() {
  const id = useSelectedId();
  const opts = { skip: id === null, relevant: forOptimization(id), intervalMs: 8000 };
  const detail = useQuery(() => window.ratchet.getOptimization(id!), [id], opts);
  const cands = useQuery(() => window.ratchet.candidatesWithResults(id!), [id], opts);
  const spend = useQuery(() => window.ratchet.spendBreakdown(id!), [id], opts);
  const series = useQuery(() => window.ratchet.ratchetHistory(id!), [id], opts);
  const [note, setNote] = useState<{ tone: 'ok' | 'error'; text: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  if (id === null) return <><PageHeader title="Result" /><Empty title="No optimization selected">Start one from Optimizations.</Empty></>;
  if ((detail.error ?? cands.error ?? spend.error) && !detail.data) return <ErrorNote error={detail.error ?? cands.error ?? spend.error} />;
  if (!detail.data || !cands.data || !spend.data) return <p className="muted">Loading…</p>;

  const d = detail.data;
  const s = d.summary;
  const wins = cands.data.filter((c) => c.movedRatchet);
  const blocked = cands.data.filter((c) => c.verdict === 'blocked').length;
  const sp = spend.data;
  const total = sp.byCategory.reduce((n, c) => n + c.usd, 0) || 1;
  const gain = gainLabel(s, 1);
  const unit = s.unit;
  const confirmed = wins.some((w) => w.confirmValue !== null);
  const bestWin = wins.at(-1) ?? null;
  const metricName = s.metric.split('.').at(-1);
  const pcts = ['p50', 'p95', 'p99'].filter((k) => k in d.baselineStats && k !== metricName);
  const finished = ['completed', 'stopped', 'failed', 'pr-open', 'merged'].includes(s.status);
  const canPr = finished && wins.length > 0;
  const agentsInvolved = new Set(wins.map((w) => w.agent)).size || s.agents.length;
  const steps = series.data?.steps ?? [];
  const value = valueOf(d.configYaml, s.direction, s.baseline, s.best, sp.total);

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
            <Button variant="ghost" disabled={busy !== null} onClick={() => run('export', async () => {
              const r = await window.ratchet.exportReport(id);
              return r.saved ? `Report saved to ${r.path}` : 'Export cancelled';
            })}>Export report</Button>
            <Button disabled={busy !== null || !canPr} title={canPr ? undefined : 'Available once the run has stopped with at least one verified win'}
              onClick={() => run('pr', async () => `Pull request opened: ${(await window.ratchet.openPullRequest(id)).url}`)}>
              <Icon name="branch" size={13} /> {d.prUrl ? 'Reopen pull request' : 'Open pull request'}
            </Button>
          </>
        }
      />
      {note && <p role={note.tone === 'error' ? 'alert' : 'status'} className={note.tone === 'error' ? 'error-note' : 'ok-note'}>{note.text}</p>}
      {d.prUrl && !note && <p className="ok-note" role="status">Pull request: <span className="mono">{d.prUrl}</span></p>}

      <Card className="headline" data-testid="headline">
        <div className="headline__top">
          <div className="headline__main">
            <Eyebrow>Verified improvement{confirmed ? ' · confirmed on hidden trace' : ''}</Eyebrow>
            <div className="headline__nums">
              <span className="hero-num--base">{s.baseline === null ? '—' : fmtNum(s.baseline)}</span>
              <span className="hero-to">to</span>
              <span className="hero-num--final">{s.best === null ? '—' : fmtNum(s.best)}</span>
              <span className="headline__unit">{unit} {s.metric.split('.').at(-1)}</span>
            </div>
            <span className="headline__sub">
              {gain && <span className="mono gain">{gain}</span>}{gain ? ' · ' : ''}
              {s.verifiedWins} verified win{s.verifiedWins === 1 ? '' : 's'} from {agentsInvolved} agent{agentsInvolved === 1 ? '' : 's'} over {fmtDuration(d.elapsedS)}
            </span>
            {value && (
              <span className="headline__value" data-testid="value">
                Worth <span className="mono gain">{fmtUsdCompact(value.usdPerYear)}</span> a year ({value.basis})
                {value.returnOnSpend !== null && <> · <span className="mono">{fmtRoi(value.returnOnSpend)}</span> return on spend</>}
              </span>
            )}
          </div>
          <div className="headline__tiles">
            <div className="stat-tile"><Eyebrow>Spend</Eyebrow><div className="serif">{fmtUsd(sp.total)} <small>/ {sp.budgetUsd.toFixed(0)}</small></div></div>
            <div className="stat-tile"><Eyebrow>Per verified win</Eyebrow><div className="serif">{wins.length ? fmtUsd(sp.total / wins.length) : '—'}</div></div>
            <div className="stat-tile"><Eyebrow>Evals</Eyebrow><div className="serif">{s.candidateCount} <small>· {wins.length} wins</small></div></div>
            <div className="stat-tile"><Eyebrow>Blocked</Eyebrow><div className="serif" style={{ color: 'var(--rt-color-rose-light)' }}>{blocked}</div></div>
          </div>
        </div>
        <Staircase steps={steps} tMax={series.data?.tMax ?? 1} />
        {steps.length > 0 && (
          <div className="mono headline__axis" aria-hidden="true">
            {steps.map((st, i) => <span key={i}>{i === 0 ? '0h · ' : ''}{fmtNum(st.value)}{i === steps.length - 1 ? ` · ${(Math.max(series.data?.tMax ?? 0, 1) / 60).toFixed(1)}h` : ''}</span>)}
          </div>
        )}
      </Card>

      <section className="result-split">
        <Card style={{ flex: '3 1 460px', minWidth: 0, padding: 0, overflow: 'hidden' }}>
          <div className="card-head" style={{ padding: '18px 20px 14px' }}>
            <h2 className="card-title">Verified wins</h2>
            <span className="card-head__note">{wins.length} commit{wins.length === 1 ? '' : 's'} moved the ratchet</span>
            {bestWin && <span className="mono faint card-head__aside" style={{ fontSize: 12 }}>best {bestWin.sha}</span>}
          </div>
          {wins.length === 0 ? (
            <p className="muted" style={{ padding: '0 20px 20px' }}>No verified wins yet.</p>
          ) : (
            <ol style={{ listStyle: 'none', margin: 0, padding: 0 }} aria-label="Verified wins">
              {wins.map((w) => (
                <li key={w.id} className="win-row">
                  <AgentAvatar agent={w.agent} size={24} />
                  <div className="win-row__body">
                    <span title={w.message}>{w.message || 'Candidate'}</span>
                    <span className="mono win-row__meta">{w.sha} · {w.benchValue === null ? '—' : `${fmtNum(w.benchValue)}${unit ? ` ${unit}` : ''}`}{w.confirmValue !== null && ` · hidden trace ${fmtNum(w.confirmValue)}${unit ? ` ${unit}` : ''}`}</span>
                  </div>
                  <span className="mono delta-badge">{w.vsBaselinePct === null ? '' : `${w.vsBaselinePct >= 0 ? '−' : '+'}${Math.abs(w.vsBaselinePct).toFixed(1)}%`}</span>
                </li>
              ))}
            </ol>
          )}
        </Card>
        <Card className="card-pad" style={{ flex: '2 1 320px', minWidth: 0, gap: 4 }}>
          <h2 className="card-title" style={{ marginBottom: 8 }}>Before and after</h2>
          <div className="ba-row">
            <span className="ba-row__k">{s.metric.split('.').at(-1)}</span>
            <span className="mono faint">{s.baseline === null ? '—' : `${fmtNum(s.baseline)} ${unit}`.trim()}</span>
            <span className="pct-row__arrow">→</span>
            <span className="mono">{s.best === null ? '—' : `${fmtNum(s.best)} ${unit}`.trim()}</span>
            {gain && <span className="mono delta-badge">{gain}</span>}
          </div>
          {pcts.map((k) => {
            const a = d.baselineStats[k];
            const b = bestWin?.stats[k];
            const dl = a && b !== undefined ? ((b - a) / a) * 100 : null;
            return (
              <div key={k} className="ba-row">
                <span className="ba-row__k">{k}</span>
                <span className="mono faint">{a === undefined ? '—' : `${fmtNum(a)} ${unit}`.trim()}</span>
                <span className="pct-row__arrow">→</span>
                <span className="mono">{b === undefined ? '—' : `${fmtNum(b)} ${unit}`.trim()}</span>
                <span className="mono delta-badge">{dl === null ? '—' : `${dl < 0 ? '−' : '+'}${Math.abs(dl).toFixed(0)}%`}</span>
              </div>
            );
          })}
        </Card>
      </section>

      <section className="result-split">
        <Card className="side-card side-card--gap" style={{ flex: '1 1 320px', minWidth: 0, padding: '18px 20px' }}>
          <Eyebrow>By agent</Eyebrow>
          {sp.byAgent.length === 0 && <span className="muted">No spend recorded.</span>}
          {sp.byAgent.map((a) => {
            const n = a.kind ? wins.filter((w) => w.agent === a.kind).length : 0;
            const pct = sp.total > 0 ? (a.usd / sp.total) * 100 : 0;
            return (
              <div key={a.agentId ?? 'shared'} className="agent-row">
                {a.kind ? <AgentAvatar agent={a.kind} size={30} /> : <span className="rt-avatar" style={{ width: 30, height: 30, background: '#151D2B', color: '#A7B1C2' }}>··</span>}
                <span className="agent-row__main">
                  <span className="agent-row__top"><span>{a.kind ? `${agentLabel(a.kind)} · ${n} win${n === 1 ? '' : 's'}` : 'Shared infrastructure'}</span><span className="mono">{fmtUsd(a.usd)}</span></span>
                  <span className="track track--lg" style={{ background: 'rgba(148,163,190,.12)' }}><span style={{ width: `${pct}%`, background: a.kind ? agentColor(a.kind) : '#4A5568' }} /></span>
                  <span className="mono agent-row__sub">{n > 0 ? `${fmtUsd(a.usd / n)} per win` : 'no wins'}</span>
                </span>
              </div>
            );
          })}
        </Card>
        <Card className="side-card side-card--gap" style={{ flex: '1 1 320px', minWidth: 0, padding: '18px 20px' }}>
          <Eyebrow>Spend breakdown</Eyebrow>
          <span className="stacked-bar" aria-hidden="true">
            {sp.byCategory.map((c, i) => <span key={c.id} style={{ width: `${(c.usd / total) * 100}%`, background: SPEND_COLORS[i % SPEND_COLORS.length] }} />)}
          </span>
          <div className="legend-list" role="list" aria-label="Spend by category">
            {sp.byCategory.map((c, i) => (
              <span key={c.id} role="listitem"><i style={{ background: SPEND_COLORS[i % SPEND_COLORS.length] }} />{c.label}<span className="mono">{fmtUsd(c.usd)}</span></span>
            ))}
          </div>
          <span className="card-note">{sp.estimated > 0 ? `Includes ${fmtUsd(sp.estimated)} estimated` : 'Reconciled with billing'}</span>
        </Card>
        <Card className="side-card side-card--gap" style={{ flex: '1 1 320px', minWidth: 0, padding: '18px 20px' }}>
          <Eyebrow>Teardown</Eyebrow>
          <ul className="checklist" aria-label="Teardown checklist">
            {d.teardown.map((t) => (
              <li key={t.id} data-done={t.done}>
                <span className="check-icon"><Icon name={t.done ? 'check' : 'clock'} size={15} /></span>
                <span>{t.label}</span>
                <span className="sr-only">{t.done ? 'done' : 'pending'}</span>
              </li>
            ))}
          </ul>
          {!finished && <span className="card-note">Available once the optimization has finished or been stopped.</span>}
          <Button variant="danger-outline" className="teardown-btn" disabled={busy !== null || !finished || d.teardown.every((t) => t.done)}
            onClick={() => run('teardown', async () => { await window.ratchet.teardown(id); return 'Sandbox torn down'; })}>
            Destroy sandbox…
          </Button>
        </Card>
      </section>
    </>
  );
}
