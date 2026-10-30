import type { ReactNode } from 'react';
import { Eyebrow, Pill, StatusPill } from '@ratchet/ui';
import type { OptimizationDetail } from '../../shared/types';
import { useNav, useSnapshot } from '../data';
import { fmtDuration } from '../format';
import type { NavId } from '../nav';

export interface PageHeaderProps {
  title: string;
  /** Breadcrumb / section label above the title. */
  eyebrow?: ReactNode;
  /** Smaller (44px) title, used by the per-optimization screens. */
  compact?: boolean;
  /** Pill next to the title. */
  badge?: ReactNode;
  sub?: ReactNode;
  actions?: ReactNode;
  tabs?: ReactNode;
}

export function PageHeader({ title, eyebrow, compact, badge, sub, actions, tabs }: PageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-header__row">
        <div className="page-header__titles">
          {eyebrow && <span className="rt-eyebrow page-eyebrow">{eyebrow}</span>}
          <div className="page-header__title-row">
            <h1 className={`page-title${compact ? ' page-title--sm' : ''}`}>{title}</h1>
            {badge}
          </div>
          {sub && <p className="page-sub">{sub}</p>}
        </div>
        {actions && <div className="page-actions">{actions}</div>}
      </div>
      {tabs}
    </header>
  );
}

/** `Optimizations / acme/search-service` */
export function Crumbs({ repo }: { repo?: string }) {
  const { go } = useNav();
  return (
    <>
      <button type="button" className="crumb" onClick={() => go('optimizations')}>Optimizations</button>
      <span aria-hidden="true">/</span>
      {repo ? <span className="mono">{repo}</span> : <span>New</span>}
    </>
  );
}

const TABS: { id: NavId; label: string }[] = [
  { id: 'agents', label: 'Agents' },
  { id: 'benchmarks', label: 'Benchmark history' },
  { id: 'result', label: 'Result' },
];

/** Per-optimization tab bar (Agents / Benchmark history / Result). */
export function OptTabs({ current }: { current: NavId }) {
  const { go } = useNav();
  return (
    <div className="tabs" role="tablist" aria-label="Optimization views">
      {TABS.map((t) => (
        <button key={t.id} type="button" role="tab" aria-selected={t.id === current} className={`tab${t.id === current ? ' tab--on' : ''}`} onClick={() => go(t.id)}>
          {t.label}
        </button>
      ))}
    </div>
  );
}

/** The status pill of a per-optimization header: `Live · 3h 10m of 8h` while running, the status otherwise. */
export function StateBadge({ detail }: { detail: OptimizationDetail }) {
  const s = detail.summary.status;
  if (s === 'running' || s === 'baselining') {
    return <Pill tone="brass" dot live large>{s === 'baselining' ? 'Baselining' : 'Live'} · {fmtDuration(detail.elapsedS)} of {fmtDuration(detail.timeLimitS)}</Pill>;
  }
  return <StatusPill status={s} large />;
}

export function ErrorNote({ error }: { error: string | null }) {
  return error ? <p role="alert" className="error-note">{error}</p> : null;
}

export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <Eyebrow>{title}</Eyebrow>
      {children && <p className="muted">{children}</p>}
    </div>
  );
}

/** Dropdown for the per-optimization screens. */
export function OptimizationPicker() {
  const { selectedId, select } = useNav();
  const snap = useSnapshot();
  const rows = snap.data?.optimizations.filter((o) => o.status !== 'draft') ?? [];
  if (rows.length === 0) return null;
  return (
    <label className="picker">
      <span className="sr-only">Optimization</span>
      <select value={selectedId ?? snap.data?.featuredId ?? ''} onChange={(e) => select(e.target.value || null)} aria-label="Optimization">
        {rows.map((o) => (
          <option key={o.id} value={o.id}>{o.name}</option>
        ))}
      </select>
    </label>
  );
}

/** Resolve the selected optimization id, falling back to the featured/first one. */
export function useSelectedId(): string | null {
  const { selectedId } = useNav();
  const snap = useSnapshot();
  const rows = snap.data?.optimizations ?? [];
  if (selectedId && rows.some((o) => o.id === selectedId)) return selectedId;
  return snap.data?.featuredId ?? rows.find((o) => o.status !== 'draft')?.id ?? null;
}
