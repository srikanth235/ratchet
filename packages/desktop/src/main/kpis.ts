import { spendEvents, type Ledger, type OptimizationSummary } from '@ratchet/core';
import type { Kpi, KpiChange, KpiRange } from '../shared/types.js';
import { gainVsBaseline, valueOfMove } from './value.js';

const DAY = 24 * 3600_000;
export const RANGE_DAYS: Record<KpiRange, number> = { '7d': 7, '30d': 30, '90d': 90 };

/** Sparkline resolution: samples between the first activity in the range and now. */
const TREND_POINTS = 10;

interface Move {
  opt: string;
  at: number;
  /** Gain vs the optimization's baseline after this move, percent (positive = better). */
  gainPct: number;
  /** Yearly value of this step (objective.value), null when the optimization has no value model. */
  usdPerYear: number | null;
}

interface Window {
  from: number;
  to: number;
}

const inside = (w: Window) => (at: number) => at >= w.from && at < w.to;

export function median(xs: readonly number[]): number {
  if (xs.length === 0) return 0;
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m]! : (s[m - 1]! + s[m]!) / 2;
}

/** Median over optimizations of the gain vs baseline they had reached by `at` (only those with a win in `w`). */
function medianGain(moves: Move[], w: Window): { value: number; n: number } {
  const last = new Map<string, number>();
  for (const m of moves) if (m.at >= w.from && m.at < w.to) last.set(m.opt, m.gainPct);
  return { value: median([...last.values()]), n: last.size };
}

const sum = (xs: number[]) => xs.reduce((a, b) => a + b, 0);
const sign = (n: number) => (n > 0 ? '+' : n < 0 ? '−' : '±');

function countChange(cur: number, prev: number): KpiChange {
  const d = cur - prev;
  return { text: `${sign(d)}${Math.abs(d)}`, tone: d > 0 ? 'good' : d < 0 ? 'bad' : 'neutral' };
}

/** Relative change; `lowerIsBetter` flips the tone; null when there is no previous value to divide by. */
function pctChange(cur: number, prev: number, tone: 'higher' | 'lower' | 'neutral'): KpiChange | null {
  if (!(prev > 0)) return null;
  const d = ((cur - prev) / prev) * 100;
  const r = Math.round(d);
  const good = tone === 'neutral' || r === 0 ? 'neutral' : (r > 0) === (tone === 'higher') ? 'good' : 'bad';
  return { text: `${sign(r)}${Math.abs(r)}%`, tone: good };
}

export const usd = (v: number): string => {
  const a = Math.abs(v);
  if (a >= 1_000_000) return `$${(v / 1_000_000).toFixed(1)}M`;
  if (a >= 10_000) return `$${(v / 1000).toFixed(1)}k`;
  if (a >= 100) return `$${Math.round(v).toLocaleString('en-US')}`;
  return `$${v.toFixed(a >= 10 ? 0 : 1)}`;
};

export const roi = (x: number): string =>
  x >= 1_000_000 ? `${(x / 1_000_000).toFixed(1)}M×` : x >= 10_000 ? `${(x / 1000).toFixed(1)}k×` : `${x >= 100 ? x.toFixed(0) : x.toFixed(1)}×`;

/**
 * KPI cards for the Optimizations screen over `range`, each compared with the equal-length window before it
 * (no chip when that window has no data). Everything comes from the ledger: ratchet moves, baselines, spend events
 * and the optimizations' `objective.value` columns. The value KPI appears only when some optimization has a value model.
 */
