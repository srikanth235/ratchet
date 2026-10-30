import type { AgentId, CloudCompute, Clock, InstanceInfo, OptimizationId, PriceBook, SpendCategory } from '@ratchet/core';
import { systemClock } from './util.js';

/** Instance role -> ledger spend category (packages/core/src/ledger/schema.ts). */
export const ROLE_CATEGORY: Record<InstanceInfo['role'], SpendCategory> = {
  agent_runner: 'agent_runner',
  experiment: 'experiment',
  bench: 'bench_runner',
};

export type MeteredCategory = 'agent_runner' | 'experiment' | 'bench_runner';

/** Shape accepted by `Ledger.recordSpend` (`RecordSpendInput`). */
export interface SpendDelta {
  optimizationId: OptimizationId;
  agentId?: AgentId;
  category: MeteredCategory;
  usd: number;
  estimated: true;
  source: string;
  at: number;
}

export interface InstanceCost {
  instanceId: string;
  optimizationId: OptimizationId;
  agentId?: AgentId;
  category: MeteredCategory;
  instanceType: string;
  billedSeconds: number;
  hourlyUsd: number;
  usd: number;
}

export interface CostSnapshot {
  at: number;
  totalUsd: number;
  byOptimization: Record<OptimizationId, { totalUsd: number; byCategory: Partial<Record<MeteredCategory, number>>; byAgent: Record<AgentId, number> }>;
  instances: InstanceCost[];
  /** Instance types the price book could not price (cost counted as 0 - treat as a metering gap). */
  unpriced: string[];
}

export interface CostMeterOptions {
  compute: CloudCompute;
  prices: PriceBook;
  clock?: Clock;
  /** EC2 Linux bills per second with a 60 s minimum. */
  minBilledSeconds?: number;
  source?: string;
}

/**
 * Live spend estimate: instance lifetimes x on-demand price. `collect()` is
 * meant to run about once a minute and returns only the new spend since the
 * previous call, ready for `Ledger.recordSpend`.
 */
export class CostMeter {
  private readonly clock: Clock;
  private readonly minBilled: number;
  private readonly emitted = new Map<string, number>(); // instanceId -> usd already emitted
  private readonly endedAt = new Map<string, number>(); // first time we saw it ended without a timestamp

  constructor(private readonly opts: CostMeterOptions) {
    this.clock = opts.clock ?? systemClock;
    this.minBilled = opts.minBilledSeconds ?? 60;
  }

  /** Cumulative cost of every tagged instance (optionally one optimization). */
  async snapshot(optimizationId?: OptimizationId): Promise<CostSnapshot> {
    const now = this.clock.now();
    const list = await this.opts.compute.listTagged(optimizationId);
    const snap: CostSnapshot = { at: now, totalUsd: 0, byOptimization: {}, instances: [], unpriced: [] };
    const priceCache = new Map<string, number | null>();
    for (const i of list) {
      let price = priceCache.get(i.instanceType);
      if (price === undefined) {
        price = await this.opts.prices.hourlyUsd(i.instanceType).catch(() => null);
        priceCache.set(i.instanceType, price);
        if (price === null) snap.unpriced.push(i.instanceType);
      }
      const seconds = this.billedSeconds(i, now);
      const usd = price === null ? 0 : (seconds / 3600) * price;
      const category = ROLE_CATEGORY[i.role] as MeteredCategory;
      snap.instances.push({
        instanceId: i.id,
        optimizationId: i.optimizationId,
        ...(i.agentId ? { agentId: i.agentId } : {}),
        category,
        instanceType: i.instanceType,
        billedSeconds: seconds,
        hourlyUsd: price ?? 0,
        usd,
      });
      snap.totalUsd += usd;
      const o = (snap.byOptimization[i.optimizationId] ??= { totalUsd: 0, byCategory: {}, byAgent: {} });
      o.totalUsd += usd;
      o.byCategory[category] = (o.byCategory[category] ?? 0) + usd;
      if (i.agentId) o.byAgent[i.agentId] = (o.byAgent[i.agentId] ?? 0) + usd;
    }
    return snap;
  }

  /** New spend since the previous `collect()`, aggregated per (optimization, agent, category). */
  async collect(optimizationId?: OptimizationId): Promise<SpendDelta[]> {
    const snap = await this.snapshot(optimizationId);
    const agg = new Map<string, SpendDelta>();
    for (const c of snap.instances) {
      const delta = c.usd - (this.emitted.get(c.instanceId) ?? 0);
      if (delta <= 1e-9) continue;
      this.emitted.set(c.instanceId, c.usd);
      const key = `${c.optimizationId}\0${c.agentId ?? ''}\0${c.category}`;
      const cur = agg.get(key);
      if (cur) cur.usd += delta;
      else
        agg.set(key, {
          optimizationId: c.optimizationId,
          ...(c.agentId ? { agentId: c.agentId } : {}),
          category: c.category,
          usd: delta,
          estimated: true,
          source: this.opts.source ?? 'cost-meter',
          at: snap.at,
        });
    }
    return [...agg.values()];
  }

  private billedSeconds(i: InstanceInfo, now: number): number {
    let end: number;
    if (i.state === 'terminated' || i.state === 'stopped') {
      let e = i.terminatedAt ?? this.endedAt.get(i.id);
      if (e === undefined) {
        e = now;
        this.endedAt.set(i.id, e);
      }
      end = e;
    } else {
      end = now;
    }
    const secs = Math.max(0, (end - i.launchedAt) / 1000);
    return Math.max(this.minBilled, secs);
  }
}

export type BudgetState = 'ok' | 'warn' | 'tripped';

export interface BudgetStatus {
  state: BudgetState;
  spentUsd: number;
  budgetUsd: number;
  fraction: number;
  /** Set only on the call that crossed a threshold, so callers act exactly once. */
  crossed: 'warn' | 'trip' | null;
}

export interface BudgetGuardOptions {
  budgetUsd: number;
  warnFraction?: number;
  onWarn?: (s: BudgetStatus) => void;
  /** Wire the kill switch + reaper here. */
  onTrip?: (s: BudgetStatus) => void;
}

/** Warn at 80% of budget, trip (kill switch) at 100%. State only moves forward. */
export class BudgetGuard {
  private state: BudgetState = 'ok';
  private readonly warnFraction: number;

  constructor(private readonly opts: BudgetGuardOptions) {
    if (!(opts.budgetUsd > 0)) throw new Error('budgetUsd must be > 0');
    this.warnFraction = opts.warnFraction ?? 0.8;
  }

  /** `spentUsd` is the total for the optimization (infra + tokens). */
  update(spentUsd: number): BudgetStatus {
    const fraction = spentUsd / this.opts.budgetUsd;
    let crossed: BudgetStatus['crossed'] = null;
    if (fraction >= 1 && this.state !== 'tripped') {
      this.state = 'tripped';
      crossed = 'trip';
    } else if (fraction >= this.warnFraction && this.state === 'ok') {
      this.state = 'warn';
      crossed = 'warn';
    }
    const status: BudgetStatus = { state: this.state, spentUsd, budgetUsd: this.opts.budgetUsd, fraction, crossed };
    if (crossed === 'warn') this.opts.onWarn?.(status);
    if (crossed === 'trip') this.opts.onTrip?.(status);
    return status;
  }

  get current(): BudgetState {
    return this.state;
  }
}
