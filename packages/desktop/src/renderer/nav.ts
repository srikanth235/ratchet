/** Sidebar entries, in the order of the design. Labels follow design/Main.dc.html; the page titles carry the longer names. */
export const NAV = [
  { id: 'optimizations', label: 'Optimizations' },
  { id: 'benchmarks', label: 'Benchmarks' },
  { id: 'agents', label: 'Agents' },
  { id: 'sandbox', label: 'Environments' },
  { id: 'result', label: 'Result' },
  { id: 'settings', label: 'Settings' },
] as const;

/** Sidebar entries plus the routes reached through buttons. */
export type NavId = (typeof NAV)[number]['id'] | 'new';
