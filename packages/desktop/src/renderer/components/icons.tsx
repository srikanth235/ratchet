import type { ReactNode } from 'react';
import type { NavId } from '../nav';

const P: Record<NavId | 'plus' | 'chevron', ReactNode> = {
  optimizations: <path d="M3 17h4v-4h4V9h4V5h4" />,
  benchmarks: <path d="M4 20V10M10 20V4M16 20v-8M22 20H2" />,
  agents: <><circle cx="12" cy="8" r="3.5" /><path d="M5 20c0-4 3-6 7-6s7 2 7 6" /></>,
  environments: <><path d="M4 8l8-4 8 4-8 4-8-4z" /><path d="M4 12l8 4 8-4M4 16l8 4 8-4" /></>,
  spend: <><circle cx="12" cy="12" r="8" /><path d="M12 7v10M9.5 9.5c0-1 1-1.5 2.5-1.5s2.5.6 2.5 1.7c0 2.3-5 1.2-5 3.6 0 1.1 1.1 1.7 2.5 1.7s2.5-.6 2.5-1.5" /></>,
  settings: <><circle cx="12" cy="12" r="3" /><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" /></>,
  plus: <path d="M12 5v14M5 12h14" />,
  chevron: <path d="M8 10l4 4 4-4" />,
};

export function Icon({ name, size = 18 }: { name: keyof typeof P; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {P[name]}
    </svg>
  );
}
