import type { EnvironmentScope } from '../config/schema.js';
import type { PriceBook } from '../ports/index.js';
import type { EnvironmentCost, EnvironmentResource, EnvironmentRole } from './types.js';

/**
 * Approximate us-east-1 Linux on-demand USD per hour. A static table is enough for an ESTIMATE shown before anything is
 * created; the real bill is reconciled from Cost Explorer after the run. `estimateEnvironmentCostLive` takes a `PriceBook` (the
 * live AWS pricing port) and falls back to this table.
 */
export const INSTANCE_USD_PER_HOUR: Readonly<Record<string, number>> = {
  't3.medium': 0.0416,
  't3.large': 0.0832,
  't3.xlarge': 0.1664,
  't3.2xlarge': 0.3328,
  't4g.small': 0.0168,
  't4g.medium': 0.0336,
  't4g.large': 0.0672,
  't4g.xlarge': 0.1344,
  'c7i.large': 0.08925,
  'c7i.xlarge': 0.1785,
  'c7i.2xlarge': 0.357,
  'c7i.4xlarge': 0.714,
  'c7i.8xlarge': 1.428,
  'c7i.12xlarge': 2.142,
  'c7i.16xlarge': 2.856,
  'c7g.large': 0.0725,
  'c7g.xlarge': 0.145,
  'c7g.2xlarge': 0.29,
  'c7g.4xlarge': 0.58,
  'm7i.large': 0.1008,
  'm7i.xlarge': 0.2016,
  'm7i.2xlarge': 0.4032,
  'm7i.4xlarge': 0.8064,
  'm7i.8xlarge': 1.6128,
  'm7g.large': 0.0816,
  'm7g.xlarge': 0.1632,
  'm7g.2xlarge': 0.3264,
  'm7g.4xlarge': 0.6528,
  'r7i.large': 0.1323,
  'r7i.xlarge': 0.2646,
  'r7i.2xlarge': 0.5292,
  'r7i.4xlarge': 1.0584,
  'r7g.large': 0.1071,
  'r7g.xlarge': 0.2142,
  'r7g.2xlarge': 0.4284,
  'r7g.4xlarge': 0.8568,
};

/** EBS gp3, USD per GB-month (us-east-1). */
export const GP3_USD_PER_GB_MONTH = 0.08;
const HOURS_PER_MONTH = 730;

/** Minutes of sandbox start-up (VPC, endpoints, roles) before the first machine boots. */
export const BASE_STARTUP_MINUTES = 2;
/** Added start-up per machine (boot + user data + seed load). */
export const STARTUP_MINUTES_PER_MACHINE = 1;

/** Hourly USD of an instance type from the static table, or `undefined` when the table does not know it. */
export function staticHourlyUsd(instanceType: string): number | undefined {
  return INSTANCE_USD_PER_HOUR[instanceType];
}

/** Hourly cost of `gb` gigabytes of gp3. */
export function volumeUsdPerHour(gb: number): number {
  return (Math.max(0, gb) * GP3_USD_PER_GB_MONTH) / HOURS_PER_MONTH;
}

const round = (n: number, places: number): number => {
  const f = 10 ** places;
  return Math.round(n * f) / f;
};

export interface MachineSpec {
  /** The key in the base module's `machines` map (absent for the default machines of `none` / `single`). */
  key?: string;
  name: string;
  role: EnvironmentRole;
  machine: string;
  detail: string;
  /** Data volume attached to the machine, GB (gp3). */
  dataVolumeGb?: number;
  /** Root volume, GB. Default 30. */
  rootVolumeGb?: number;
}

export interface CostEstimateOptions {
  /** The run's time limit, seconds. */
  timeLimitSeconds: number;
  /** Hourly price per instance type (default: the static table). Missing types are reported in `unpriced`. */
  prices?: Readonly<Record<string, number>>;
}

export interface CostEstimate {
  resources: EnvironmentResource[];
  cost: EnvironmentCost;
  /** Instance types the price table does not know: their rows are priced 0 and the draft must not pass the cost check. */
  unpriced: string[];
}

/**
 * Pure cost math for a set of machines: `usdPerHour` is the sum of instance and volume prices (rounded to cents of a cent),
 * `startupMinutes` is the sandbox boot plus one minute per machine (0 with no machine), and the run estimate bills the
 * machines for the run's time limit plus the start-up (they are up while the stack loads).
 */
export function estimateMachinesCost(machines: readonly MachineSpec[], opts: CostEstimateOptions): CostEstimate {
  const prices = opts.prices ?? INSTANCE_USD_PER_HOUR;
  const unpriced: string[] = [];
  const resources: EnvironmentResource[] = machines.map((m) => {
    const base = prices[m.machine];
    if (base === undefined && !unpriced.includes(m.machine)) unpriced.push(m.machine);
    const usd = (base ?? 0) + volumeUsdPerHour((m.rootVolumeGb ?? 30) + (m.dataVolumeGb ?? 0));
    return { name: m.name, role: m.role, machine: m.machine, detail: m.detail, usdPerHour: round(usd, 4) };
  });
  const usdPerHour = round(
    resources.reduce((sum, r) => sum + r.usdPerHour, 0),
    4,
  );
  const startupMinutes = machines.length === 0 ? 0 : BASE_STARTUP_MINUTES + STARTUP_MINUTES_PER_MACHINE * machines.length;
  const runHours = Math.max(0, opts.timeLimitSeconds) / 3600;
  const estimateForRunUsd = round(usdPerHour * (runHours + startupMinutes / 60), 2);
  return { resources, cost: { usdPerHour, startupMinutes, estimateForRunUsd }, unpriced };
}

/** As `estimateMachinesCost`, with prices from a live `PriceBook`; a type the book cannot price falls back to the static table. */
export async function estimateEnvironmentCostLive(machines: readonly MachineSpec[], opts: CostEstimateOptions, book: PriceBook): Promise<CostEstimate> {
  const prices: Record<string, number> = { ...INSTANCE_USD_PER_HOUR };
  for (const t of new Set(machines.map((m) => m.machine))) {
    try {
      prices[t] = await book.hourlyUsd(t);
    } catch {
      /* keep the static price, if any */
    }
  }
  return estimateMachinesCost(machines, { ...opts, prices });
}

/** The one machine of a scope that needs no drafting: the evaluator's host (`none`) or the pinned machine (`single`). */
export function defaultMachines(scope: EnvironmentScope, machine: string): MachineSpec[] {
  if (scope === 'full') return [];
  return [
    scope === 'none'
      ? { name: 'Evaluator', role: 'evaluator', machine, detail: 'runs the evaluation in its container, on pinned hardware' }
      : { name: 'Machine', role: 'app', machine, detail: 'builds and measures the repo on pinned hardware' },
  ];
}
