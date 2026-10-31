import { useNav, useSnapshot } from '../data';
import { NAV, navFor } from '../nav';
import { NavIcon, RatchetMark } from './icons';
import { SandboxStatus } from './SandboxStatus';

const isMac = typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent);

/** Logo, four places to go, and the sandbox status line. Nothing else. */
export function Sidebar() {
  const { route, go } = useNav();
  const snap = useSnapshot().data;
  const active = navFor(route);

  return (
    <aside className="sidebar">
      {isMac && <div className="sidebar__titlebar" aria-hidden="true" />}
      <div className="brand">
        <span className="brand__mark" aria-hidden="true"><RatchetMark /></span>
        <span className="brand__name">Ratchet</span>
      </div>

      <nav className="nav" aria-label="Primary">
        {NAV.map((n) => (
          <button
            key={n.id}
            type="button"
            className={`nav__item${n.id === active ? ' nav__item--active' : ''}`}
            aria-current={n.id === active ? 'page' : undefined}
            onClick={() => go(n.id)}
          >
            <NavIcon id={n.id} />
            {n.label}
          </button>
        ))}
      </nav>

      <div className="sidebar__foot">
        {snap?.engine.kind === 'dev' && <div className="dev-banner" role="note">Dev engine · simulated results ({snap.engine.mode})</div>}
        <SandboxStatus />
      </div>
    </aside>
  );
}
