import { existsSync } from 'node:fs';
import { openLedger, reconcileSpend, type CostReconcileReport, type CostReconciler } from '@ratchet/core';
import { AwsCostReconciler } from '@ratchet/aws';
import type { Io } from '../io.js';

export interface CostReconcileOptions {
  /** Path of the ledger SQLite file (the desktop app's `ratchet.db`). Falls back to `$RATCHET_DB`. */
  db?: string | undefined;
  json?: boolean | undefined;
  /** Hours after the optimization ended before billing data is trusted. Default 24. */
  lagHours?: number | undefined;
  /** Treat days Cost Explorer still flags `Estimated` as final. */
  acceptEstimated?: boolean | undefined;
}

/** Injection points, so tests run without AWS. */
export interface CostDeps {
  /** Build the billing source. Default: `AwsCostReconciler` using the standard AWS credential chain. */
  reconciler?: (o: CostReconcileOptions) => CostReconciler;
  now?: () => number;
}

const usd = (n: number): string => `${n < 0 ? '-' : ''}$${Math.abs(n).toFixed(4)}`;
const signed = (n: number): string => `${n < 0 ? '-' : '+'}$${Math.abs(n).toFixed(4)}`;

export function formatReconcileReport(r: CostReconcileReport): string {
  const out: string[] = [];
  const headline =
    r.status === 'pending'
      ? `pending: nothing was written. ${r.reason ?? ''}`.trim()
      : r.status === 'unchanged'
        ? 'unchanged: the ledger already matches billing'
        : `reconciled: ${r.lines.filter((l) => l.written).length} correction row(s) written, ${signed(r.writtenUsd)} in total`;
  out.push(`Cost reconcile ${r.optimizationId}: ${headline}`);
  if (r.lines.length > 0) {
    out.push('');
    out.push(`  ${'category'.padEnd(14)} ${'ledger'.padStart(10)} ${'billed'.padStart(10)} ${'delta'.padStart(10)}`);
    for (const l of r.lines) {
      out.push(`  ${l.category.padEnd(14)} ${usd(l.ledgerUsd).padStart(10)} ${usd(l.actualUsd).padStart(10)} ${signed(l.deltaUsd).padStart(10)}${l.written ? '  written' : ''}`);
    }
  }
  return out.join('\n') + '\n';
}

/**
 * `ratchet cost reconcile <optimizationId> --db <ratchet.db>`
 *
 * Compares the ledger's estimated infrastructure spend with AWS Cost Explorer (tag `ratchet:opt`)
 * and appends correction rows (`estimated=false`, `source='cost-explorer'`). Safe to re-run. While
 * billing data is incomplete (Cost Explorer lags up to 24 h) it reports `pending` and writes nothing,
 * which is exit code 0. Exit code 1: unknown optimization, unreadable ledger, or a billing error.
 */
export async function runCostReconcile(optimizationId: string, opts: CostReconcileOptions, io: Io, deps: CostDeps = {}): Promise<number> {
  const dbPath = opts.db ?? process.env['RATCHET_DB'];
  if (!dbPath) {
    io.err('no ledger given: pass --db <path to ratchet.db> or set RATCHET_DB\n');
    return 1;
  }
  if (!existsSync(dbPath)) {
    io.err(`ledger not found: ${dbPath}\n`);
    return 1;
  }
  const ledger = openLedger(dbPath);
  try {
    if (!ledger.getOptimization(optimizationId)) {
      io.err(`optimization ${optimizationId} not found in ${dbPath}\n`);
      return 1;
    }
    const reconciler = deps.reconciler?.(opts) ?? new AwsCostReconciler({ ...(opts.acceptEstimated ? { acceptEstimated: true } : {}) });
    let report: CostReconcileReport;
    try {
      report = await reconcileSpend(ledger, reconciler, optimizationId, {
        now: (deps.now ?? Date.now)(),
        ...(opts.lagHours !== undefined ? { lagMs: opts.lagHours * 3_600_000 } : {}),
      });
    } catch (err) {
      io.err(`cost reconcile failed: ${(err as Error).message}\n`);
      const name = (err as { name?: string }).name;
      if (name === 'AccessDeniedException' || /ce:GetCostAndUsage/.test((err as Error).message)) {
        io.err('the AWS identity needs ce:GetCostAndUsage (see docs/real-env/aws.md, "Cost Explorer reconcile")\n');
      }
      return 1;
    }
    io.out(opts.json ? JSON.stringify(report, null, 2) + '\n' : formatReconcileReport(report));
    return 0;
  } finally {
    ledger.close();
  }
}
