export const NAV = [
  { id: 'optimizations', label: 'Optimizations' },
  { id: 'agents', label: 'Live agents' },
  { id: 'benchmarks', label: 'Benchmark history' },
  { id: 'result', label: 'Result' },
  { id: 'sandbox', label: 'AWS sandbox' },
  { id: 'settings', label: 'Settings' },
] as const;

/** Sidebar entries plus the routes reached through buttons. */
export type NavId = (typeof NAV)[number]['id'] | 'new';
