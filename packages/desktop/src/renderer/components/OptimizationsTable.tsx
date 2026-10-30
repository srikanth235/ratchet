import { useState } from 'react';
import { AgentAvatar, Card, StatusPill, type OptimizationStatus } from '@ratchet/ui';
import type { Optimization, RatchetSeries } from '../../shared/types';
import { forOptimization, useQuery } from '../data';
import { fmtMetric, gainLabel, metricGlyph } from '../format';

type Filter = 'all' | 'running' | 'shipped' | 'drafts';
const GROUPS: Record<Exclude<Filter, 'all'>, OptimizationStatus[]> = {
  running: ['running', 'baselining', 'stopping'],
  shipped: ['completed', 'pr-open', 'merged'],
  drafts: ['draft'],
};
const FILTERS: { id: Filter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'shipped', label: 'Shipped' },
  { id: 'drafts', label: 'Drafts' },
];

/** Stepped path in a 96×28 box: improvements descend for lower-is-better metrics and ascend for higher-is-better ones. */
export function trendPath(steps: { t: number; value: number }[], tMax: number): string {
  const W = 96, H = 28, pad = 4;
  if (steps.length < 2) return `M0,${H / 2} H${W}`;
  const vs = steps.map((s) => s.value);
  const lo = Math.min(...vs), hi = Math.max(...vs), span = hi - lo || 1;
  const y = (v: number) => (H - pad - ((v - lo) / span) * (H - pad * 2)).toFixed(1);
  const x = (t: number) => ((t / (tMax || 1)) * W).toFixed(1);
  let d = `M0,${y(steps[0]!.value)}`;
  for (const s of steps.slice(1)) d += ` H${x(s.t)} V${y(s.value)}`;
  return `${d} H${W}`;
}

function Trend({ o }: { o: Optimization }) {
  const q = useQuery<RatchetSeries>(() => window.ratchet.ratchetHistory(o.id), [o.id], { skip: o.status === 'draft', relevant: forOptimization(o.id), intervalMs: 30_000 });
  const won = (q.data?.steps.length ?? 0) > 1;
  const color = o.status === 'draft' || !won ? '#5C6779' : o.status === 'running' || o.status === 'baselining' ? '#D9B26F' : '#7BD8B0';
  return (
    <svg width="96" height="28" viewBox="0 0 96 28" aria-hidden="true">
      <path d={q.data ? trendPath(q.data.steps, q.data.tMax) : 'M0,14 H96'} fill="none" stroke={color} strokeWidth="1.6" strokeLinejoin="round" />
    </svg>
  );
}

export function OptimizationsTable({ rows, onOpen }: { rows: Optimization[]; onOpen?: (o: Optimization) => void }) {
  const [filter, setFilter] = useState<Filter>('all');
  const shown = filter === 'all' ? rows : rows.filter((o) => GROUPS[filter].includes(o.status));
  const count = (f: Filter) => (f === 'all' ? rows.length : rows.filter((o) => GROUPS[f].includes(o.status)).length);
  return (
    <Card className="table-card" data-testid="optimizations-table">
      <div className="table-head">
        <h2 className="card-title">All optimizations</h2>
        <div className="chips" role="group" aria-label="Filter by status">
          {FILTERS.map((f) => (
            <button key={f.id} type="button" className={`chip${filter === f.id ? ' chip--on' : ''}`} aria-pressed={filter === f.id} onClick={() => setFilter(f.id)}>
              {f.label}{f.id === 'all' || filter === f.id ? ` · ${count(f.id)}` : ''}
            </button>
          ))}
        </div>
      </div>
      <div className="table-scroll">
        <table className="table">
          <thead>
            <tr>
              <th>Optimization</th><th>Trend</th><th className="num">Baseline → best</th><th>Gain</th><th>Agents</th><th className="num">Spend</th><th>Status</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((o) => {
              const has = o.baseline !== null;
              const openable = onOpen && o.status !== 'draft';
              const gain = gainLabel(o);
              return (
                <tr key={o.id} className={openable ? 'row-link' : undefined} onClick={openable ? () => onOpen(o) : undefined}>
                  <td>
                    <div className="opt-cell">
                      <span className="opt-icon mono" aria-hidden="true">{metricGlyph(o.metric)}</span>
                      <span className="opt-name">
                        <span className="table__name">
                          {openable ? <button type="button" className="link-btn table__name" onClick={(e) => { e.stopPropagation(); onOpen(o); }}>{o.name}</button> : o.name}
                        </span>
                        <span className="table__sub mono">{o.repo} · {o.metric}</span>
                      </span>
                    </div>
                  </td>
                  <td><Trend o={o} /></td>
                  <td className="mono num">
                    {has ? <><span className="faint">{fmtMetric(o.baseline, o.unit)}</span>{'  →  '}<span>{fmtMetric(o.best, o.unit)}</span></> : <span className="faint">—</span>}
                  </td>
                  <td><span className={`gain-badge mono${gain ? '' : ' gain-badge--none'}`}>{gain ?? '—'}</span></td>
                  <td><span className="avatar-stack">{o.agents.map((a) => <AgentAvatar key={a} agent={a} size={24} ring />)}</span></td>
                  <td className="mono num muted">{o.spendUsd ? `$${o.spendUsd.toFixed(0)}` : '$0'}</td>
                  <td><StatusPill status={o.status} /></td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Card>
  );
}
