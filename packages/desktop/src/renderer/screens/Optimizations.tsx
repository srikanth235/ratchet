import { AgentAvatar, Button, Card, Eyebrow, StairChart, StatusPill } from '@ratchet/ui';
import { ActivityFeed } from '../components/ActivityFeed';
import { ErrorNote, Empty, PageHeader } from '../components/common';
import { Icon } from '../components/icons';
import { KpiCard } from '../components/KpiCard';
import { OptimizationsTable } from '../components/OptimizationsTable';
import { useNav, useSnapshot } from '../data';
import { fmtMetric } from '../format';

export function Optimizations() {
  const { data, error } = useSnapshot();
  const { go, open } = useNav();

  if (error && !data) return <ErrorNote error={`Failed to load optimizations: ${error}`} />;
  if (!data) return <p className="muted">Loading…</p>;

  const featured = data.optimizations.find((o) => o.id === data.featuredId);
  const series = data.featuredSeries;

  return (
    <>
      <PageHeader
        title="Optimizations"
        sub="Agents ratchet your benchmarks down — only verified wins ship."
        actions={<Button onClick={() => go('new')}><Icon name="plus" size={16} /> New optimization</Button>}
      />

      <section className="kpis" aria-label="Key metrics">
        {data.kpis.map((k) => <KpiCard key={k.id} kpi={k} />)}
      </section>

      {data.optimizations.length === 0 ? (
        <Card><Empty title="No optimizations yet">Create one to start a baseline run.</Empty></Card>
      ) : (
        <section className="grid-main">
          {featured && (
            <Card className="featured">
              <div className="featured__head">
                <div>
                  <Eyebrow>{featured.status === 'running' || featured.status === 'baselining' ? 'Live optimization' : 'Latest optimization'}</Eyebrow>
                  <h2 className="featured__title">{featured.name}</h2>
                  <div className="mono featured__repo">{featured.repo}</div>
                </div>
                <StatusPill status={featured.status} />
              </div>
              <div className="featured__hero">
                <span className="hero-num hero-num--lg">{fmtMetric(featured.baseline, featured.unit)}</span>
                <span className="arrow">→</span>
                <span className="hero-num hero-num--lg brass">{fmtMetric(featured.best, featured.unit)}</span>
                {featured.gainPct !== null && (
                  <span className="mono gain">{featured.gainPct >= 0 ? (featured.direction === 'lower' ? '−' : '+') : ''}{Math.abs(featured.gainPct).toFixed(0)}%</span>
                )}
                <span className="avatars">{featured.agents.map((a) => <AgentAvatar key={a} agent={a} />)}</span>
              </div>
              {series && series.steps.length > 0 && (
                <StairChart steps={series.steps} points={series.points} direction={series.direction} unit={series.unit} tMax={series.tMax}
                  noiseBand={series.baseline !== null && series.noiseFloor !== null ? { center: series.baseline, halfWidth: series.noiseFloor } : undefined} />
              )}
              <div className="featured__links">
                <button type="button" className="link-btn" onClick={() => open(featured.id, 'agents')}>Live agents →</button>
                <button type="button" className="link-btn" onClick={() => open(featured.id, 'benchmarks')}>Benchmark history →</button>
              </div>
            </Card>
          )}
          <ActivityFeed items={data.activity} />
        </section>
      )}

      <OptimizationsTable rows={data.optimizations} onOpen={(o) => open(o.id, o.status === 'running' || o.status === 'baselining' ? 'agents' : 'result')} />
    </>
  );
}
