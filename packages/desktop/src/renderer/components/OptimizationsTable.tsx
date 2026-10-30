import { AgentAvatar, Card, Eyebrow, StatusPill } from '@ratchet/ui';
import type { Optimization } from '../../shared/types';
import { fmtMetric, gainPct } from '../format';

export function OptimizationsTable({ rows }: { rows: Optimization[] }) {
  return (
    <Card className="table-card">
      <Eyebrow>All optimizations</Eyebrow>
      <table className="table">
        <thead>
          <tr><th>Optimization</th><th>Metric</th><th>Baseline → Best</th><th>Gain</th><th>Agents</th><th>Spend</th><th>Status</th></tr>
        </thead>
        <tbody>
          {rows.map((o) => {
            const g = gainPct(o);
            const has = o.baseline > 0;
            return (
              <tr key={o.id}>
                <td><div className="table__name">{o.name}</div><div className="table__sub mono">{o.repo}</div></td>
                <td>{o.metric}</td>
                <td className="mono">{has ? `${fmtMetric(o.baseline, o.unit)} → ${fmtMetric(o.best, o.unit)}` : '—'}</td>
                <td className="mono gain">{g === null ? '—' : `${g.toFixed(0)}%`}</td>
                <td><span className="avatars">{o.agents.map((a) => <AgentAvatar key={a} agent={a} size={22} />)}</span></td>
                <td className="mono">{o.spendUsd ? `$${o.spendUsd.toFixed(2)}` : '—'}</td>
                <td><StatusPill status={o.status} /></td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </Card>
  );
}
