import { describe, it, expect, beforeEach } from 'vitest';
import { mockClient } from 'aws-sdk-client-mock';
import { GetProductsCommand, PricingClient } from '@aws-sdk/client-pricing';
import { AwsPriceBook, StaticPriceBook, parseOnDemandUsd } from './pricing.js';
import { ManualClock } from './util.js';

const pricing = mockClient(PricingClient);
const item = (usd: string, unit = 'Hrs') =>
  JSON.stringify({ terms: { OnDemand: { 'SKU.TERM': { priceDimensions: { 'SKU.TERM.RATE': { unit, pricePerUnit: { USD: usd } } } } } } });

beforeEach(() => pricing.reset());

describe('StaticPriceBook', () => {
  it('knows common c7i/m7i types and supports overrides', async () => {
    const p = new StaticPriceBook({ 'c7i.large': 1 });
    expect(await p.hourlyUsd('c7i.2xlarge')).toBeCloseTo(0.357);
    expect(await p.hourlyUsd('m7i.xlarge')).toBeCloseTo(0.2016);
    expect(await p.hourlyUsd('c7i.large')).toBe(1);
    await expect(p.hourlyUsd('z9.mega')).rejects.toThrow(/no static price/);
  });
  it('prices scale linearly with size for c7i', async () => {
    const p = new StaticPriceBook();
    expect((await p.hourlyUsd('c7i.4xlarge')) / (await p.hourlyUsd('c7i.large'))).toBeCloseTo(8, 5);
  });
});

describe('AwsPriceBook', () => {
  const make = (extra = {}) => {
    const clock = new ManualClock(0);
    return { clock, book: new AwsPriceBook({ region: 'us-east-1', client: new PricingClient({}), clock, ttlMs: 1000, ...extra }) };
  };

  it('queries the Pricing API with exact Linux/shared/on-demand filters', async () => {
    pricing.on(GetProductsCommand).resolves({ PriceList: [item('0.3570000000')] as never });
    const { book } = make();
    expect(await book.hourlyUsd('c7i.2xlarge')).toBeCloseTo(0.357);
    const input = pricing.commandCalls(GetProductsCommand)[0]!.args[0].input;
    expect(input.ServiceCode).toBe('AmazonEC2');
    expect(input.Filters).toEqual([
      { Type: 'TERM_MATCH', Field: 'instanceType', Value: 'c7i.2xlarge' },
      { Type: 'TERM_MATCH', Field: 'regionCode', Value: 'us-east-1' },
      { Type: 'TERM_MATCH', Field: 'operatingSystem', Value: 'Linux' },
      { Type: 'TERM_MATCH', Field: 'tenancy', Value: 'Shared' },
      { Type: 'TERM_MATCH', Field: 'preInstalledSw', Value: 'NA' },
      { Type: 'TERM_MATCH', Field: 'capacitystatus', Value: 'Used' },
    ]);
  });

  it('caches within ttl, refetches after, and dedupes concurrent lookups', async () => {
    pricing.on(GetProductsCommand).resolves({ PriceList: [item('0.5')] as never });
    const { book, clock } = make();
    await Promise.all([book.hourlyUsd('m7i.large'), book.hourlyUsd('m7i.large')]);
    await book.hourlyUsd('m7i.large');
    expect(pricing.commandCalls(GetProductsCommand)).toHaveLength(1);
    clock.advance(1001);
    await book.hourlyUsd('m7i.large');
    expect(pricing.commandCalls(GetProductsCommand)).toHaveLength(2);
  });

  it('errors when nothing matches, or falls back when configured', async () => {
    pricing.on(GetProductsCommand).resolves({ PriceList: [] });
    await expect(make().book.hourlyUsd('c7i.large')).rejects.toThrow(/no on-demand price/);
    const { book } = make({ fallback: new StaticPriceBook() });
    expect(await book.hourlyUsd('c7i.large')).toBeCloseTo(0.08925);
    pricing.on(GetProductsCommand).rejects(new Error('offline'));
    expect(await make({ fallback: new StaticPriceBook() }).book.hourlyUsd('c7i.large')).toBeCloseTo(0.08925);
  });

  it('parseOnDemandUsd ignores non-hourly and zero dimensions', () => {
    expect(parseOnDemandUsd(item('1.25'))).toBe(1.25);
    expect(parseOnDemandUsd(item('1.25', 'GB'))).toBeNull();
    expect(parseOnDemandUsd(item('0.0000000000'))).toBeNull();
    expect(parseOnDemandUsd('{}')).toBeNull();
  });
});
