export const NAV = [
  { id: 'optimizations', label: 'Optimizations' },
  { id: 'benchmarks', label: 'Benchmarks' },
  { id: 'agents', label: 'Agents' },
  { id: 'environments', label: 'Environments' },
  { id: 'spend', label: 'Spend' },
  { id: 'settings', label: 'Settings' },
] as const;

export type NavId = (typeof NAV)[number]['id'];
