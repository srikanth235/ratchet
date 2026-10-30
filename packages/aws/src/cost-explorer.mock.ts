import type { CostActuals, CostActualsQuery, CostReconciler } from '@ratchet/core';

export interface MockCostReconcilerOptions {
  /** Canned answer, or a function of the query. Default: complete, no cost. */
  actuals?: CostActuals | ((q: CostActualsQuery) => CostActuals | Promise<CostActuals>);
  /** Make every call fail (simulates Cost Explorer being unreachable / AccessDenied). */
  error?: Error;
}

/** In-memory `CostReconciler` for tests and simulation mode; records every query. */
export class MockCostReconciler implements CostReconciler {
  readonly calls: CostActualsQuery[] = [];

  constructor(private readonly opts: MockCostReconcilerOptions = {}) {}

  async actuals(q: CostActualsQuery): Promise<CostActuals> {
    this.calls.push(q);
    if (this.opts.error) throw this.opts.error;
    const a = this.opts.actuals;
    if (typeof a === 'function') return a(q);
    return a ?? { byCategory: {}, groups: 0, complete: true };
  }
}

/** Build a `CostActuals` from per-category USD, as Cost Explorer would return it once data is final. */
export function billed(byCategory: CostActuals['byCategory']): CostActuals {
  return { byCategory, groups: Object.values(byCategory).filter((v) => v !== 0).length, complete: true };
}
