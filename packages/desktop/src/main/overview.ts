import { spendEvents, type Ledger, type OptimizationRow, type OptimizationSummary } from '@ratchet/core';
import type { OverviewNeed, OverviewRange, OverviewRow, OverviewVM, OverviewWin, StatusTone } from '../shared/overview.js';
import { toUiStatus, unitForMetric, type MetaView } from './mappers.js';
import { valueOfMove } from './value.js';

const DAY = 24 * 3600_000;
const RANGE_DAYS: Record<Exclude<OverviewRange, 'all'>, number> = { '30d': 30, '90d': 90 };
/** A failed run stops asking for attention after this long. */
const FAILED_FOR_DAYS = 14;

const sum = (xs: readonly number[]): number => xs.reduce((a, b) => a + b, 0);

/** `−39%` for a lower-is-better win, `+5.9%` for a higher-is-better one (same wording as the renderer's `gainLabel`). */
function signedGain(direction: 'minimize' | 'maximize', gainPct: number | null): string | null {
  if (gainPct === null) return null;
  const sign = gainPct === 0 ? '' : direction === 'minimize' ? (gainPct > 0 ? '−' : '+') : gainPct > 0 ? '+' : '−';
  const a = Math.abs(gainPct);
  return `${sign}${a >= 10 ? a.toFixed(0) : a.toFixed(1)}%`;
}

const plural = (n: number, one: string, many = `${one}s`): string => `${n} ${n === 1 ? one : many}`;

interface Move {
  opt: string;
  at: number;
  usdPerYear: number | null;
}

/**
 * The Overview: what was spent, what the verified wins are worth, and what is waiting on the user, over `range`.
 * Everything comes from the ledger. Value uses the same model as the Result screen (`objective.value`, via `valueOfMove`),
 * so the staircase, the hero number and the table's Value column always add up.
 */
export function buildOverview(ledger: Ledger, meta: MetaView, now: number, range: OverviewRange = '90d'): OverviewVM {
  const summaries = ledger.listOptimizations();
  const rowsById = new Map<string, OptimizationRow>(summaries.map((s) => [s.id, ledger.getOptimization(s.id)!]));
  const moves: Move[] = summaries
    .flatMap((s) => ledger.ratchetHistory(s.id).map((m) => ({ opt: s.id, at: m.at, usdPerYear: valueOfMove(rowsById.get(s.id)!, m.fromValue, m.toValue) })))
    .sort((a, b) => a.at - b.at);
  const ids = new Set(summaries.map((s) => s.id));
  const spend = ledger.db
    .select({ at: spendEvents.at, usd: spendEvents.usd, opt: spendEvents.optimizationId })
    .from(spendEvents)
    .all()
    .filter((e) => ids.has(e.opt));

  const firstActivity = Math.min(now, ...summaries.filter((s) => s.status !== 'draft').map((s) => s.startedAt ?? s.createdAt), ...moves.map((m) => m.at), ...spend.map((e) => e.at));
  const from = range === 'all' ? (summaries.length > 0 ? firstActivity : now - 30 * DAY) : now - RANGE_DAYS[range] * DAY;
  const inside = (at: number): boolean => at >= from && at <= now;

  const hasValueModel = summaries.some((s) => rowsById.get(s.id)!.valueUsdPerUnit !== null);
  const needs = needsYou(summaries, meta, now);
  const waiting = new Set(needs.filter((n) => n.kind === 'ship').map((n) => n.optimizationId));

  const inRange = (s: OptimizationSummary): boolean => {
    if (range === 'all') return true;
    if (s.status === 'draft') return inside(s.createdAt);
    if (s.status === 'running' || s.status === 'baselining' || s.status === 'stopping') return true;
    return (
      inside(s.startedAt ?? s.createdAt) ||
      (s.endedAt !== null && inside(s.endedAt)) ||
      spend.some((e) => e.opt === s.id && inside(e.at)) ||
      moves.some((m) => m.opt === s.id && inside(m.at))
    );
  };

  const rows: OverviewRow[] = summaries
    .filter(inRange)
    .map((s) => {
      const row = rowsById.get(s.id)!;
      const ui = toUiStatus(s.status, meta.prUrl(s.id));
      const live = s.status === 'running' || s.status === 'baselining' || s.status === 'stopping';
      const word = statusWord(s, ui, waiting.has(s.id));
      const valued = row.valueUsdPerUnit !== null;
      const remaining = live && s.startedAt !== null ? Math.max(0, row.timeLimitS - (now - s.startedAt) / 1000) : null;
      return {
        id: s.id,
        name: s.name ?? `${s.repo} ${s.metric}`,
        repo: s.repo,
        status: ui,
        statusLabel: word.label,
        tone: word.tone,
        remainingS: remaining === null ? null : Math.round(remaining),
        metric: s.metric,
        unit: unitForMetric(s.metric),
        direction: s.direction === 'minimize' ? 'lower' : 'higher',
        baseline: s.baselineValue,
        best: s.bestValue,
        gainPct: s.gainPct,
        verifiedWins: s.verifiedWins,
        spentUsd: sum(spend.filter((e) => e.opt === s.id && inside(e.at)).map((e) => e.usd)),
        budgetUsd: s.budgetUsd,
        live,
        valueUsdPerYear: valued ? sum(moves.filter((m) => m.opt === s.id && inside(m.at) && m.usdPerYear !== null).map((m) => m.usdPerYear!)) : null,
      } satisfies OverviewRow;
    })
    .sort((a, b) => Number(b.live) - Number(a.live) || activityAt(summaries, b.id) - activityAt(summaries, a.id));

  const spentUsd = sum(spend.filter((e) => inside(e.at)).map((e) => e.usd));
  const budgetUsd = sum(rows.filter((r) => r.status !== 'draft').map((r) => r.budgetUsd));

  const winMoves = moves.filter((m) => inside(m.at) && m.usdPerYear !== null);
  const names = new Map(summaries.map((s) => [s.id, s.name ?? `${s.repo} ${s.metric}`]));
  let cumulative = 0;
  const wins: OverviewWin[] = winMoves.map((m) => {
    cumulative += m.usdPerYear!;
    return { at: m.at, cumulativeUsdPerYear: cumulative, usdPerYear: m.usdPerYear!, optimizationId: m.opt, optimization: names.get(m.opt)! };
  });
  const valueUsd = cumulative;
  const bases = new Set(winMoves.map((m) => rowsById.get(m.opt)!.valueBasis ?? 'estimated'));

  return {
    range,
    from,
    to: now,
    empty: summaries.length === 0,
    spent: { usd: spentUsd, budgetUsd, optimizations: rows.filter((r) => r.status !== 'draft').length },
    value: hasValueModel ? { usdPerYear: valueUsd, wins: wins.length, basis: bases.size > 0 && !bases.has('estimated') ? 'measured' : 'estimated' } : null,
    returnOnSpend: hasValueModel && valueUsd > 0 && spentUsd > 0 ? { ratio: valueUsd / spentUsd, paybackDays: (spentUsd / valueUsd) * 365 } : null,
    wins,
    needsYou: needs,
    rows,
  };
}