export function buildKpis(ledger: Ledger, rows: OptimizationSummary[], now: number, range: KpiRange = '30d'): Kpi[] {
  const len = RANGE_DAYS[range] * DAY;
  const cur: Window = { from: now - len, to: now + 1 };
  const prev: Window = { from: now - 2 * len, to: now - len };

  const optRows = new Map(rows.map((r) => [r.id, ledger.getOptimization(r.id)!]));
  const moves: Move[] = rows
    .flatMap((r) => {
      const row = optRows.get(r.id)!;
      return ledger.ratchetHistory(r.id).map((m) => ({
        opt: r.id,
        at: m.at,
        gainPct: r.baselineValue === null ? m.gainPct : gainVsBaseline(r.direction, r.baselineValue, m.toValue),
        usdPerYear: valueOfMove(row, m.fromValue, m.toValue),
      }));
    })
    .sort((a, b) => a.at - b.at);
  const ids = new Set(rows.map((r) => r.id));
  const spend = ledger.db
    .select({ at: spendEvents.at, usd: spendEvents.usd, opt: spendEvents.optimizationId })
    .from(spendEvents)
    .all()
    .filter((e) => ids.has(e.opt))
    .sort((a, b) => a.at - b.at);

  const curMoves = moves.filter((m) => inside(cur)(m.at));
  const prevMoves = moves.filter((m) => inside(prev)(m.at));
  const curSpend = sum(spend.filter((e) => inside(cur)(e.at)).map((e) => e.usd));
  const prevSpend = sum(spend.filter((e) => inside(prev)(e.at)).map((e) => e.usd));
  // "Previous data" = the workspace did anything (spent or ratcheted) in the previous window.
  const hadPrev = prevMoves.length > 0 || spend.some((e) => inside(prev)(e.at));

  // Sparklines: from the first activity inside the range to now, so a quiet start does not flatten the line.
  const first = Math.min(...[...curMoves.map((m) => m.at), ...spend.filter((e) => inside(cur)(e.at)).map((e) => e.at)], now);
  const ticks = Array.from({ length: TREND_POINTS }, (_, i) => first + ((now - first) * (i + 1)) / TREND_POINTS);
  const upTo = (t: number): Window => ({ from: cur.from, to: t + 1 });
  const spendTo = (t: number) => sum(spend.filter((e) => inside(upTo(t))(e.at)).map((e) => e.usd));
  const winsTo = (t: number) => curMoves.filter((m) => m.at <= t).length;

  const kpis: Kpi[] = [];

  // Verified wins
  const optsWithWins = new Set(curMoves.map((m) => m.opt)).size;
  kpis.push({
    id: 'wins',
    label: 'Verified wins',
    value: String(curMoves.length),
    unit: `last ${range}`,
    change: hadPrev ? countChange(curMoves.length, prevMoves.length) : null,
    foot: curMoves.length ? `across ${optsWithWins} optimization${optsWithWins === 1 ? '' : 's'}` : 'none in this range',
    trend: ticks.map(winsTo),
  });

  // Median gain vs baseline of the optimizations that ratcheted in the range
  const g = medianGain(moves, cur);
  const gPrev = medianGain(moves, prev);
  const gd = Math.round(g.value) - Math.round(gPrev.value);
  kpis.push({
    id: 'gain',
    label: 'Median gain',
    value: g.n ? g.value.toFixed(0) : '—',
    unit: g.n ? '% better' : '',
    change: g.n && gPrev.n ? { text: `${sign(gd)}${Math.abs(gd)} pts`, tone: gd > 0 ? 'good' : gd < 0 ? 'bad' : 'neutral' } : null,
    foot: g.n ? `vs baseline · ${g.n} optimization${g.n === 1 ? '' : 's'}` : 'no verified wins in this range',
    trend: g.n ? ticks.map((t) => medianGain(moves, upTo(t)).value) : [],
  });

  // Spend
  const dayAgo = now - DAY;
  const today = sum(spend.filter((e) => e.at >= dayAgo).map((e) => e.usd));
  kpis.push({
    id: 'spend',
    label: 'Spend',
    value: usd(curSpend),
    change: hadPrev ? pctChange(curSpend, prevSpend, 'neutral') : null,
    foot: `${usd(today)} in the last 24h`,
    trend: ticks.map(spendTo),
  });

  // Cost per win
  const cpw = curMoves.length ? curSpend / curMoves.length : null;
  const prevCpw = prevMoves.length ? prevSpend / prevMoves.length : null;
  kpis.push({
    id: 'cpw',
    label: 'Cost per win',
    value: cpw === null ? '—' : usd(cpw),
    unit: cpw === null ? '' : 'avg',
    change: cpw !== null && prevCpw !== null ? pctChange(cpw, prevCpw, 'lower') : null,
    foot: cpw === null ? 'no verified wins in this range' : 'all spend ÷ verified wins',
    trend: ticks.filter((t) => winsTo(t) > 0).map((t) => spendTo(t) / winsTo(t)),
  });

  // Value created (only with a value model somewhere)
  const valued = rows.filter((r) => optRows.get(r.id)!.valueUsdPerUnit !== null);
  if (valued.length > 0) {
    const vIds = new Set(valued.map((r) => r.id));
    const valueIn = (w: Window) => sum(moves.filter((m) => inside(w)(m.at) && m.usdPerYear !== null).map((m) => m.usdPerYear!));
    const v = valueIn(cur);
    const vSpend = sum(spend.filter((e) => vIds.has(e.opt) && inside(cur)(e.at)).map((e) => e.usd));
    const estimated = valued.some((r) => optRows.get(r.id)!.valueBasis !== 'measured');
    kpis.push({
      id: 'value',
      label: 'Value created',
      value: usd(v),
      change: hadPrev ? pctChange(v, valueIn(prev), 'higher') : null,
      foot: `per year${vSpend > 0 && v > 0 ? ` · ${roi(v / vSpend)} ROI` : ''} · ${estimated ? 'estimated' : 'measured'}`,
      trend: ticks.map((t) => valueIn(upTo(t))),
    });
  }
  return kpis;
}
