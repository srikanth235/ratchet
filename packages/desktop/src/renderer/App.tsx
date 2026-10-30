import { useCallback, useMemo, useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { NavContext, SnapshotContext, useQuery, type NavState } from './data';
import type { NavId } from './nav';
import { Benchmarks } from './screens/Benchmarks';
import { LiveAgents } from './screens/LiveAgents';
import { NewOptimization } from './screens/NewOptimization';
import { Optimizations } from './screens/Optimizations';
import { Result } from './screens/Result';
import { Sandbox } from './screens/Sandbox';
import { Settings } from './screens/Settings';

export function App({ initialRoute = 'optimizations' }: { initialRoute?: NavId }) {
  const [route, setRoute] = useState<NavId>(initialRoute);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const snapshot = useQuery(() => window.ratchet.listOptimizations(), [], { intervalMs: 10_000 });

  const go = useCallback((r: NavId) => setRoute(r), []);
  const nav = useMemo<NavState>(
    () => ({ route, go, selectedId, select: setSelectedId, open: (id, r) => { setSelectedId(id); setRoute(r); } }),
    [route, go, selectedId],
  );

  return (
    <NavContext.Provider value={nav}>
      <SnapshotContext.Provider value={snapshot}>
        <div className="shell">
          <Sidebar />
          <main className="content" data-screen={route}>
            {route === 'optimizations' && <Optimizations />}
            {route === 'new' && <NewOptimization />}
            {route === 'agents' && <LiveAgents />}
            {route === 'benchmarks' && <Benchmarks />}
            {route === 'result' && <Result />}
            {route === 'sandbox' && <Sandbox />}
            {route === 'settings' && <Settings />}
          </main>
        </div>
      </SnapshotContext.Provider>
    </NavContext.Provider>
  );
}
