import { Eyebrow } from '@ratchet/ui';
import { useNav, useQuery, useSnapshot } from '../data';
import { fmtDuration, gainLabel } from '../format';
import { NAV } from '../nav';
import { NavIcon, RatchetMark } from './icons';

const isMac = typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent);

export function Sidebar() {
  const { route, go, open } = useNav();
  const snap = useSnapshot().data;
  const live = snap?.optimizations.find((o) => o.id === snap.featuredId && (o.status === 'running' || o.status === 'baselining'));
  const detail = useQuery(() => window.ratchet.getOptimization(live!.id), [live?.id], { skip: !live, intervalMs: 10_000 });
  const settings = useQuery(() => window.ratchet.getSettings(), [], { intervalMs: 15_000 });
  const repos = new Set(snap?.optimizations.map((o) => o.repo) ?? []).size;
  const total = snap?.optimizations.length ?? 0;
  const liveAgents = live?.agents.length ?? 0;
  const s = settings.data;
  const sim = s?.mode === 'simulation';
  const integrations: { label: string; on: boolean }[] = [
    { label: 'GitHub', on: !!s?.secrets.githubToken },
    { label: 'AWS', on: !!s && (s.awsProfile !== '' || sim) },
    { label: 'Claude Code', on: !!s?.secrets.anthropicApiKey },
    { label: 'Codex', on: !!s?.secrets.openaiApiKey },
  ];
  const counts: Record<string, string | undefined> = {
    optimizations: total > 0 ? String(total) : undefined,
    agents: liveAgents > 0 ? `${liveAgents} live` : undefined,
  };
  const progress = detail.data && detail.data.timeLimitS > 0 ? Math.min(100, (detail.data.elapsedS / detail.data.timeLimitS) * 100) : 0;
  const gain = live ? gainLabel(live) : null;

  return (
    <aside className="sidebar">
      {isMac && <div className="sidebar__titlebar" aria-hidden="true" />}
      <div className="brand">
        <span className="brand__mark" aria-hidden="true"><RatchetMark /></span>
        <span className="brand__name">Ratchet</span>
        <span className="brand__version mono">v0</span>
      </div>

      <div className="workspace">
        <span className="workspace__avatar">L</span>
        <span className="workspace__text">
          <span className="workspace__name">Local workspace</span>
          <span className="workspace__sub">{repos} repo{repos === 1 ? '' : 's'}</span>
        </span>
        <svg className="workspace__chevron" width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="#8592A6" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 6l3-3 3 3M5 10l3 3 3-3" /></svg>
      </div>

      <div className="nav-group">
        <Eyebrow className="nav-eyebrow">Workspace</Eyebrow>
        <nav className="nav" aria-label="Primary">
          {NAV.map((n) => (
            <button
              key={n.id}
              type="button"
              className={`nav__item${n.id === route ? ' nav__item--active' : ''}`}
              aria-current={n.id === route ? 'page' : undefined}
              onClick={() => go(n.id)}
            >
              <NavIcon id={n.id} />
              {n.label}
              {counts[n.id] && <span className="nav__count mono">{counts[n.id]}</span>}
            </button>
          ))}
        </nav>
      </div>

      {live && (
        <div className="live-group">
          <Eyebrow>Live now</Eyebrow>
          <button type="button" className="live-card" onClick={() => open(live.id, 'agents')} aria-label={`Open ${live.name}`}>
            <span className="live-card__row">
              <span className="halo-dot" aria-hidden="true" />
              <span className="live-card__title">{live.name}</span>
              {gain && <span className="live-card__gain mono">{gain}</span>}
            </span>
            <span className="track" aria-hidden="true"><span style={{ width: `${progress}%` }} /></span>
            <span className="live-card__meta mono">
              {detail.data ? `${fmtDuration(detail.data.elapsedS)} · ` : ''}${live.spendUsd.toFixed(0)} of ${live.budgetUsd.toFixed(0)}
            </span>
          </button>
        </div>
      )}

      <div className="sidebar__foot">
        <div className="integrations" aria-label="Integrations">
          {integrations.map((i) => (
            <span key={i.label} className={`integration${i.on ? ' integration--on' : ''}`} title={i.on ? 'Configured' : 'Not configured'}>
              <span className="integration__dot" aria-hidden="true" />{i.label}
            </span>
          ))}
        </div>
        {snap?.engine.kind === 'dev' && (
          <div className="dev-banner" role="note">Dev engine · simulated results ({snap.engine.mode})</div>
        )}
        <div className="user">
          <span className="user__avatar" aria-hidden="true">YO</span>
          <span className="user__text">
            <span className="user__name">You</span>
            <span className="user__sub">Local owner</span>
          </span>
        </div>
      </div>
    </aside>
  );
}
