import type { Mode } from '../shared/types.js';

/**
 * One sandbox per mode. Simulation and live must never share a ledger row: a sandbox "applied" by the mock would
 * otherwise hand fake subnets and buckets to the real AWS clients when the user switches to live mode.
 */
export const SANDBOX_IDS: Record<Mode, string> = { live: 'ratchet-sandbox', simulation: 'ratchet-sim' };

export const sandboxIdFor = (mode: Mode): string => SANDBOX_IDS[mode];
