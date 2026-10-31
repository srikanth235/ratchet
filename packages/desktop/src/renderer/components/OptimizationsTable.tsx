import type { OverviewRow, StatusTone } from '../../shared/types';
import { fmtDuration, fmtNum, fmtUsdCell, gainLabel, gainPct, repoLabel } from '../format';

/** Status as a dot and a word: the only status marker of the calm screens. */
export function StatusDot({ tone }: { tone: StatusTone }) {
  return <span className={`rt-dot rt-dot--${tone}`} aria-hidden="true" />;
}

/** `412 → 288 ms`; the unit only when the metric has one. */
function fromTo(r: OverviewRow): string {
  const unit = r.unit ? ` ${r.unit}` : '';
  return `${fmtNum(r.baseline!)} → ${fmtNum(r.best!)}${unit}`;
}

function Result({ r }: { r: OverviewRow }) {
  const g = gainPct(r);
  const gain = r.verifiedWins > 0 && g !== null ? gainLabel(r, Math.abs(g) < 10 ? 1 : 0) : null;
  if (gain) return <><span className="up mono">{gain}</span> <span className="s">{fromTo(r)}</span></>;
  if (r.status === 'draft') return <span className="s">not started</span>;
  if (r.live) return <span className="s">{r.baseline === null ? 'measuring the baseline' : 'no win yet'}</span>;
  return <span className="s">nothing beat the noise</span>;
}

/**
 * The optimizations as a table with hairlines: name and repo, status dot and word, result, spend (with the budget
 * while it runs), value per year. Used by the Overview and the Optimizations list.
 */
export function OptimizationsTable({ rows, onOpen, caption }: { rows: OverviewRow[]; onOpen?: (r: OverviewRow) => void; caption?: string }) {
  return (
    <div className="table-scroll" data-testid="optimizations-table">
      <table className="calm-table">
        {caption && <caption className="sr-only">{caption}</caption>}
        <thead>
          <tr>
            <th className="col-name">Optimization</th>
            <th>Status</th>
            <th>Result</th>
            <th className="r">Spent</th>
            <th className="r">Value / yr</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const openable = onOpen && r.status !== 'draft';
            return (
              <tr key={r.id} className={openable ? 'row-link' : undefined} onClick={openable ? () => onOpen(r) : undefined}>
                <td>
                  {openable ? (
                    <button type="button" className="row-name" onClick={(e) => { e.stopPropagation(); onOpen(r); }}>{r.name}</button>
                  ) : (
                    <span className="row-name">{r.name}</span>
                  )}
                  <div className="s">{repoLabel(r.repo)}</div>
                </td>
                <td>
                  <StatusDot tone={r.tone} />{r.statusLabel}
                  {r.remainingS !== null && <div className="s row-note">{fmtDuration(r.remainingS)} left</div>}
                </td>
                <td><Result r={r} /></td>
                <td className="r mono">
                  {fmtUsdCell(r.spentUsd)}
                  {r.live && r.budgetUsd > 0 && <> <span className="s">/ {Math.round(r.budgetUsd)}</span></>}
                </td>
                <td className="r mono">{r.valueUsdPerYear !== null && r.valueUsdPerYear > 0 ? fmtUsdCell(r.valueUsdPerYear) : <span className="s">—</span>}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
