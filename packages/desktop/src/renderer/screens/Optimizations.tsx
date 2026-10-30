import { AgentAvatar, Button, Card, StairChart, StatusPill, agentColor, agentLabel } from '@ratchet/ui';
import { ActivityFeed } from '../components/ActivityFeed';
import { Empty, ErrorNote, PageHeader } from '../components/common';
import { Icon } from '../components/icons';
import { KpiCard } from '../components/KpiCard';
import { OptimizationsTable } from '../components/OptimizationsTable';
import { forOptimization, useNav, useQuery, useSnapshot } from '../data';
import { KPI_RANGES } from '../../shared/types';
import { fmtDuration, fmtNum, fmtUsdCompact, gainLabel, repoLabel } from '../format';

export function Optimizations() {
  const { data, error, range, setRange } = useSnapshot();
  const { go, open } = useNav();
  const featured = data?.optimizations.find((o) => o.id === data.featuredId);
  const detail = useQuery(() => window.ratchet.getOptimization(featured!.id), [featured?.id], {
    skip: !featured, relevant: forOptimization(featured?.id ?? null), intervalMs: 10_000,
  });

  if (error && !data) return <ErrorNote error={`Failed to load optimizations: ${error}`} />;
  if (!data) return <p className="muted">Loading…</p>;

  const series = data.featuredSeries;
  const repos = new Set(data.optimizations.map((o) => o.repo)).size;
  const d = detail.data;
  const live = featured && (featured.status === 'running' || featured.status === 'baselining');
  const evals = featured?.candidateCount ?? 0;
  const blocked = d?.agents.reduce((n, a) => n + a.stats.blocked, 0) ?? 0;
  const rate = d && d.elapsedS > 60 ? (evals / d.elapsedS) * 3600 : null;
  const locks = series ? Math.max(0, series.steps.length - 1) : 0;
  const gain = featured ? gainLabel(featured, 1) : null;
  const value = featured?.value ?? null;
  const spendPct = featured && featured.budgetUsd > 0 ? Math.min(100, (featured.spendUsd / featured.budgetUsd) * 100) : 0;

  return (
    <>
      <PageHeader
        eyebrow={<span>Local workspace · {repos} repo{repos === 1 ? '' : 's'}</span>}
        title="Optimizations"
        sub={<>Agents propose changes. The evaluator decides. <span className="tagline">Only verified wins ratchet forward.</span></>}
        actions={
          <>
            <div className="range-switch" role="radiogroup" aria-label="KPI range">
              {KPI_RANGES.map((r) => (
                <button key={r} type="button" role="radio" aria-checked={range === r} className={range === r ? 'range-switch__on' : ''} onClick={() => setRange(r)}>{r}</button>
              ))}
            </div>
            <Button onClick={() => go('new')}><Icon name="plus" size={13} /> New optimization</Button>
          </>
        }
      />

      {/* Five cards (with the value KPI) still fit one row: narrower columns, shorter sparklines. */}
      <section className={`kpis${data.kpis.length > 4 ? ' kpis--5' : ''}`} aria-label="Key metrics">
        {data.kpis.map((k) => <KpiCard key={k.id} kpi={k} sparkWidth={data.kpis.length > 4 ? 72 : 112} />)}
      </section>

      {data.optimizations.length === 0 ? (
        <Card><Empty title="No optimizations yet">Create one to start a baseline run.</Empty></Card>
      ) : (
        <section className="grid-main">
          {featured && (
            <Card className="featured" data-testid="featured">
              <div className="featured__head">
                <div className="featured__id">
                  {live ? (
                    <span className="featured__eyebrow">
                      <span className="halo-dot" aria-hidden="true" />
                      Live{d ? ` · ${fmtDuration(d.elapsedS)} of ${fmtDuration(d.timeLimitS)}` : ''}
                    </span>
                  ) : (
                    <span className="featured__eyebrow"><StatusPill status={featured.status} /></span>
                  )}
                  <h2 className="featured__title">{featured.name}</h2>
                  <span className="mono featured__repo">{repoLabel(featured.repo)} · {featured.metric}</span>
                </div>
                <div className="featured__side">
                  <span className="avatars">{featured.agents.map((a, i) => <AgentAvatar key={a} agent={a} size={28} ring overlap={i ? 8 : 0} />)}</span>
                  <button type="button" className="open-btn" onClick={() => open(featured.id, live ? 'agents' : 'result')}>Open <Icon name="chevron" size={12} /></button>
                </div>
              </div>

              <div className="featured__hero">
                <div className="hero-big">
                  <span className="serif" data-testid="featured-best">{featured.best === null ? '—' : fmtNum(featured.best)}</span>
                  {featured.unit && <span className="hero-big__unit">{featured.unit}</span>}
                </div>
                <div className="hero-meta">
                  {gain && <span className="mono gain">{gain} vs baseline</span>}
                  <span className="hero-meta__sub">
                    {featured.baseline !== null ? `baseline ${fmtNum(featured.baseline)} ${featured.unit}`.trim() : 'baseline pending'}
                    {series?.noiseFloor != null ? ` · noise ±${fmtNum(series.noiseFloor)} ${featured.unit}`.trimEnd() : ''}
                    {` · ${locks} lock${locks === 1 ? '' : 's'}`}
                    {value ? ` · ≈ ${fmtUsdCompact(value.usdPerYear)} / year` : ''}
                  </span>
                </div>
                <div className="chart-legend">
                  <span><i className="legend-line" aria-hidden="true" />Best verified</span>
                  {featured.agents.map((a) => (
                    <span key={a}><i className="legend-dot" style={{ background: agentColor(a) }} aria-hidden="true" />{agentLabel(a)}</span>
                  ))}
                </div>
              </div>

              {series && series.steps.length > 0 && (
                <StairChart steps={series.steps} points={series.points} direction={series.direction} unit={series.unit} tMax={series.tMax}
                  {...(live && d ? { limit: d.timeLimitS / 60 } : {})} />
              )}

              <div className="strip">
                <div className="strip__cell">
                  <div className="rt-eyebrow">Spend</div>
                  <div className="strip__value mono">${featured.spendUsd.toFixed(0)} <span className="faint">/ {featured.budgetUsd.toFixed(0)}</span></div>
                  <div className="track" aria-hidden="true"><span style={{ width: `${spendPct}%` }} /></div>
                </div>
                <div className="strip__cell">
                  <div className="rt-eyebrow">Evals</div>
                  <div className="strip__value mono">{evals}</div>
                  <div className="strip__foot">{rate !== null ? `≈ ${rate.toFixed(1)} / hour` : 'warming up'}</div>
                </div>
                <div className="strip__cell">
                  <div className="rt-eyebrow">Verified</div>
                  <div className="strip__value mono gain">{featured.verifiedWins}</div>
                  <div className="strip__foot">of {evals} evals</div>
                </div>
                <div className="strip__cell">
                  <div className="rt-eyebrow">Blocked</div>
                  <div className="strip__value mono" style={{ color: 'var(--rt-color-rose)' }}>{blocked}</div>
                  <div className="strip__foot">by guardrails</div>
                </div>
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
