import { createContext, useCallback, useContext, useEffect, useRef, useState, useSyncExternalStore, type DependencyList } from 'react';
import type { EngineEventVM, KpiRange, OptimizationsSnapshot } from '../shared/types';
import type { NavId } from './nav';

// ------------------------------------------------------------------ navigation

export interface NavState {
  route: NavId;
  go(route: NavId): void;
  /** The optimization the per-optimization screens (agents, benchmarks, result) show. */
  selectedId: string | null;
  select(id: string | null): void;
  /** Select + navigate. */
  open(id: string, route: NavId): void;
}

export const NavContext = createContext<NavState>({
  route: 'optimizations',
  go: () => undefined,
  selectedId: null,
  select: () => undefined,
  open: () => undefined,
});
export const useNav = (): NavState => useContext(NavContext);

// ------------------------------------------------------------------ data

export interface Query<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  refresh(): void;
}

/**
 * Load data through `window.ratchet`, and re-load (debounced) whenever the engine pushes an event that
 * passes `relevant`. `intervalMs` is a safety net for state that changes without events (elapsed time).
 */
export function useQuery<T>(
  load: () => Promise<T>,
  deps: DependencyList,
  opts: { relevant?: (e: EngineEventVM) => boolean; intervalMs?: number; skip?: boolean } = {},
): Query<T> {
  const [state, setState] = useState<{ data: T | null; error: string | null; loading: boolean }>({ data: null, error: null, loading: !opts.skip });
  const loadRef = useRef(load);
  loadRef.current = load;
  const relevantRef = useRef(opts.relevant);
  relevantRef.current = opts.relevant;
  const seq = useRef(0);

  const run = useCallback(() => {
    if (opts.skip) return;
    const n = ++seq.current;
    loadRef.current().then(
      (data) => n === seq.current && setState({ data, error: null, loading: false }),
      (e: unknown) => n === seq.current && setState((s) => ({ ...s, error: errMsg(e), loading: false })),
    );
  }, [opts.skip]);

  useEffect(() => {
    setState((s) => ({ ...s, loading: !opts.skip }));
    run();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = window.ratchet.onEvent((e) => {
      if (relevantRef.current && !relevantRef.current(e)) return;
      clearTimeout(timer);
      timer = setTimeout(run, 350);
    });
    const poll = opts.intervalMs ? setInterval(run, opts.intervalMs) : undefined;
    return () => {
      off();
      clearTimeout(timer);
      if (poll) clearInterval(poll);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [run, opts.intervalMs, ...deps]);

  return { ...state, refresh: run };
}

export function errMsg(e: unknown): string {
  const m = e instanceof Error ? e.message : String(e);
  // Electron prefixes rejected invokes with "Error invoking remote method '…': Error: "
  return m.replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
}

export const forOptimization =
  (id: string | null) =>
  (e: EngineEventVM): boolean =>
    id !== null && e.optimizationId === id;

// ------------------------------------------------------------------ shared snapshot

/** The Optimizations snapshot, plus the KPI window it was loaded for (the range switch changes it). */
export type SnapshotQuery = Query<OptimizationsSnapshot> & { range: KpiRange; setRange(r: KpiRange): void };

export const SnapshotContext = createContext<SnapshotQuery>({ data: null, error: null, loading: true, refresh: () => undefined, range: '30d', setRange: () => undefined });
export const useSnapshot = (): SnapshotQuery => useContext(SnapshotContext);

// ------------------------------------------------------------------ background start errors

/**
 * Starting an optimization (baseline run, ~1 minute) continues in the background after the form navigates to the
 * Agents screen; a failure to start is parked here and shown on that screen.
 */
const startErrors = new Map<string, string>();
const startErrorListeners = new Set<() => void>();

export function reportStartError(id: string, e: unknown): void {
  startErrors.set(id, errMsg(e));
  startErrorListeners.forEach((l) => l());
}

export function useStartError(id: string | null): string | null {
  return useSyncExternalStore(
    (l) => (startErrorListeners.add(l), () => startErrorListeners.delete(l)),
    () => (id ? (startErrors.get(id) ?? null) : null),
  );
}
