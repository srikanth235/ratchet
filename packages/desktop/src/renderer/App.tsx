import { useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { NAV, type NavId } from './nav';
import { Optimizations } from './screens/Optimizations';
import { Placeholder } from './screens/Placeholder';

export function App() {
  const [route, setRoute] = useState<NavId>('optimizations');
  const label = NAV.find((n) => n.id === route)!.label;
  return (
    <div className="shell">
      <Sidebar active={route} onNavigate={setRoute} />
      <main className="content">
        {route === 'optimizations' ? <Optimizations /> : <Placeholder title={label} />}
      </main>
    </div>
  );
}
