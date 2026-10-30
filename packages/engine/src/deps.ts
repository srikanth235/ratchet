import type { AgentUsage, Clock, CloudCompute, Ledger, PriceBook } from '@ratchet/core';
import type { KvStore } from './kv.js';
import type { ResolvedSettings } from './settings.js';

/** What the helper modules (cost meter, reaper) need from the engine. */
export interface Deps {
  ledger: Ledger;
  kv: KvStore;
  clock: Clock;
  compute: CloudCompute;
  prices: PriceBook;
  settings: ResolvedSettings;
  reportError(err: unknown, context: string): void;
}

export function tokenCostUsd(usage: AgentUsage, pricing: { inputUsdPerMTok: number; outputUsdPerMTok: number }): number {
  if (usage.costUsd !== undefined && Number.isFinite(usage.costUsd)) return Math.max(0, usage.costUsd);
  const inTok = Number.isFinite(usage.inputTokens) ? usage.inputTokens : 0;
  const outTok = Number.isFinite(usage.outputTokens) ? usage.outputTokens : 0;
  return (Math.max(0, inTok) * pricing.inputUsdPerMTok + Math.max(0, outTok) * pricing.outputUsdPerMTok) / 1_000_000;
}
