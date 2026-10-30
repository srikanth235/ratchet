import { useMemo, useState } from 'react';
import { Card, Eyebrow, Histogram, StairChart, VerdictPill, agentColor, agentLabel, agentSoft, type Verdict } from '@ratchet/ui';
import { Crumbs, Empty, ErrorNote, OptTabs, OptimizationPicker, PageHeader, StateBadge, useSelectedId } from '../components/common';
import { forOptimization, useQuery } from '../data';
import { fmtNum } from '../format';

const FILTERS: (Verdict | 'all')[] = ['all', 'verified', 'rejected', 'failed_confirm', 'blocked', 'pending'];

export function Benchmarks() {
  const id = useSelectedId();
  const opts = { skip: id === null, relevant: forOptimization(id), intervalMs: 8000 };
  const series = useQuery(() => window.ratchet.ratchetHistory(id!), [id], opts);
  const cands = useQuery(() => window.ratchet.candidatesWithResults(id!), [id], opts);
  const detail = useQuery(() => window.ratchet.getOptimization(id!), [id], opts);
  const [filter, setFilter] = useState<Verdict | 'all'>('all');
  const [showRejected, setShowRejected] = useState(true);

  const rows = useMemo(() => (cands.data ?? []).filter((c) => filter === 'all' || c.verdict === filter), [cands.data, filter]);

  if (id === null) return <><PageHeader title="Benchmarks" /><Empty title="No optimization selected">Start one from Optimizations.</Empty></>;
  const err = series.error ?? cands.error ?? detail.error;
  if (err && !series.data) return <ErrorNote error={err} />;
  if (!series.data || !cands.data || !detail.data) return <p className="muted">Loading…</p>;

  const sr = series.data;
  const d = detail.data;
  const s = d.summary;
  const unit = sr.unit;
  const best = [...cands.data].reverse().find((c) => c.movedRatchet) ?? null;
  const pcts = ['p50', 'p95', 'p99'].filter((k) => k in d.baselineStats);
  const delta = (a: number | undefined, b: number | undefined) => (a && b !== undefined ? ((b - a) / a) * 100 : null);
  const locks = Math.max(0, sr.steps.length - 1);
  const saved = s.baseline !== null && s.best !== null ? s.baseline - s.best : null;
  const kinds = [...new Set(sr.points.map((p) => p.agent))];
  const points = showRejected ? sr.points : sr.points.filter((p) => p.verified);
  const markers = [
    d.baselineStats['p95'] !== undefined ? { value: d.baselineStats['p95'], label: `p95 ${fmtNum(d.baselineStats['p95'])}`, dashed: true } : null,
    best?.stats['p95'] !== undefined ? { value: best.stats['p95'], label: `p95 ${fmtNum(best.stats['p95'])}` } : null,
  ].filter((m): m is { value: number; label: string; dashed?: boolean } => m !== null);
  const cols = { a: 'var(--rt-color-slate)', b: 'var(--rt-color-brass)' };
  const widths = (a: number | undefined, b: number | undefined) => {
    const hi = Math.max(a ?? 0, b ?? 0) || 1;
    return [`${((a ?? 0) / hi) * 100}%`, `${((b ?? 0) / hi) * 100}%`] as const;
  };

  return (
    <>
      <PageHeader
        compact
        eyebrow={<Crumbs repo={s.repo} />}
        title={s.name}
        badge={<StateBadge detail={d} />}
        actions={<OptimizationPicker />}
        tabs={<OptTabs current="benchmarks" />}
      />

      <Card className="ratchet-card" data-testid="ratchet-card">
        <div className="ratchet-card__head">
          <div className="ratchet-card__hero">
            <Eyebrow>Best verified · {sr.direction === 'lower' ? 'lower' : 'higher'} is better</Eyebrow>
            <span className="ratchet-card__num">
              <span className="hero-num hero-num--xl">{s.best === null ? '—' : `${fmtNum(s.best)}${unit ? ` ${unit}` : ''}`}</span>
              <span className="mono gain">{saved !== null ? `${saved >= 0 ? '−' : '+'}${fmtNum(Math.abs(saved))}${unit ? ` ${unit}` : ''} · ` : ''}{locks} lock{locks === 1 ? '' : 's'}</span>
            </span>
          </div>
          <div className="ratchet-card__tools">
            <label className="check-label">
              <input type="checkbox" checked={showRejected} onChange={(e) => setShowRejected(e.target.checked)} />
              Show rejected
            </label>
          </div>
        </div>
        <div className="chart-legend" style={{ margin: 0, padding: 0 }}>
          <span><i className="legend-line" aria-hidden="true" />Best verified</span>
          {sr.noiseFloor !== null && <span><i style={{ display: 'inline-block', width: 16, height: 8, borderRadius: 2, background: 'rgba(217,178,111,.22)' }} aria-hidden="true" />Noise band ±{fmtNum(sr.noiseFloor)}{unit ? ` ${unit}` : ''}</span>}
          {kinds.map((k) => <span key={k}><i className="legend-dot" style={{ background: agentColor(k) }} aria-hidden="true" />{agentLabel(k)}</span>)}
        </div>
        {sr.steps.length > 0 ? (
          <StairChart steps={sr.steps} points={points} direction={sr.direction} unit={unit} tMax={sr.tMax} width={1000} height={310} baselineLabel
            limit={d.timeLimitS / 60}
            noiseBand={sr.baseline !== null && sr.noiseFloor !== null ? { center: sr.baseline, halfWidth: sr.noiseFloor } : undefined} />
        ) : (
          <Empty title="No baseline yet">The chart appears once the baseline run finishes.</Empty>
        )}
      </Card>

      <section className="split">
        <Card className="card-pad" style={{ flex: '3 1 460px', minWidth: 0 }}>
          <div className="card-head">
            <h2 className="card-title">Latency distribution</h2>
            <span className="card-head__note">{d.baselineSamples.length} baseline samples</span>
            <span className="chart-legend" style={{ margin: '0 0 0 auto', padding: 0 }}>
              <span><i style={{ display: 'inline-block', width: 12, height: 10, border: '1.4px solid #8592A6', borderRadius: 2 }} aria-hidden="true" />Baseline</span>
              <span><i style={{ display: 'inline-block', width: 12, height: 10, borderRadius: 2, background: 'rgba(217,178,111,.55)' }} aria-hidden="true" />Best</span>
            </span>
          </div>
          {d.baselineSamples.length > 0 && best && best.samples.length > 0 ? (
            <div style={{ overflowX: 'auto' }}>
              <Histogram unit={unit} markers={markers} series={[{ name: 'baseline', values: d.baselineSamples }, { name: 'best', values: best.samples }]} />
            </div>
          ) : (
            <p className="muted">Needs a baseline and at least one verified win.</p>
          )}
        </Card>
        <Card className="card-pad" style={{ flex: '2 1 340px', minWidth: 0, gap: 6 }}>
          <h2 className="card-title" style={{ marginBottom: 6 }}>Baseline vs best</h2>
          {pcts.length > 0 && best ? (
            pcts.map((k) => {
              const a = d.baselineStats[k];
              const b = best.stats[k];
              const dl = delta(a, b);
              const [wa, wb] = widths(a, b);
              return (
                <div key={k} className="pct-row" data-testid="pct-row">
                  <div className="pct-row__top">
                    <span className="pct-row__k">{k}</span>
                    <span className="mono faint">{a === undefined ? '—' : `${fmtNum(a)}${unit ? ` ${unit}` : ''}`}</span>
                    <span className="pct-row__arrow">→</span>
                    <span className="mono">{b === undefined ? '—' : `${fmtNum(b)}${unit ? ` ${unit}` : ''}`}</span>
                    <span className={`mono delta-badge${dl !== null && dl > 0 ? ' delta-badge--rose' : ''}`}>{dl === null ? '—' : `${dl < 0 ? '−' : '+'}${Math.abs(dl).toFixed(0)}%`}</span>
                  </div>
                  <div className="pct-row__bars"><span style={{ width: wa, background: cols.a }} /><span style={{ width: wb, background: cols.b }} /></div>
                </div>
              );
            })
          ) : (
            <p className="muted">No percentile data yet.</p>
          )}
        </Card>
      </section>

      <Card className="table-card">
        <div className="table-head">
          <h2 className="card-title">Candidates</h2>
          <span className="card-head__note">{rows.length} shown · every commit an agent submitted, re-benched by the trusted runner</span>
          <div className="chips" role="group" aria-label="Filter by verdict">
            {FILTERS.map((f) => (
              <button key={f} type="button" aria-pressed={filter === f} className={`chip${filter === f ? ' chip--on' : ''}`} onClick={() => setFilter(f)}>{f.replace('_', ' ')}</button>
            ))}
          </div>
        </div>
        <div className="table-scroll">
          <table className="table table--sm">
            <thead>
              <tr><th>Commit</th><th>Change</th><th>Guardrails</th><th className="num">Bench</th><th className="num">Confirm</th><th className="num">vs baseline</th><th>Verdict</th></tr>
            </thead>
            <tbody>
              {rows.map((c) => {
                const soft = agentSoft(c.agent);
                const bad = c.verdict === 'blocked';
                return (
                  <tr key={c.id} data-verdict={c.verdict} title={c.reasons.join('\n')}>
                    <td>
                      <span style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                        <span className="lane-chip" style={{ background: soft.bg, color: soft.ink }} role="img" aria-label={agentLabel(c.agent)}>{c.agent === 'codex' ? 'CX' : c.agent === 'claude-code' ? 'CC' : 'MK'}</span>
                        <span className="mono sha">{c.sha}</span>
                      </span>
                    </td>
                    <td style={{ color: '#C3CBD8' }}>{c.message}{!bad && c.reasons[0] && <div className="table__sub">{c.reasons[0]}</div>}</td>
                    <td>
                      <span className="gdots">
                        <span style={{ background: bad ? 'var(--rt-color-rose)' : 'var(--rt-color-mint)' }} />
                        <span className="gdots__txt" style={{ color: bad ? 'var(--rt-color-rose)' : 'var(--rt-color-text-secondary)' }}>{bad ? (c.reasons[0] ?? 'blocked').slice(0, 40) : 'passed'}</span>
                      </span>
                    </td>
                    <td className="mono num">{c.benchValue === null ? '—' : `${fmtNum(c.benchValue)}${unit ? ` ${unit}` : ''}`}</td>
                    <td className="mono num muted">{c.confirmValue === null ? '—' : fmtNum(c.confirmValue)}</td>
                    <td className="mono num" style={{ color: c.vsBaselinePct !== null && c.vsBaselinePct > 0 ? 'var(--rt-color-mint)' : 'var(--rt-color-text-tertiary)' }}>
                      {c.vsBaselinePct === null ? '—' : `${c.vsBaselinePct >= 0 ? '+' : '−'}${Math.abs(c.vsBaselinePct).toFixed(1)}%`}
                    </td>
                    <td><VerdictPill verdict={c.verdict} /></td>
                  </tr>
                );
              })}
              {rows.length === 0 && <tr><td colSpan={7} className="muted">No candidates match.</td></tr>}
            </tbody>
          </table>
        </div>
      </Card>
    </>
  );
}