const activityAt = (all: OptimizationSummary[], id: string): number => {
  const s = all.find((x) => x.id === id)!;
  return s.endedAt ?? s.startedAt ?? s.createdAt;
};

function statusWord(s: OptimizationSummary, ui: OverviewRow['status'], waiting: boolean): { label: string; tone: StatusTone } {
  switch (s.status) {
    case 'running':
      return { label: 'Running', tone: 'run' };
    case 'baselining':
      return { label: 'Baselining', tone: 'run' };
    case 'stopping':
      return { label: 'Stopping', tone: 'run' };
    case 'draft':
      return { label: 'Draft', tone: 'off' };
    case 'failed':
      return { label: 'Failed', tone: 'bad' };
    default:
      if (waiting) return { label: 'Waiting on you', tone: 'wait' };
      if (ui === 'pr-open') return { label: 'PR open', tone: 'ok' };
      if (s.verifiedWins === 0) return { label: 'No win', tone: 'off' };
      return { label: s.status === 'stopped' ? 'Stopped' : 'Done', tone: 'ok' };
  }
}

/**
 * What waits on the user, whatever the range: verified wins nobody has opened a pull request for, and recent failed runs.
 * (Environment approvals and expired connectors do not exist in the data model yet.)
 */
function needsYou(summaries: OptimizationSummary[], meta: MetaView, now: number): OverviewNeed[] {
  const ship: OverviewNeed[] = [];
  const failed: OverviewNeed[] = [];
  for (const s of [...summaries].sort((a, b) => (b.endedAt ?? b.createdAt) - (a.endedAt ?? a.createdAt))) {
    const name = s.name ?? `${s.repo} ${s.metric}`;
    if ((s.status === 'completed' || s.status === 'stopped') && s.verifiedWins > 0 && toUiStatus(s.status, meta.prUrl(s.id)) !== 'pr-open') {
      const gain = signedGain(s.direction, s.gainPct);
      ship.push({ id: `ship:${s.id}`, kind: 'ship', optimizationId: s.id, title: `Open a pull request for ${plural(s.verifiedWins, 'verified win')}`, sub: gain ? `${name} · ${gain}` : name, action: 'Review' });
    } else if (s.status === 'failed' && now - (s.endedAt ?? s.startedAt ?? s.createdAt) < FAILED_FOR_DAYS * DAY) {
      failed.push({ id: `failed:${s.id}`, kind: 'failed', optimizationId: s.id, title: 'A run failed', sub: name, action: 'View' });
    }
  }
  return [...ship, ...failed];
}
