import { useState } from 'react';
import { ValueStair } from '@ratchet/ui';
import { OVERVIEW_RANGES, type OverviewNeed, type OverviewRange, type OverviewVM } from '../../shared/types';
import { EmptyOptimizations, ErrorNote } from '../components/common';
import { OptimizationsTable, StatusDot } from '../components/OptimizationsTable';
import { useNav, useQuery } from '../data';
import { fmtPayback, fmtReturn, fmtUsdCompact, fmtUsdCell } from '../format';

const RANGE_LABEL: Record<OverviewRange, string> = { '30d': '30 days', '90d': '90 days', all: 'All time' };

export function RangeSwitch({ value, onChange }: { value: OverviewRange; onChange(r: OverviewRange): void }) {
  return (
    <div className="range" role="radiogroup" aria-label="Range">
      {OVERVIEW_RANGES.map((r) => (
        <button key={r} type="button" role="radio" aria-checked={value === r} className={value === r ? 'on' : undefined} onClick={() => onChange(r)}>
          {RANGE_LABEL[r]}
        </button>
      ))}
    </div>
  );
}

const plural = (n: number, one: string): string => `${n} ${one}${n === 1 ? '' : 's'}`;

function Hero({ o }: { o: OverviewVM }) {
  const pct = o.spent.budgetUsd > 0 ? Math.min(100, (o.spent.usd / o.spent.budgetUsd) * 100) : 0;
  const v = o.value;
  return (
    <section className="hero" aria-label="Spend and return">
      <div data-testid="hero-spent">
        <div className="lbl">Spent</div>
        <div className="big serif">{fmtUsdCell(o.spent.usd)}</div>
        {o.spent.budgetUsd > 0 && (
          <div className="bar" role="img" aria-label={`${Math.round(pct)}% of the budget`}><i style={{ width: `${pct}%` }} /></div>
        )}
        <div className="note">
          {o.spent.budgetUsd > 0 ? `of ${fmtUsdCompact(o.spent.budgetUsd)} budgeted across ${plural(o.spent.optimizations, 'optimization')}` : 'nothing spent in this range'}
        </div>
      </div>
      <div data-testid="hero-value">
        <div className="lbl">Value locked in</div>
        {v ? (
          <>
            <div className="big serif">{fmtUsdCell(v.usdPerYear)}<small>/ yr</small></div>
            <div className="note">{v.wins > 0 ? `from ${plural(v.wins, 'verified win')} (${v.basis})` : 'no verified wins in this range'}</div>
          </>
        ) : (
          <>
            <div className="big serif">—</div>
            <div className="note" data-testid="set-a-value">Set a value to see what the wins are worth</div>
          </>
        )}
      </div>
      <div data-testid="hero-return">
        <div className="lbl">Return</div>
        <div className="big serif">{o.returnOnSpend ? fmtReturn(o.returnOnSpend.ratio) : '—'}</div>
        <div className="note">
          {o.returnOnSpend
            ? `yearly value for every $1 spent · paid back in ${fmtPayback(o.returnOnSpend.paybackDays)}`
            : v ? 'needs a verified win and some spend' : 'needs a value'}
        </div>
      </div>
    </section>
  );
}

function Needs({ items, onOpen }: { items: OverviewNeed[]; onOpen(n: OverviewNeed): void }) {
  return (
    <div data-testid="needs-you">
      <h2 className="section-title">Needs you</h2>
      {items.length > 0 && (
        <ul className="list">
          {items.map((n) => (
            <li key={n.id} className="li">
              <StatusDot tone={n.kind === 'failed' ? 'bad' : 'wait'} />
              <div className="li__text"><div className="t">{n.title}</div><div className="s">{n.sub}</div></div>
              <button type="button" className="btn-sm" onClick={() => onOpen(n)} aria-label={`${n.action}: ${n.title}`}>{n.action}</button>
            </li>
          ))}
        </ul>
      )}
      <p className="note">{items.length > 0 ? 'Nothing else is waiting on you.' : 'Nothing is waiting on you.'}</p>
    </div>
  );
}

export function Overview() {
  const [range, setRange] = useState<OverviewRange>('90d');
  const { go, open } = useNav();
  const q = useQuery(() => window.ratchet.overview(range), [range], { intervalMs: 10_000 });
  const o = q.data;

  // With no optimization at all the empty state carries the one New optimization button.
  const header = (
    <header className="head">
      <h1 className="serif">Overview</h1>
      {!o?.empty && <RangeSwitch value={range} onChange={setRange} />}
      {!o?.empty && <button type="button" className="btn pri" onClick={() => go('new')}>New optimization</button>}
    </header>
  );

  if (q.error && !o) return <>{header}<ErrorNote error={`Failed to load the overview: ${q.error}`} /></>;
  if (!o) return <>{header}<p className="muted">Loading…</p></>;

  const rows = o.rows.filter((r) => r.status !== 'draft'); // drafts have nothing to show yet: they live on Optimizations
  if (o.empty) return <>{header}<EmptyOptimizations onNew={() => go('new')} /></>;

  const openRow = (id: string, live: boolean) => open(id, live ? 'agents' : 'result');
  return (
    <>
      {header}
      <Hero o={o} />
      <div className="grid2">
        <div>
          <h2 className="section-title">Value locked in, per year</h2>
          {o.value ? (
            <>
              <ValueStair from={o.from} to={o.to} steps={o.wins.map((w) => ({ at: w.at, value: w.cumulativeUsdPerYear, label: w.optimization }))} />
              {o.wins.length === 0 && <p className="note">No verified wins in this range yet.</p>}
            </>
          ) : (
            <p className="note">Once an optimization has a value, every verified win adds a step here.</p>
          )}
        </div>
        <Needs items={o.needsYou} onOpen={(n) => open(n.optimizationId, 'result')} />
      </div>
      <section>
        <h2 className="section-title">Optimizations</h2>
        {rows.length === 0 ? (
          <p className="note">Nothing ran in this range.</p>
        ) : (
          <OptimizationsTable rows={rows} onOpen={(r) => openRow(r.id, r.live)} caption="Optimizations in this range" />
        )}
      </section>
    </>
  );
}
