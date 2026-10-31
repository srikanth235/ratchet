// View models of the Overview (and the calm Optimizations list). Re-exported from types.ts.
import type { OptimizationStatus } from '@ratchet/ui';

/** Window the Overview covers. `all` = everything in the ledger. */
export type OverviewRange = '30d' | '90d' | 'all';
export const OVERVIEW_RANGES: readonly OverviewRange[] = ['30d', '90d', 'all'];

/** Dot colour of a status word: brass (running), amber (waiting on you), mint (done), grey (nothing), rose (failed). */
export type StatusTone = 'run' | 'wait' | 'ok' | 'off' | 'bad';

/** One step of the "value locked in" staircase: a verified win with a value model. */
export interface OverviewWin {
  /** When the ratchet moved. */
  at: number;
  /** Cumulative value locked in after this win, USD per year. */
  cumulativeUsdPerYear: number;
  /** What this win added, USD per year. */
  usdPerYear: number;
  optimizationId: string;
  optimization: string;
}

export interface OverviewNeed {
  id: string;
  /** `ship`: verified wins that have no pull request yet; `failed`: the run failed. */
  kind: 'ship' | 'failed';
  optimizationId: string;
  title: string;
  sub: string;
  /** Button label. */
  action: string;
}

export interface OverviewRow {
  id: string;
  name: string;
  repo: string;
  /** Ledger status as the pills use it (`pr-open` once a pull request exists). */
  status: OptimizationStatus;
  /** The status word shown next to the dot ("Running", "Waiting on you", "No win"). */
  statusLabel: string;
  tone: StatusTone;
  /** Seconds left of the time limit while running, else null. */
  remainingS: number | null;
  metric: string;
  unit: string;
  direction: 'lower' | 'higher';
  baseline: number | null;
  best: number | null;
  /** Signed gain vs baseline in percent (positive = better); null without a baseline. */
  gainPct: number | null;
  verifiedWins: number;
  /** Spend in the range (lifetime when the range is `all`). */
  spentUsd: number;
  budgetUsd: number;
  live: boolean;
  /** Value of the wins in the range, USD per year; null without a value model. */
  valueUsdPerYear: number | null;
}

export interface OverviewVM {
  range: OverviewRange;
  /** The window, epoch ms. `from` is the first activity for `all`. */
  from: number;
  to: number;
  /** The ledger has no optimization at all (the Overview shows one calm empty state). */
  empty: boolean;
  spent: {
    usd: number;
    /** Budget of the optimizations listed in `rows`. */
    budgetUsd: number;
    optimizations: number;
  };
  /** Null when no optimization has a value model ("Set a value"). */
  value: { usdPerYear: number; wins: number; basis: 'estimated' | 'measured' } | null;
  /** Yearly value per $1 spent, and days until the spend is earned back; null without value or spend. */
  returnOnSpend: { ratio: number; paybackDays: number } | null;
  /** Staircase of cumulative value, oldest first; empty without a value model or wins in the range. */
  wins: OverviewWin[];
  needsYou: OverviewNeed[];
  /** Optimizations with activity in the range (drafts created in it included), newest first. */
  rows: OverviewRow[];
}
