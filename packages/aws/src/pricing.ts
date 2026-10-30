import { GetProductsCommand, PricingClient } from '@aws-sdk/client-pricing';
import type { Clock, PriceBook } from '@ratchet/core';
import { systemClock } from './util.js';

/**
 * Approximate us-east-1 Linux on-demand USD/hour. For simulation and as a
 * fallback; the live number comes from AwsPriceBook.
 */
export const STATIC_HOURLY_USD: Readonly<Record<string, number>> = {
  't3.medium': 0.0416,
  't3.large': 0.0832,
  't3.xlarge': 0.1664,
  'c7i.large': 0.08925,
  'c7i.xlarge': 0.1785,
  'c7i.2xlarge': 0.357,
  'c7i.4xlarge': 0.714,
  'c7i.8xlarge': 1.428,
  'c7i.12xlarge': 2.142,
  'c7i.16xlarge': 2.856,
  'c7i.24xlarge': 4.284,
  'c7i.48xlarge': 8.568,
  'm7i.large': 0.1008,
  'm7i.xlarge': 0.2016,
  'm7i.2xlarge': 0.4032,
  'm7i.4xlarge': 0.8064,
  'm7i.8xlarge': 1.6128,
  'm7i.12xlarge': 2.4192,
  'm7i.16xlarge': 3.2256,
  'm7i.24xlarge': 4.8384,
  'm7i.48xlarge': 9.6768,
};

export class StaticPriceBook implements PriceBook {
  private readonly table: Record<string, number>;
  constructor(overrides: Record<string, number> = {}) {
    this.table = { ...STATIC_HOURLY_USD, ...overrides };
  }
  async hourlyUsd(instanceType: string): Promise<number> {
    const p = this.table[instanceType];
    if (p === undefined) throw new Error(`no static price for instance type ${instanceType}`);
    return p;
  }
}

export interface AwsPriceBookOptions {
  /** Region whose prices to read, e.g. `us-east-1`. */
  region: string;
  /** The Pricing API only exists in us-east-1 / ap-south-1 / eu-central-1. */
  client?: PricingClient;
  /** Named profile in ~/.aws/config; default credential chain when unset. */
  profile?: string;
  clock?: Clock;
  /** Cache lifetime. Default 24h. */
  ttlMs?: number;
  /** Used when the Pricing API fails or has no match (e.g. offline). */
  fallback?: PriceBook;
}

export class AwsPriceBook implements PriceBook {
  private readonly client: PricingClient;
  private readonly clock: Clock;
  private readonly ttl: number;
  private readonly cache = new Map<string, { usd: number; at: number }>();
  private readonly inflight = new Map<string, Promise<number>>();

  constructor(private readonly opts: AwsPriceBookOptions) {
    this.client = opts.client ?? new PricingClient({ region: 'us-east-1', ...(opts.profile ? { profile: opts.profile } : {}) });
    this.clock = opts.clock ?? systemClock;
    this.ttl = opts.ttlMs ?? 24 * 3600_000;
  }

  async hourlyUsd(instanceType: string): Promise<number> {
    const hit = this.cache.get(instanceType);
    if (hit && this.clock.now() - hit.at < this.ttl) return hit.usd;
    let p = this.inflight.get(instanceType);
    if (!p) {
      p = this.fetch(instanceType)
        .then((usd) => {
          this.cache.set(instanceType, { usd, at: this.clock.now() });
          return usd;
        })
        .catch((e) => {
          if (this.opts.fallback) return this.opts.fallback.hourlyUsd(instanceType);
          throw e;
        })
        .finally(() => this.inflight.delete(instanceType));
      this.inflight.set(instanceType, p);
    }
    return p;
  }

  private async fetch(instanceType: string): Promise<number> {
    const term = (field: string, value: string) => ({ Type: 'TERM_MATCH' as const, Field: field, Value: value });
    const r = await this.client.send(
      new GetProductsCommand({
        ServiceCode: 'AmazonEC2',
        FormatVersion: 'aws_v1',
        MaxResults: 10,
        Filters: [
          term('instanceType', instanceType),
          term('regionCode', this.opts.region),
          term('operatingSystem', 'Linux'),
          term('tenancy', 'Shared'),
          term('preInstalledSw', 'NA'),
          term('capacitystatus', 'Used'),
        ],
      }),
    );
    for (const item of r.PriceList ?? []) {
      const usd = parseOnDemandUsd(typeof item === 'string' ? item : JSON.stringify(item));
      if (usd !== null) return usd;
    }
    throw new Error(`no on-demand price found for ${instanceType} in ${this.opts.region}`);
  }
}

/** Extract the hourly USD from one Pricing API `PriceList` entry. */
export function parseOnDemandUsd(priceListJson: string): number | null {
  const doc = JSON.parse(priceListJson) as {
    terms?: { OnDemand?: Record<string, { priceDimensions?: Record<string, { unit?: string; pricePerUnit?: { USD?: string } }> }> };
  };
  for (const term of Object.values(doc.terms?.OnDemand ?? {})) {
    for (const dim of Object.values(term.priceDimensions ?? {})) {
      const usd = Number(dim.pricePerUnit?.USD);
      if (dim.unit === 'Hrs' && Number.isFinite(usd) && usd > 0) return usd;
    }
  }
  return null;
}
