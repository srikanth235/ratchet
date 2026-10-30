import { DescribeImagesCommand, EC2Client, type Image } from '@aws-sdk/client-ec2';
import { mockClient } from 'aws-sdk-client-mock';
import { beforeEach, describe, expect, it } from 'vitest';
import { findRatchetImages, imageVersionMatches, ImageLookupError, resolveImageRefs } from './images.js';
import { DEFAULT_REGION, environmentRegion, regionForOptimization } from './region.js';

const ec2 = mockClient(EC2Client);
const client = () => new EC2Client({ region: 'us-east-1' });

const img = (id: string, kind: string, version: string, created: string, extra: Partial<Image> = {}): Image => ({
  ImageId: id,
  Name: `ratchet-${kind}-${version}-x86_64`,
  State: 'available',
  Architecture: 'x86_64',
  CreationDate: created,
  Tags: [
    { Key: 'ratchet:image', Value: kind },
    { Key: 'ratchet:image_version', Value: version },
  ],
  ...extra,
});

beforeEach(() => ec2.reset());

describe('imageVersionMatches', () => {
  it('matches the version itself or the version plus a build suffix, never a longer version', () => {
    expect(imageVersionMatches('0.4.0', '0.4.0')).toBe(true);
    expect(imageVersionMatches('0.4.0-20261030-1a2b3c4', '0.4.0')).toBe(true);
    expect(imageVersionMatches('0.4.0+dev', '0.4.0')).toBe(true);
    expect(imageVersionMatches('0.4.01', '0.4.0')).toBe(false);
    expect(imageVersionMatches('0.4.0.1', '0.4.0')).toBe(false);
    expect(imageVersionMatches('0.4.1', '0.4.0')).toBe(false);
    expect(imageVersionMatches('', '0.4.0')).toBe(false);
  });
});

describe('findRatchetImages', () => {
  it('asks EC2 for this account\'s available images tagged ratchet:image=runner|bench of the architecture, and picks the newest matching the version', async () => {
    ec2.on(DescribeImagesCommand).resolves({
      Images: [
        img('ami-0000000000000001', 'runner', '0.4.0-20261001-aaaaaaa', '2026-10-01T10:00:00.000Z'),
        img('ami-0000000000000002', 'runner', '0.4.0-20261020-bbbbbbb', '2026-10-20T10:00:00.000Z'),
        img('ami-0000000000000003', 'runner', '0.3.0-20260901-ccccccc', '2026-09-01T10:00:00.000Z'),
        img('ami-0000000000000004', 'bench', '0.4.0-20261020-bbbbbbb', '2026-10-20T10:00:00.000Z'),
      ],
    });
    const found = await findRatchetImages('0.4.0', { region: 'us-east-1', ec2: client() });
    expect(found.runner?.id).toBe('ami-0000000000000002');
    expect(found.bench?.id).toBe('ami-0000000000000004');
    expect(found.otherVersions.runner).toEqual(['0.3.0-20260901-ccccccc', '0.4.0-20261001-aaaaaaa', '0.4.0-20261020-bbbbbbb']);

    const call = ec2.commandCalls(DescribeImagesCommand)[0]!.args[0].input;
    expect(call.Owners).toEqual(['self']);
    expect(call.Filters).toEqual([
      { Name: 'tag:ratchet:image', Values: ['runner', 'bench'] },
      { Name: 'state', Values: ['available'] },
      { Name: 'architecture', Values: ['x86_64'] },
    ]);
  });

  it('never picks the host-toolchain bench variant or an image of another kind, and honors the architecture', async () => {
    ec2.on(DescribeImagesCommand).resolves({
      Images: [img('ami-0000000000000005', 'bench-host', '0.4.0', '2026-10-30T10:00:00.000Z'), img('ami-0000000000000006', 'bench', '0.4.0', '2026-10-01T10:00:00.000Z')],
    });
    const found = await findRatchetImages('0.4.0', { region: 'eu-west-1', arch: 'arm64', ec2: client() });
    expect(found.bench?.id).toBe('ami-0000000000000006');
    expect(ec2.commandCalls(DescribeImagesCommand)[0]!.args[0].input.Filters).toContainEqual({ Name: 'architecture', Values: ['arm64'] });
  });

  it('reports nothing for a version that was not built, and lists what exists', async () => {
    ec2.on(DescribeImagesCommand).resolves({ Images: [img('ami-0000000000000007', 'bench', '0.3.0', '2026-09-01T10:00:00.000Z')] });
    const found = await findRatchetImages('0.4.0', { region: 'us-east-1', ec2: client() });
    expect(found.bench).toBeUndefined();
    expect(found.otherVersions.bench).toEqual(['0.3.0']);
  });

  it('rejects a version that could smuggle anything into a filter', async () => {
    await expect(findRatchetImages('0.4.0 OR *', { region: 'us-east-1', ec2: client() })).rejects.toThrow(/not a plain version/);
  });
});

