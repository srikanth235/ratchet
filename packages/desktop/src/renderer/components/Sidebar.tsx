import { Card, Eyebrow, Pill } from '@ratchet/ui';
import { useNav, useSnapshot } from '../data';
import { fmtMetric } from '../format';
import { NAV } from '../nav';
import { Icon } from './icons';

export function Sidebar() {
  const { route, go } = useNav();
  const snap = useSnapshot().data;
  const live = snap?.optimizations.find((o) => o.id === snap.featuredId && (o.status === 'running' || o.status === 'baselining'));
  const repos = new Set(snap?.optimizations.map((o) => o.repo) ?? []).size;
  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand__mark" aria-hidden="true" />
        <span className="brand__name">Ratchet</span>
      </div>

      <div className="workspace">
        <span className="workspace__avatar">R</span>
        <span className="workspace__text">
          <span className="workspace__name">Local workspace</span>
          <span className="workspace__sub">{repos} repo{repos === 1 ? '' : 's'}</span>
        </span>
      </div>

      <nav className="nav" aria-label="Primary">
        {NAV.map((n) => (
          <button
            key={n.id}
            type="button"
            className={`nav__item${n.id === route ? ' nav__item--active' : ''}`}
            aria-current={n.id === route ? 'page' : undefined}
            onClick={() => go(n.id)}
          >
            <Icon name={n.id} />
            {n.label}
          </button>
        ))}
      </nav>

      <div className="sidebar__spacer" />

      {snap?.engine.kind === 'dev' && (
        <div className="dev-banner" role="note">
          <Pill tone="neutral">Dev engine</Pill>
          <span>Simulated results · {snap.engine.mode}</span>
        </div>
      )}

      {live && (
        <Card className="live-card">
          <Eyebrow>Live now</Eyebrow>
          <div className="live-card__title">{live.name}</div>
          <Pill tone="brass" dot live>{live.agents.length} agent{live.agents.length === 1 ? '' : 's'} running</Pill>
          <div className="mono live-card__meta">{fmtMetric(live.best, live.unit)} · ${live.spendUsd.toFixed(2)}</div>
        </Card>
      )}
    </aside>
  );
}
