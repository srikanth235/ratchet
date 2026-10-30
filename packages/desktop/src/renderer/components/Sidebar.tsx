import { Card, Eyebrow, Pill } from '@ratchet/ui';
import { NAV, type NavId } from '../nav';
import { Icon } from './icons';

export function Sidebar({ active, onNavigate }: { active: NavId; onNavigate: (id: NavId) => void }) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand__mark" aria-hidden="true" />
        <span className="brand__name">Ratchet</span>
      </div>

      <button className="workspace" type="button">
        <span className="workspace__avatar">A</span>
        <span className="workspace__text">
          <span className="workspace__name">Acme Platform</span>
          <span className="workspace__sub">3 repos</span>
        </span>
        <Icon name="chevron" size={16} />
      </button>

      <nav className="nav" aria-label="Primary">
        {NAV.map((n) => (
          <button
            key={n.id}
            type="button"
            className={`nav__item${n.id === active ? ' nav__item--active' : ''}`}
            aria-current={n.id === active ? 'page' : undefined}
            onClick={() => onNavigate(n.id)}
          >
            <Icon name={n.id} />
            {n.label}
          </button>
        ))}
      </nav>

      <div className="sidebar__spacer" />

      <Card className="live-card">
        <Eyebrow>Live now</Eyebrow>
        <div className="live-card__title">search-api p95</div>
        <Pill tone="brass" dot live>2 agents running</Pill>
        <div className="mono live-card__meta">3h 34m · $42.60</div>
      </Card>

      <div className="user">
        <span className="user__avatar">SR</span>
        <span className="user__text">
          <span className="user__name">Srikanth</span>
          <span className="user__sub">Owner</span>
        </span>
      </div>
    </aside>
  );
}
