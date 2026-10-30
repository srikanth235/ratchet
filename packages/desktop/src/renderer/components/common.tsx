import type { ReactNode } from 'react';
import { Eyebrow } from '@ratchet/ui';
import { useNav, useSnapshot } from '../data';

export function PageHeader({ title, sub, actions }: { title: string; sub?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="page-header">
      <div>
        <h1 className="page-title">{title}</h1>
        {sub && <p className="page-sub">{sub}</p>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </header>
  );
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
      <select value={selectedId ?? ''} onChange={(e) => select(e.target.value || null)} aria-label="Optimization">
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
