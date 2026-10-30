import { useEffect, useState } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, StatusPill } from '@ratchet/ui';
import type { OptimizationsSnapshot } from '../../shared/types';
import { ActivityFeed } from '../components/ActivityFeed';
import { Icon } from '../components/icons';
import { KpiCard } from '../components/KpiCard';
import { OptimizationsTable } from '../components/OptimizationsTable';
import { RatchetChart } from '../components/RatchetChart';
import { fmtMetric, gainPct } from '../format';

export function Optimizations() {
  const [data, setData] = useState<OptimizationsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    window.ratchet.listOptimizations().then(setData, (e: unknown) => setError(String(e)));
  }, []);

  if (error) return <p role="alert">Failed to load optimizations: {error}</p>;
  if (!data) return <p className="muted">Loading…</p>;

  const featured = data.optimizations.find((o) => o.id === data.featuredId);

  return (
    <>
      <header className="page-header">
        <div>
          <h1 className="page-title">Optimizations</h1>
          <p className="page-sub">Agents ratchet your benchmarks down — only verified wins ship.</p>
        </div>
        <Button><Icon name="plus" size={16} /> New optimization</Button>
      </header>

      <section className="kpis" aria-label="Key metrics">
        {data.kpis.map((k) => <KpiCard key={k.id} kpi={k} />)}
      </section>

      <section className="grid-main">
        {featured && (
          <Card className="featured">
            <div className="featured__head">
              <div>
                <Eyebrow>Live optimization</Eyebrow>
                <h2 className="featured__title">{featured.name}</h2>
                <div className="mono featured__repo">{featured.repo}</div>
              </div>
              <StatusPill status={featured.status} />
            </div>
            <div className="featured__hero">
              <span className="hero-num hero-num--lg">{fmtMetric(featured.baseline, featured.unit)}</span>
              <span className="arrow">→</span>
              <span className="hero-num hero-num--lg brass">{fmtMetric(featured.best, featured.unit)}</span>
              <span className="mono gain">−{gainPct(featured)?.toFixed(0)}%</span>
              <span className="avatars">{featured.agents.map((a) => <AgentAvatar key={a} agent={a} />)}</span>
            </div>
            <RatchetChart opt={featured} />
          </Card>
        )}
        <ActivityFeed items={data.activity} />
      </section>

      <OptimizationsTable rows={data.optimizations} />
    </>
  );
}
