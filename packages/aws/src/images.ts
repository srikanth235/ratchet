import { DescribeImagesCommand, EC2Client, type Image } from '@aws-sdk/client-ec2';

/**
 * Finds the machine images (AMIs) the app launches, by TAG, so nobody types an AMI id.
 *
 * `infra/images` (Packer) tags every AMI and snapshot it builds with
 *
 *   ratchet:image          runner | bench      (the legacy host-toolchain bench variant is `bench-host`: never found here)
 *   ratchet:image_version  <app version>[-<build suffix>]
 *   ratchet:arch           x86_64 | arm64
 *
 * and the app asks EC2 (DescribeImages, own account only) for the newest image of each kind whose version matches the app version.
 * An explicit AMI id (the "Advanced" override, for development) always wins over the lookup.
 */

export type RatchetImageKind = 'runner' | 'bench';
export type RatchetArch = 'x86_64' | 'arm64';

export const TAG_IMAGE = 'ratchet:image';
export const TAG_IMAGE_VERSION = 'ratchet:image_version';

export interface RatchetImage {
  id: string;
  name: string;
  /** The `ratchet:image_version` tag. */
  version: string;
  arch: string;
  createdAt: string;
}

export interface FoundImages {
  runner?: RatchetImage;
  bench?: RatchetImage;
  /** Versions that exist per kind but did not match, for error messages. */
  otherVersions: { runner: string[]; bench: string[] };
}

export interface FindImagesOptions {
  region: string;
  /** Instance architecture of the images to find (match the instance types the app launches). Default `x86_64`. */
  arch?: RatchetArch | undefined;
  profile?: string | undefined;
  /** Injected in tests. */
  ec2?: Pick<EC2Client, 'send'> | undefined;
  /** Accounts that own the images. Default `self`: the sandbox account, where Packer built them. */
  owners?: string[] | undefined;
}

const VERSION_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,39}$/;

/**
 * `tag` matches the app version when it is that version, or the version followed by a build suffix (`0.4.0` matches `0.4.0`,
 * `0.4.0-20261030-1a2b3c4`, `0.4.0+dev`), never a longer version (`0.4.0` does not match `0.4.01` or `0.4.0.1`).
 */
export function imageVersionMatches(tag: string, appVersion: string): boolean {
  return tag === appVersion || tag.startsWith(`${appVersion}-`) || tag.startsWith(`${appVersion}+`);
}

const tagOf = (i: Image, key: string): string | undefined => i.Tags?.find((t) => t.Key === key)?.Value;

/** The newest image of each kind whose `ratchet:image_version` matches `appVersion`. Never throws for "not found": see `resolveImageRefs`. */
export async function findRatchetImages(appVersion: string, opts: FindImagesOptions): Promise<FoundImages> {
  if (!VERSION_RE.test(appVersion)) throw new Error(`app version ${JSON.stringify(appVersion)} is not a plain version string`);
  const arch = opts.arch ?? 'x86_64';
  const ec2 = opts.ec2 ?? new EC2Client({ region: opts.region, ...(opts.profile ? { profile: opts.profile } : {}) });
  const res = await ec2.send(
    new DescribeImagesCommand({
      Owners: opts.owners ?? ['self'],
      Filters: [
        { Name: `tag:${TAG_IMAGE}`, Values: ['runner', 'bench'] },
        { Name: 'state', Values: ['available'] },
        { Name: 'architecture', Values: [arch] },
      ],
    }),
  );
  const out: FoundImages = { otherVersions: { runner: [], bench: [] } };
  for (const kind of ['runner', 'bench'] as const) {
    const ofKind = (res.Images ?? []).filter((i) => tagOf(i, TAG_IMAGE) === kind && i.ImageId && (i.State ?? 'available') === 'available');
    const matching = ofKind.filter((i) => imageVersionMatches(tagOf(i, TAG_IMAGE_VERSION) ?? '', appVersion));
    // ISO-8601 timestamps sort lexicographically.
    matching.sort((a, b) => (b.CreationDate ?? '').localeCompare(a.CreationDate ?? ''));
    const best = matching[0];
    if (best) {
      out[kind] = { id: best.ImageId!, name: best.Name ?? '', version: tagOf(best, TAG_IMAGE_VERSION) ?? '', arch: best.Architecture ?? arch, createdAt: best.CreationDate ?? '' };
    }
    out.otherVersions[kind] = [...new Set(ofKind.map((i) => tagOf(i, TAG_IMAGE_VERSION) ?? '(untagged)'))].sort();
  }
  return out;
}

/** Thrown when an image the app needs is not published for this version, with what to do about it. */
export class ImageLookupError extends Error {
  constructor(
    readonly kind: RatchetImageKind,
    readonly version: string,
    readonly region: string,
    readonly arch: string,
    readonly found: string[],
  ) {
    super(
      `no Ratchet ${kind} image for version ${version} (${arch}) in ${region} owned by this account` +
        (found.length ? `; found versions: ${found.join(', ')}` : '; found none') +
        `. Build it with infra/images/build.sh ${kind} (it tags the AMI ratchet:image=${kind} and ratchet:image_version), ` +
        'in the sandbox account and region, or set the AMI id explicitly under Settings > Advanced (development only).',
    );
    this.name = 'ImageLookupError';
  }
}

export interface ResolvedImages {
  runner?: string;
  bench?: string;
  /** Where each id came from, for the Environments screen. */
  source: { runner?: 'override' | 'tag'; bench?: 'override' | 'tag' };
}

const AMI_RE = /^ami-[0-9a-f]{8,17}$/;

/**
 * The AMI ids to launch: an explicit override wins, otherwise the tag lookup (only for the kinds that are needed and not
 * overridden). `need` says which kinds the caller will launch; a missing needed image is an `ImageLookupError`.
 */
export async function resolveImageRefs(a: {
  appVersion: string;
  region: string;
  need?: readonly RatchetImageKind[];
  override?: { runner?: string | undefined; bench?: string | undefined };
  arch?: RatchetArch | undefined;
  profile?: string | undefined;
  ec2?: FindImagesOptions['ec2'];
}): Promise<ResolvedImages> {
  const need = a.need ?? ['runner', 'bench'];
  const out: ResolvedImages = { source: {} };
  const missing: RatchetImageKind[] = [];
  for (const kind of need) {
    const o = a.override?.[kind];
    if (o) {
      if (!AMI_RE.test(o)) throw new Error(`the ${kind} AMI override ${JSON.stringify(o)} is not an ami-... id`);
      out[kind] = o;
      out.source[kind] = 'override';
    } else missing.push(kind);
  }
  if (missing.length === 0) return out;
  const found = await findRatchetImages(a.appVersion, { region: a.region, arch: a.arch, profile: a.profile, ec2: a.ec2 });
  for (const kind of missing) {
    const hit = found[kind];
    if (!hit) throw new ImageLookupError(kind, a.appVersion, a.region, a.arch ?? 'x86_64', found.otherVersions[kind]);
    out[kind] = hit.id;
    out.source[kind] = 'tag';
  }
  return out;
}
