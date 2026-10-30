/**
 * Cost reconcile: compare the ledger's estimated infrastructure spend with billing data and append
 * correction rows (docs/V0_ARCHITECTURE.md §7). `spend_events` is append-only, so a correction is a
 * new row `estimated=false, source='cost-explorer'` whose amount may be negative.
 *
 * Idempotence: the correction for a category is `billed - (everything the ledger already holds for
 * that category, estimates and earlier corrections)`, so a second run with the same billing data
 * computes 0 and writes nothing. When billing catches up later, only the remainder is written.
 */
import type { Ledger } from '../ledger/store.js';
import type { OptimizationRow, SpendCategory } from '../ledger/schema.js';
import type { CostReconcileLine, CostReconcileReport, CostReconciler } from '../ports/index.js';

export const COST_RECONCILE_SOURCE = 'cost-explorer';

/** Cost Explorer data can lag up to ~24 h behind usage. */
export const COST_EXPLORER_LAG_MS = 24 * 3_600_000;

/** Categories that come from cloud billing. `tokens` are model API spend, which AWS does not bill. */
export const RECONCILED_CATEGORIES: readonly SpendCategory[] = ['agent_runner', 'experiment', 'bench_runner', 'storage_egress'];

export interface ReconcileSpendOptions {
  /** "Now" (epoch ms), from the engine's clock. */
  now: number;
  /** How long after the optimization ended billing is assumed incomplete. Default 24 h. */
  lagMs?: number;
  /** Ignore corrections smaller than this (USD). Default 1e-6. */
  epsilonUsd?: number;
}

const TERMINAL = new Set<OptimizationRow['status']>(['completed', 'stopped', 'failed']);

/**
 * Reconcile one optimization. Writes nothing (status `pending`) while the optimization is still
 * running, within the billing lag of its end, when the billing system flags its data as estimated,
 * or when it returned no data at all while the ledger has estimates (typically: cost-allocation
 * tags not activated yet). Rejecting those cases is what keeps a lagging report from "correcting"
 * real estimates down to zero.
 */
export async function reconcileSpend(
  ledger: Ledger,
  reconciler: CostReconciler,
  optimizationId: string,
  opts: ReconcileSpendOptions,
): Promise<CostReconcileReport> {
  const opt = ledger.getOptimization(optimizationId);
  if (!opt) throw new Error(`optimization ${optimizationId} not found`);
  const lagMs = opts.lagMs ?? COST_EXPLORER_LAG_MS;
  const eps = opts.epsilonUsd ?? 1e-6;
  // The window ends when the optimization did (all instances are terminated before that), so the
  // still-open current billing day is never part of the query.
  const startMs = opt.startedAt ?? opt.createdAt;
  const window = { startMs, endMs: Math.max(startMs, opt.endedAt ?? opts.now) };

  const earlyPending = (reason: string): CostReconcileReport => ({ optimizationId, at: opts.now, status: 'pending', reason, window, lines: [], writtenUsd: 0 });

  // Cheap gates first: no billing query (each Cost Explorer request costs money) while it cannot be final.
  if (!TERMINAL.has(opt.status) || opt.endedAt === null) {
    return earlyPending(`optimization is ${opt.status}; reconcile after it has ended`);
  }
  const readyAt = opt.endedAt + lagMs;
  if (opts.now < readyAt) {
    const hours = Math.ceil((readyAt - opts.now) / 3_600_000);
    return earlyPending(`billing data lags up to ${Math.round(lagMs / 3_600_000)} h behind usage; try again in about ${hours} h`);
  }

  const actuals = await reconciler.actuals({ optimizationId, ...window });
  const current = ledger.spendBreakdown(optimizationId).byCategory;
  const pending = (reason: string): CostReconcileReport => ({
    ...earlyPending(reason),
    lines: RECONCILED_CATEGORIES.map((category) => {
      const actualUsd = actuals.byCategory[category] ?? 0;
      return { category, ledgerUsd: current[category], actualUsd, deltaUsd: actualUsd - current[category], written: false };
    }),
  });

  if (!actuals.complete) return pending(actuals.note ?? 'billing data for this period is still marked as estimated');
  const estimatedTotal = RECONCILED_CATEGORIES.reduce((a, c) => a + current[c], 0);
  if (actuals.groups === 0 && Math.abs(estimatedTotal) > eps) {
    return pending(
      actuals.note ?? 'billing returned no cost data for this optimization (cost-allocation tags not activated yet, or data not available yet)',
    );
  }

  // Re-read inside the write transaction so two concurrent reconciles cannot both correct the same gap.
  const lines = ledger.transaction(() => {
    const now = ledger.spendBreakdown(optimizationId).byCategory;
    return RECONCILED_CATEGORIES.map((category): CostReconcileLine => {
      const actualUsd = actuals.byCategory[category] ?? 0;
      const deltaUsd = actualUsd - now[category];
      const written = Math.abs(deltaUsd) > eps;
      if (written) {
        ledger.recordSpend({ optimizationId, agentId: null, category, usd: deltaUsd, estimated: false, source: COST_RECONCILE_SOURCE, at: opts.now });
      }
      return { category, ledgerUsd: now[category], actualUsd, deltaUsd, written };
    });
  });
  const writtenUsd = lines.reduce((a, l) => a + (l.written ? l.deltaUsd : 0), 0);
  return {
    optimizationId,
    at: opts.now,
    status: lines.some((l) => l.written) ? 'reconciled' : 'unchanged',
    window,
    lines,
    writtenUsd,
  };
}
