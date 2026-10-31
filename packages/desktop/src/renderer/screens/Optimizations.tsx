import { useState } from 'react';
import type { OverviewRow } from '../../shared/types';
import { EmptyOptimizations, ErrorNote } from '../components/common';
import { OptimizationsTable } from '../components/OptimizationsTable';
import { useNav, useQuery } from '../data';

type Filter = 'all' | 'running' | 'done';
const FILTERS: { id: Filter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'done', label: 'Done' },
];
/** Running = live; Done = everything that has ended (drafts are only in All). */
const MATCH: Record<Filter, (r: OverviewRow) => boolean> = {
  all: () => true,
  running: (r) => r.live,
  done: (r) => !r.live && r.status !== 'draft',
};

/** Every optimization, newest first, in the same table as the Overview. */
export function Optimizations() {
  const { go, open } = useNav();
  const [filter, setFilter] = useState<Filter>('all');
  const q = useQuery(() => window.ratchet.overview('all'), [], { intervalMs: 10_000 });
  const rows = q.data?.rows ?? [];
  const shown = rows.filter(MATCH[filter]);

  return (
    <>
      <header className="head">
        <h1 className="serif">Optimizations</h1>
        {rows.length > 0 && (
          <div className="range" role="radiogroup" aria-label="Show">
            {FILTERS.map((f) => (
              <button key={f.id} type="button" role="radio" aria-checked={filter === f.id} className={filter === f.id ? 'on' : undefined} onClick={() => setFilter(f.id)}>
                {f.label}
              </button>
            ))}
          </div>
        )}
        <button type="button" className="btn pri" onClick={() => go('new')}>New optimization</button>
      </header>
      {q.error && !q.data ? (
        <ErrorNote error={`Failed to load optimizations: ${q.error}`} />
      ) : !q.data ? (
        <p className="muted">Loading…</p>
      ) : rows.length === 0 ? (
        <EmptyOptimizations onNew={() => go('new')} />
      ) : shown.length === 0 ? (
        <p className="note">{filter === 'running' ? 'Nothing is running.' : 'Nothing has finished yet.'}</p>
      ) : (
        <OptimizationsTable rows={shown} onOpen={(r) => open(r.id, r.live ? 'agents' : 'result')} caption="Optimizations" />
      )}
    </>
  );
}
