/** Sidebar entries, in the order of the calm design (design/v02.html). `settings` is the Connectors screen. */
export const NAV = [
  { id: 'overview', label: 'Overview' },
  { id: 'optimizations', label: 'Optimizations' },
  { id: 'benchmarks', label: 'Benchmarks' },
  { id: 'settings', label: 'Connectors' },
] as const;

/**
 * Sidebar entries plus the routes reached through buttons: `new` from New optimization, `agents` / `result` / `sandbox` from
 * an optimization (or the sandbox status line). They are not in the sidebar; they belong to Optimizations.
 */
export type NavId = (typeof NAV)[number]['id'] | 'new' | 'agents' | 'result' | 'sandbox';

/** The sidebar entry a route is shown under. */
export function navFor(route: NavId): (typeof NAV)[number]['id'] | null {
  switch (route) {
    case 'new':
    case 'agents':
    case 'result':
      return 'optimizations';
    case 'sandbox':
      return null;
    default:
      return route;
  }
}
