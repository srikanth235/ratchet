/**
 * The region of an optimization comes from the repo (`environment.region` in ratchet.yaml), not from a global setting;
 * the account default is `us-east-1`.
 */
export const DEFAULT_REGION = 'us-east-1';

const REGION_RE = /^[a-z]{2}(?:-[a-z]+)+-\d$/;

export const isRegion = (r: string): boolean => REGION_RE.test(r);

/**
 * Defensive read of `config.environment.region` (the schema key is added elsewhere; any other shape is ignored). A value that is
 * present but is not a region name is an error: silently falling back would launch machines in a region the repo did not ask for.
 */
export function environmentRegion(config: unknown): string | undefined {
  // TODO(schema): use the typed `config.environment.region` once packages/core declares it.
  const env = (config as { environment?: unknown } | null | undefined)?.environment;
  if (!env || typeof env !== 'object' || Array.isArray(env)) return undefined;
  const r = (env as { region?: unknown }).region;
  if (r === undefined || r === null || r === '') return undefined;
  if (typeof r !== 'string' || !isRegion(r)) throw new Error(`environment.region ${JSON.stringify(r)} is not an AWS region name (like us-east-1)`);
  return r;
}

/** The region for one optimization: the repo's, else the fallback (settings), else `us-east-1`. */
export function regionForOptimization(config: unknown, fallback?: string): string {
  const r = environmentRegion(config) ?? fallback ?? DEFAULT_REGION;
  if (!isRegion(r)) throw new Error(`region ${JSON.stringify(r)} is not an AWS region name (like us-east-1)`);
  return r;
}
