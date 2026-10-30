import { CostExplorerClient, GetCostAndUsageCommand, type GetCostAndUsageCommandInput, type ResultByTime } from '@aws-sdk/client-cost-explorer';
import type { CostActuals, CostActualsQuery, CostReconciler, SpendCategory } from '@ratchet/core';
import { TAG_AGENT, TAG_OPT, TAG_ROLE, TAG_SANDBOX, assertValidId } from './util.js';

const DAY_MS = 86_400_000;

/** `YYYY-MM-DD` (UTC), the only date format Cost Explorer accepts for DAILY granularity. */
function ymd(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/**
 * Map the value of the `ratchet:role` / `ratchet:agent` tags of a billed resource to a ledger category.
 * Runners and bench instances carry `ratchet:role`; agent-launched experiments carry only
 * `ratchet:opt` + `ratchet:agent` (the permission boundary forces exactly those); anything tagged
 * only with `ratchet:opt` (volumes, snapshots, data transfer attributed by tag) is `storage_egress`.
 */
export function categoryForTags(role: string, agent: string): SpendCategory {
  if (role === 'agent_runner') return 'agent_runner';
  if (role === 'bench') return 'bench_runner';
  if (role === 'experiment') return 'experiment';
  if (role === '' && agent !== '') return 'experiment';
  return 'storage_egress';
}

/** Cost Explorer returns group keys as `<tag key>$<tag value>` (empty value when the tag is absent). */
function tagValue(key: string | undefined, tag: string): string {
  const k = key ?? '';
  const prefix = `${tag}$`;
  return k.startsWith(prefix) ? k.slice(prefix.length) : '';
}

export interface AwsCostReconcilerOptions {
  /** Injected for tests. Default: a client for `us-east-1` (Cost Explorer's only endpoint). */
  client?: CostExplorerClient;
  /**
   * Also require this `ratchet:sandbox` tag value. Off by default: agent-launched experiment
   * instances carry only `ratchet:opt` and `ratchet:agent`, so a sandbox filter would hide them.
   */
  sandboxId?: string;
  /**
   * Treat days Cost Explorer flags `Estimated` as complete. Default false: such a window is
   * reported incomplete (`pending`) and nothing is written.
   */
  acceptEstimated?: boolean;
}

/**
 * `CostReconciler` backed by AWS Cost Explorer `GetCostAndUsage`: unblended cost of everything
 * tagged `ratchet:opt=<id>`, grouped by the `ratchet:role` and `ratchet:agent` tags and mapped to
 * ledger categories. Requires `ce:GetCostAndUsage` and the `ratchet:*` tags activated as
 * cost-allocation tags (docs/real-env/aws.md). Each request costs $0.01.
 */
export class AwsCostReconciler implements CostReconciler {
  private readonly client: CostExplorerClient;

  constructor(private readonly opts: AwsCostReconcilerOptions = {}) {
    // Cost Explorer is a global service served from us-east-1.
    this.client = opts.client ?? new CostExplorerClient({ region: 'us-east-1' });
  }

  async actuals(q: CostActualsQuery): Promise<CostActuals> {
    assertValidId('optimization', q.optimizationId);
    if (this.opts.sandboxId !== undefined) assertValidId('sandbox', this.opts.sandboxId);
    if (!(q.endMs >= q.startMs)) throw new Error('cost window ends before it starts');

    // End is exclusive: the day after the one containing endMs.
    const start = ymd(q.startMs);
    const end = ymd(Math.floor(q.endMs / DAY_MS) * DAY_MS + DAY_MS);
    const optFilter = { Tags: { Key: TAG_OPT, Values: [q.optimizationId], MatchOptions: ['EQUALS' as const] } };
    const filter: GetCostAndUsageCommandInput['Filter'] =
      this.opts.sandboxId === undefined
        ? optFilter
        : { And: [optFilter, { Tags: { Key: TAG_SANDBOX, Values: [this.opts.sandboxId], MatchOptions: ['EQUALS' as const] } }] };

    const byCategory: Partial<Record<SpendCategory, number>> = {};
    let groups = 0;
    const estimatedDays: string[] = [];
    let token: string | undefined;
    do {
      const r = await this.client.send(
        new GetCostAndUsageCommand({
          TimePeriod: { Start: start, End: end },
          Granularity: 'DAILY',
          Metrics: ['UnblendedCost'],
          Filter: filter,
          GroupBy: [
            { Type: 'TAG', Key: TAG_ROLE },
            { Type: 'TAG', Key: TAG_AGENT },
          ],
          ...(token ? { NextPageToken: token } : {}),
        }),
      );
      for (const day of r.ResultsByTime ?? []) {
        this.addDay(day, byCategory, estimatedDays, () => groups++);
      }
      token = r.NextPageToken;
    } while (token);

    const incomplete = !this.opts.acceptEstimated && estimatedDays.length > 0;
    const note = incomplete
      ? `Cost Explorer still marks ${estimatedDays.length} day(s) as estimated (${estimatedDays[0]}${estimatedDays.length > 1 ? ', ...' : ''}); data is not final`
      : groups === 0
        ? `Cost Explorer returned no cost for tag ${TAG_OPT}=${q.optimizationId} between ${start} and ${end}: data may not be available yet (up to 24 h), or the ratchet:* tags are not activated as cost-allocation tags (Billing > Cost allocation tags; activation takes up to 24 h more)`
        : undefined;
    return { byCategory, groups, complete: !incomplete, ...(note ? { note } : {}) };
  }

  private addDay(day: ResultByTime, out: Partial<Record<SpendCategory, number>>, estimatedDays: string[], countGroup: () => void): void {
    if (day.Estimated) estimatedDays.push(day.TimePeriod?.Start ?? '?');
    for (const g of day.Groups ?? []) {
      const raw = g.Metrics?.['UnblendedCost']?.Amount;
      const usd = Number(raw);
      if (raw === undefined || !Number.isFinite(usd)) throw new Error(`Cost Explorer returned a non-numeric amount ${JSON.stringify(raw)}`);
      if (usd === 0) continue;
      countGroup();
      const category = categoryForTags(tagValue(g.Keys?.[0], TAG_ROLE), tagValue(g.Keys?.[1], TAG_AGENT));
      out[category] = (out[category] ?? 0) + usd;
    }
  }
}
