import type { InstanceInfo, SpendCategory } from '@ratchet/core';
import type { Deps } from './deps.js';
import { K } from './kv.js';

const HOUR_MS = 3_600_000;

const CATEGORY: Record<InstanceInfo['role'], SpendCategory> = {
  agent_runner: 'agent_runner',
  experiment: 'experiment',
  bench: 'bench_runner',
};

/**
 * Live cost estimate: tagged instances x on-demand price, accrued since a per-instance watermark
 * (kept in the kv table, so it survives restarts and never double counts). Called once per tick;
 * the tick interval is therefore the metering interval.
 */
export class CostMeter {
  private readonly priceCache = new Map<string, number>();

  constructor(private readonly d: Deps) {}

  private async hourly(instanceType: string): Promise<number> {
    try {
      const p = await this.d.prices.hourlyUsd(instanceType);
      if (!Number.isFinite(p) || p < 0) throw new RangeError(`bad price ${p} for ${instanceType}`);
      this.priceCache.set(instanceType, p);
      return p;
    } catch (err) {
      this.d.reportError(err, `price lookup ${instanceType}`);
      return this.priceCache.get(instanceType) ?? this.d.settings.fallbackHourlyUsd;
    }
  }

  /** Records spend for everything accrued up to now. Returns the USD added. */
  async meter(optimizationId: string, instances?: InstanceInfo[]): Promise<number> {
    const { ledger, kv, clock, compute } = this.d;
    const now = clock.now();
    const list = instances ?? (await compute.listTagged(optimizationId));
    const agentIds = new Set(ledger.listAgents(optimizationId).map((a) => a.id));

    const rows: Array<{ inst: InstanceInfo; usd: number; to: number }> = [];
    for (const inst of list) {
      if (inst.optimizationId !== optimizationId) continue;
      const from = kv.getNumber(K.acct(inst.id)) ?? inst.launchedAt;
      // A terminated instance with no recorded end time cannot be charged past what we already have.
      const to = Math.min(now, inst.terminatedAt ?? (inst.state === 'terminated' ? from : now));
      if (!(to > from)) continue;
      const usd = ((await this.hourly(inst.instanceType)) * (to - from)) / HOUR_MS;
      rows.push({ inst, usd, to });
    }
    if (rows.length === 0) return 0;

    let added = 0;
    ledger.transaction(() => {
      for (const { inst, usd, to } of rows) {
        if (usd > 0) {
          ledger.recordSpend({
            optimizationId,
            agentId: inst.agentId && agentIds.has(inst.agentId) ? inst.agentId : null,
            category: CATEGORY[inst.role],
            usd,
            estimated: true,
            source: `ec2:${inst.id}`,
            at: now,
          });
          added += usd;
        }
        kv.set(K.acct(inst.id), String(to));
      }
    });
    return added;
  }
}
