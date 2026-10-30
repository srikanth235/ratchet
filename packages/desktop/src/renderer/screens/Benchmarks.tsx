import { useMemo, useState } from 'react';
import { AgentAvatar, Card, Eyebrow, Histogram, StairChart, VerdictPill, type Verdict } from '@ratchet/ui';
import { ErrorNote, Empty, OptimizationPicker, PageHeader, useSelectedId } from '../components/common';
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

  const rows = useMemo(() => (cands.data ?? []).filter((c) => filter === 'all' || c.verdict === filter), [cands.data, filter]);

  if (id === null) return <><PageHeader title="Benchmark history" /><Empty title="No optimization selected">Start one from Optimizations.</Empty></>;
  const err = series.error ?? cands.error ?? detail.error;
  if (err && !series.data) return <ErrorNote error={err} />;
  if (!series.data || !cands.data || !detail.data) return <p className="muted">Loading…</p>;

  const sr = series.data;
  const d = detail.data;
  const unit = sr.unit;
  const best = [...cands.data].reverse().find((c) => c.movedRatchet) ?? null;
  const pcts = ['p50', 'p95', 'p99'].filter((k) => k in d.baselineStats);
  const delta = (a: number | undefined, b: number | undefined) => (a && b !== undefined ? ((b - a) / a) * 100 : null);

  return (
    <>
      <PageHeader
        title="Benchmark history"
        sub={<>Every candidate, judged against the noise floor{sr.noiseFloor !== null && <> (±<span className="mono">{fmtNum(sr.noiseFloor)}{unit && ` ${unit}`}</span>)</>}.</>}
        actions={<OptimizationPicker />}
      />

      <Card>
        <div className="row-between">
          <Eyebrow>Ratchet</Eyebrow>
          <span className="legend">
            <span><i className="swatch swatch--solid" /> verified</span>
            <span><i className="swatch swatch--hollow" /> did not move the ratchet</span>
            <span><i className="swatch swatch--band" /> baseline noise</span>
          </span>
        </div>
        {sr.steps.length > 0 ? (
          <StairChart steps={sr.steps} points={sr.points} direction={sr.direction} unit={unit} tMax={sr.tMax} height={300}
            noiseBand={sr.baseline !== null && sr.noiseFloor !== null ? { center: sr.baseline, halfWidth: sr.noiseFloor } : undefined} />
        ) : (
          <Empty title="No baseline yet">The chart appears once the baseline run finishes.</Empty>
        )}
      </Card>

      <section className="grid-two">
        <Card>
          <Eyebrow>Distribution · baseline vs best</Eyebrow>
          {d.baselineSamples.length > 0 && best && best.samples.length > 0 ? (
            <Histogram unit={unit} series={[{ name: 'baseline', values: d.baselineSamples }, { name: 'best', values: best.samples }]} />
          ) : (
            <p className="muted">Needs a baseline and at least one verified win.</p>
          )}
        </Card>
        <Card>
          <Eyebrow>Percentiles</Eyebrow>
          {pcts.length > 0 && best ? (
            <table className="table table--compact">
              <thead><tr><th></th><th>Baseline</th><th>Best</th><th>Δ</th></tr></thead>
              <tbody>
                {pcts.map((k) => {
                  const a = d.baselineStats[k];
                  const b = best.stats[k];
                  const dl = delta(a, b);
                  return (
                    <tr key={k}>
                      <td className="mono">{k}</td>
                      <td className="mono">{a === undefined ? '—' : fmtNum(a)}</td>
                      <td className="mono">{b === undefined ? '—' : fmtNum(b)}</td>
                      <td className="mono gain">{dl === null ? '—' : `${dl.toFixed(1)}%`}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          ) : (
            <p className="muted">No percentile data yet.</p>
          )}
        </Card>
      </section>

      <Card className="table-card">
        <div className="row-between">
          <Eyebrow>Candidates · {rows.length}</Eyebrow>
          <div className="chips" role="group" aria-label="Filter by verdict">
            {FILTERS.map((f) => (
              <button key={f} type="button" aria-pressed={filter === f} className={`chip${filter === f ? ' chip--on' : ''}`} onClick={() => setFilter(f)}>{f.replace('_', ' ')}</button>
            ))}
          </div>
        </div>
        <table className="table">
          <thead><tr><th>Sha</th><th>Agent</th><th>When</th><th>Bench</th><th>vs baseline</th><th>Confirm</th><th>Verdict</th><th>Why</th></tr></thead>
          <tbody>
            {rows.map((c) => (
              <tr key={c.id} data-verdict={c.verdict}>
                <td className="mono">{c.sha}</td>
                <td><AgentAvatar agent={c.agent} size={22} /></td>
                <td className="mono muted">{c.t.toFixed(0)}m</td>
                <td className="mono">{c.benchValue === null ? '—' : `${fmtNum(c.benchValue)}${unit && ` ${unit}`}`}</td>
                <td className="mono">{c.vsBaselinePct === null ? '—' : `${c.vsBaselinePct >= 0 ? '+' : ''}${c.vsBaselinePct.toFixed(1)}%`}</td>
                <td className="mono">{c.confirmValue === null ? '—' : fmtNum(c.confirmValue)}</td>
                <td><VerdictPill verdict={c.verdict} /></td>
                <td className="why" title={c.reasons.join('\n')}>{c.message}{c.reasons[0] && <div className="table__sub">{c.reasons[0]}</div>}</td>
              </tr>
            ))}
            {rows.length === 0 && <tr><td colSpan={8} className="muted">No candidates match.</td></tr>}
          </tbody>
        </table>
      </Card>
    </>
  );
}