describe('resolveImageRefs', () => {
  it('uses the tag lookup when no override is given, so the user never types an AMI id', async () => {
    ec2.on(DescribeImagesCommand).resolves({
      Images: [img('ami-0000000000000010', 'runner', '0.4.0', '2026-10-20T10:00:00.000Z'), img('ami-0000000000000011', 'bench', '0.4.0', '2026-10-20T10:00:00.000Z')],
    });
    const r = await resolveImageRefs({ appVersion: '0.4.0', region: 'us-east-1', ec2: client() });
    expect(r).toEqual({ runner: 'ami-0000000000000010', bench: 'ami-0000000000000011', source: { runner: 'tag', bench: 'tag' } });
  });

  it('an explicit override wins and is not looked up; a lookup is skipped when everything is overridden', async () => {
    const r = await resolveImageRefs({ appVersion: '0.4.0', region: 'us-east-1', override: { runner: 'ami-0123456789abcdef0', bench: 'ami-0fedcba9876543210' }, ec2: client() });
    expect(r).toMatchObject({ runner: 'ami-0123456789abcdef0', bench: 'ami-0fedcba9876543210', source: { runner: 'override', bench: 'override' } });
    expect(ec2.commandCalls(DescribeImagesCommand)).toHaveLength(0);
  });

  it('looks up only what is not overridden and only what is needed', async () => {
    ec2.on(DescribeImagesCommand).resolves({ Images: [img('ami-0000000000000011', 'bench', '0.4.0', '2026-10-20T10:00:00.000Z')] });
    const r = await resolveImageRefs({ appVersion: '0.4.0', region: 'us-east-1', need: ['bench'], ec2: client() });
    expect(r.bench).toBe('ami-0000000000000011');
    expect(r.runner).toBeUndefined();
    const mixed = await resolveImageRefs({ appVersion: '0.4.0', region: 'us-east-1', override: { runner: 'ami-0123456789abcdef0' }, ec2: client() });
    expect(mixed.source).toEqual({ runner: 'override', bench: 'tag' });
  });

  it('fails with an actionable error when a needed image is not published for this version', async () => {
    ec2.on(DescribeImagesCommand).resolves({ Images: [img('ami-0000000000000007', 'bench', '0.3.0', '2026-09-01T10:00:00.000Z')] });
    const err = await resolveImageRefs({ appVersion: '0.4.0', region: 'eu-west-1', need: ['bench'], ec2: client() }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ImageLookupError);
    expect((err as Error).message).toContain('no Ratchet bench image for version 0.4.0 (x86_64) in eu-west-1');
    expect((err as Error).message).toContain('found versions: 0.3.0');
    expect((err as Error).message).toContain('infra/images/build.sh bench');
    expect((err as Error).message).toContain('Advanced');
  });

  it('rejects an override that is not an ami id', async () => {
    await expect(resolveImageRefs({ appVersion: '0.4.0', region: 'us-east-1', override: { bench: 'ubuntu' }, ec2: client() })).rejects.toThrow(/not an ami-... id/);
  });
});

describe('region per optimization (environment.region)', () => {
  it('reads the repo\'s region defensively and falls back to the setting, then to us-east-1', () => {
    expect(environmentRegion({ environment: { region: 'eu-west-1' } })).toBe('eu-west-1');
    expect(environmentRegion({})).toBeUndefined();
    expect(environmentRegion({ environment: 'nope' })).toBeUndefined();
    expect(environmentRegion(null)).toBeUndefined();
    expect(regionForOptimization({ environment: { region: 'ap-southeast-2' } }, 'us-west-2')).toBe('ap-southeast-2');
    expect(regionForOptimization({}, 'us-west-2')).toBe('us-west-2');
    expect(regionForOptimization({})).toBe(DEFAULT_REGION);
    expect(DEFAULT_REGION).toBe('us-east-1');
  });
  it('refuses a value that is not a region name instead of launching somewhere else', () => {
    for (const bad of ['europe', 'us_east_1', 'US-EAST-1', 'us-east-1; rm', 7]) {
      expect(() => environmentRegion({ environment: { region: bad } }), String(bad)).toThrow(/not an AWS region name/);
    }
    expect(() => regionForOptimization({}, 'not a region')).toThrow(/not an AWS region name/);
  });
});
